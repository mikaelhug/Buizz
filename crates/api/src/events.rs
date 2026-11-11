use std::convert::Infallible;
use std::future;

use axum::Extension;
use axum::extract::State;
use axum::response::sse::{Event, KeepAlive, Sse};
use buizz_core::api::SpineEvent;
use futures::{Stream, StreamExt, stream};
use tokio_stream::wrappers::{BroadcastStream, WatchStream};

use crate::AppState;
use crate::auth::ActiveSession;

fn frame(event: &SpineEvent) -> Option<Event> {
    Event::default().event(event.name()).json_data(event).ok()
}

#[utoipa::path(get, path = "/api/events", tag = "events",
    responses((status = 200,
        description = "server-sent events; the event name is the `type` field",
        content_type = "text/event-stream", body = SpineEvent)))]
pub(crate) async fn events(
    State(app): State<AppState>,
    Extension(session): Extension<ActiveSession>,
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
    let stream = live
        .take_until(session.ended())
        .filter_map(|event| future::ready(frame(&event).map(Ok)));
    Sse::new(stream).keep_alive(KeepAlive::default())
}
