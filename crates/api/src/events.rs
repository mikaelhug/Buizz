use std::convert::Infallible;
use std::future;

use axum::Extension;
use axum::extract::State;
use axum::http::HeaderMap;
use axum::response::sse::{Event, KeepAlive, Sse};
use buizz_core::api::SpineEvent;
use buizz_core::run::RunId;
use futures::{Stream, StreamExt, stream};
use tokio_stream::wrappers::{BroadcastStream, WatchStream};

use crate::AppState;
use crate::auth::ActiveSession;

const REPLAY_LIMIT: u32 = 1000;
const LAST_EVENT_ID: &str = "last-event-id";

#[derive(Clone, Copy)]
struct ResumePoint {
    run_id: RunId,
    seq: u64,
}

fn parse_resume_point(headers: &HeaderMap) -> Option<ResumePoint> {
    let (run_id, seq) = headers.get(LAST_EVENT_ID)?.to_str().ok()?.split_once(':')?;
    Some(ResumePoint {
        run_id: RunId(run_id.parse().ok()?),
        seq: seq.parse().ok()?,
    })
}

fn event_id(event: &SpineEvent) -> Option<String> {
    match event {
        SpineEvent::Log { run_id, event } => Some(format!("{run_id}:{}", event.seq)),
        _ => None,
    }
}

fn frame(event: &SpineEvent) -> Option<Event> {
    let frame = Event::default().event(event.name());
    let frame = match event_id(event) {
        Some(id) => frame.id(id),
        None => frame,
    };
    frame.json_data(event).ok()
}

async fn replay(app: &AppState, resume: ResumePoint) -> Vec<SpineEvent> {
    match app
        .state
        .events(resume.run_id, Some(resume.seq), REPLAY_LIMIT)
        .await
    {
        Ok(events) => events
            .into_iter()
            .map(|event| SpineEvent::Log {
                run_id: resume.run_id,
                event,
            })
            .collect(),
        Err(error) => {
            tracing::warn!(%error, "event replay failed");
            Vec::new()
        }
    }
}

fn replayed_through(backlog: &[SpineEvent], resume: Option<ResumePoint>) -> Option<ResumePoint> {
    match backlog.last() {
        Some(SpineEvent::Log { run_id, event }) => Some(ResumePoint {
            run_id: *run_id,
            seq: event.seq,
        }),
        _ => resume,
    }
}

fn already_sent(event: &SpineEvent, through: Option<ResumePoint>) -> bool {
    match (event, through) {
        (SpineEvent::Log { run_id, event }, Some(through)) => {
            *run_id == through.run_id && event.seq <= through.seq
        }
        _ => false,
    }
}

#[utoipa::path(get, path = "/api/events", tag = "events",
    responses((status = 200,
        description = "server-sent events; the event name is the `type` field. Log events carry the id `<run>:<seq>`; reconnecting with `Last-Event-ID` replays that run's missed log events from the state store",
        content_type = "text/event-stream", body = SpineEvent)))]
pub(crate) async fn events(
    State(app): State<AppState>,
    Extension(session): Extension<ActiveSession>,
    headers: HeaderMap,
) -> Sse<impl Stream<Item = Result<Event, Infallible>>> {
    let runner = BroadcastStream::new(app.runner.events());
    let ingest = BroadcastStream::new(app.ingest.events());
    let feed = WatchStream::new(app.feed.slots()).map(|slots| Ok(SpineEvent::Feed { slots }));
    let live = stream::select(runner, stream::select(ingest, feed)).filter_map(|received| {
        future::ready(match received {
            Ok(event) => Some(event),
            Err(error) => {
                tracing::warn!(%error, "event stream lagged");
                None
            }
        })
    });

    let resume = parse_resume_point(&headers);
    let backlog = match resume {
        Some(resume) => replay(&app, resume).await,
        None => Vec::new(),
    };
    let through = replayed_through(&backlog, resume);
    let live = live.filter(move |event| future::ready(!already_sent(event, through)));

    let stream = stream::iter(backlog)
        .chain(live)
        .take_until(session.ended())
        .filter_map(|event| future::ready(frame(&event).map(Ok)));
    Sse::new(stream).keep_alive(KeepAlive::default())
}
