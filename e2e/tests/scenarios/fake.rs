use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};

use axum::extract::ws::{Message, WebSocket, WebSocketUpgrade};
use axum::extract::{Path, Query, State};
use axum::http::{StatusCode, header};
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use axum::{Json, Router};
use buizz_core::market::{AssetClass, Bar, CorporateActionKind, Symbol};
use buizz_core::time::Timestamp;
use chrono::{Datelike, NaiveDate, NaiveTime, TimeDelta};
use serde_json::{Number, Value, json};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::net::{TcpListener, TcpStream};
use tokio::sync::{broadcast, watch};

use crate::world::World;

const PAGE_ROWS: usize = 1000;
const RETRY_AFTER_SECONDS: &str = "3600";

type Params = HashMap<String, String>;

struct Fake {
    world: Mutex<World>,
    released: watch::Sender<Option<Timestamp>>,
    disconnects: broadcast::Sender<()>,
    hits: Mutex<BTreeMap<&'static str, u32>>,
    rate_limited: Mutex<BTreeMap<&'static str, u32>>,
}

impl Fake {
    fn world(&self) -> MutexGuard<'_, World> {
        self.world.lock().unwrap_or_else(PoisonError::into_inner)
    }

    fn answer(&self, route: &'static str) -> Option<Response> {
        *self
            .hits
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .entry(route)
            .or_default() += 1;
        let mut limited = self
            .rate_limited
            .lock()
            .unwrap_or_else(PoisonError::into_inner);
        let remaining = limited.get_mut(route).filter(|remaining| **remaining > 0)?;
        *remaining -= 1;
        Some(
            (
                StatusCode::TOO_MANY_REQUESTS,
                [(header::RETRY_AFTER, RETRY_AFTER_SECONDS)],
            )
                .into_response(),
        )
    }

    fn visible(&self, bar: &Bar) -> bool {
        self.released
            .borrow()
            .is_none_or(|released| bar.start <= released)
    }
}

#[derive(Clone)]
pub struct FakeProviders {
    pub url: String,
    pub ftp_address: String,
    fake: Arc<Fake>,
}

impl FakeProviders {
    pub async fn serve(mut world: World) -> Self {
        let released = world.released.take();
        let fake = Arc::new(Fake {
            world: Mutex::new(world),
            released: watch::Sender::new(released),
            disconnects: broadcast::Sender::new(4),
            hits: Mutex::default(),
            rate_limited: Mutex::default(),
        });
        let router = Router::new()
            .route("/v2/calendar", get(calendar))
            .route("/v2/assets", get(assets))
            .route("/v2/stocks/bars", get(bars))
            .route("/v1/corporate-actions", get(corporate_actions))
            .route("/stream", get(stream))
            .route("/tiingo/daily/{symbol}/prices", get(tiingo))
            .route("/files/company_tickers.json", get(tickers))
            .route("/files/company_tickers_mf.json", get(fund_tickers))
            .route("/api/xbrl/companyfacts/{file}", get(company_facts))
            .route("/submissions/{file}", get(submissions))
            .route("/w/api.php", get(wikipedia))
            .route(
                "/resource-center/data-chart-center/interest-rates/daily-treasury-rates.csv/{year}/all",
                get(treasury),
            )
            .route("/fred/series/observations", get(fred))
            .route("/control/release", post(release))
            .with_state(fake.clone());
        let listener = TcpListener::bind("127.0.0.1:0")
            .await
            .expect("the fake providers bind");
        let url = format!("http://{}", listener.local_addr().expect("bound address"));
        tokio::spawn(async move { axum::serve(listener, router).await });
        let ftp = TcpListener::bind("127.0.0.1:0")
            .await
            .expect("the fake FTP server binds");
        let ftp_address = ftp.local_addr().expect("bound address").to_string();
        tokio::spawn(serve_ftp(ftp, fake.clone()));
        Self {
            url,
            ftp_address,
            fake,
        }
    }

    pub fn stream_url(&self) -> String {
        format!("{}/stream", self.url.replacen("http", "ws", 1))
    }

    pub fn world<T>(&self, read: impl FnOnce(&mut World) -> T) -> T {
        read(&mut self.fake.world())
    }

    pub fn released(&self) -> Timestamp {
        self.fake
            .released
            .borrow()
            .expect("the world releases bars")
    }

    pub fn release(&self, minutes: i64) -> (Timestamp, Timestamp) {
        let from = self.released() + TimeDelta::minutes(1);
        let through = self.released() + TimeDelta::minutes(minutes);
        self.fake.released.send_replace(Some(through));
        (from, through)
    }

    pub fn rate_limit(&self, route: &'static str, times: u32) {
        self.fake
            .rate_limited
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .insert(route, times);
    }

    pub fn hits(&self) -> BTreeMap<&'static str, u32> {
        self.fake
            .hits
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .clone()
    }

    pub fn drop_streams(&self) {
        let _ = self.fake.disconnects.send(());
    }
}

fn number(value: impl ToString) -> Value {
    Value::Number(
        value
            .to_string()
            .parse::<Number>()
            .expect("decimals render as JSON numbers"),
    )
}

fn wire_bar(bar: &Bar, t: String) -> Value {
    json!({
        "t": t,
        "o": number(bar.open),
        "h": number(bar.high),
        "l": number(bar.low),
        "c": number(bar.close),
        "v": bar.volume,
    })
}

fn param<'a>(params: &'a Params, name: &str) -> &'a str {
    params.get(name).map_or("", String::as_str)
}

fn symbols(params: &Params) -> BTreeSet<Symbol> {
    param(params, "symbols")
        .split(',')
        .filter(|symbol| !symbol.is_empty())
        .map(Symbol::new)
        .collect()
}

fn page_offset(params: &Params) -> usize {
    param(params, "page_token").parse().unwrap_or(0)
}

fn next_token(offset: usize, page: usize, total: usize) -> Value {
    if offset + page < total {
        json!((offset + page).to_string())
    } else {
        Value::Null
    }
}

async fn calendar(State(fake): State<Arc<Fake>>, Query(params): Query<Params>) -> Response {
    if let Some(limited) = fake.answer("calendar") {
        return limited;
    }
    let (start, end): (NaiveDate, NaiveDate) = (
        param(&params, "start").parse().unwrap_or(NaiveDate::MIN),
        param(&params, "end").parse().unwrap_or(NaiveDate::MAX),
    );
    let days: Vec<Value> = fake
        .world()
        .sessions
        .iter()
        .filter(|session| (start..=end).contains(&session.date))
        .map(|session| {
            json!({
                "date": session.date,
                "open": session.open.exchange_time().format("%H:%M").to_string(),
                "close": session.close.exchange_time().format("%H:%M").to_string(),
            })
        })
        .collect();
    Json(days).into_response()
}

async fn assets(State(fake): State<Arc<Fake>>) -> Response {
    if let Some(limited) = fake.answer("assets") {
        return limited;
    }
    let assets: Vec<Value> = fake
        .world()
        .instruments
        .iter()
        .map(|instrument| {
            json!({
                "class": "us_equity",
                "exchange": instrument.exchange,
                "symbol": instrument.symbol,
                "name": instrument.name,
                "status": if instrument.active { "active" } else { "inactive" },
                "tradable": instrument.active,
                "shortable": instrument.shortable,
            })
        })
        .collect();
    Json(assets).into_response()
}

async fn bars(State(fake): State<Arc<Fake>>, Query(params): Query<Params>) -> Response {
    let daily = param(&params, "timeframe") == "1Day";
    if let Some(limited) = fake.answer(if daily { "bars_1d" } else { "bars_1m" }) {
        return limited;
    }
    let wanted = symbols(&params);
    let world = fake.world();
    let mut matching: Vec<&Bar> = if daily {
        let (start, end): (NaiveDate, NaiveDate) = (
            param(&params, "start").parse().expect("daily start date"),
            param(&params, "end").parse().expect("daily end date"),
        );
        world
            .daily
            .iter()
            .filter(|bar| (start..=end).contains(&bar.start.exchange_date()))
            .collect()
    } else {
        let (start, end): (Timestamp, Timestamp) = (
            param(&params, "start").parse().expect("minute start"),
            param(&params, "end").parse().expect("minute end"),
        );
        world
            .minute
            .iter()
            .filter(|bar| (start..=end).contains(&bar.start) && fake.visible(bar))
            .collect()
    };
    matching.retain(|bar| wanted.contains(&bar.symbol));
    matching.sort_by_key(|bar| (bar.symbol.clone(), bar.start));
    let rows: Vec<(&Symbol, Value)> = matching
        .into_iter()
        .map(|bar| {
            let t = if daily {
                Timestamp::at_exchange_time(bar.start.exchange_date(), NaiveTime::MIN)
                    .expect("midnight exists")
            } else {
                bar.start
            };
            (&bar.symbol, wire_bar(bar, t.to_string()))
        })
        .collect();
    let limit = param(&params, "limit").parse().unwrap_or(PAGE_ROWS);
    let page = limit.min(PAGE_ROWS);
    let offset = page_offset(&params);
    let mut grouped: BTreeMap<String, Vec<Value>> = BTreeMap::new();
    for (symbol, value) in rows.iter().skip(offset).take(page) {
        grouped
            .entry(symbol.to_string())
            .or_default()
            .push(value.clone());
    }
    Json(json!({
        "bars": grouped,
        "next_page_token": next_token(offset, page, rows.len()),
    }))
    .into_response()
}

async fn corporate_actions(
    State(fake): State<Arc<Fake>>,
    Query(params): Query<Params>,
) -> Response {
    if let Some(limited) = fake.answer("corporate_actions") {
        return limited;
    }
    let wanted = symbols(&params);
    let (start, end): (NaiveDate, NaiveDate) = (
        param(&params, "start").parse().expect("actions start"),
        param(&params, "end").parse().expect("actions end"),
    );
    let world = fake.world();
    let matching: Vec<_> = world
        .actions
        .iter()
        .filter(|action| wanted.is_empty() || wanted.contains(&action.symbol))
        .filter(|action| (start..=end).contains(&action.ex_date))
        .collect();
    let offset = page_offset(&params);
    let mut lists = json!({});
    if let Some(action) = matching.get(offset) {
        let (list, entry) = match action.kind {
            CorporateActionKind::Split { old_rate, new_rate } => (
                if new_rate >= old_rate {
                    "forward_splits"
                } else {
                    "reverse_splits"
                },
                json!({
                    "symbol": action.symbol,
                    "ex_date": action.ex_date,
                    "old_rate": number(old_rate),
                    "new_rate": number(new_rate),
                }),
            ),
            CorporateActionKind::CashDividend { rate } => (
                "cash_dividends",
                json!({"symbol": action.symbol, "ex_date": action.ex_date, "rate": number(rate)}),
            ),
        };
        lists[list] = json!([entry]);
    }
    Json(json!({
        "corporate_actions": lists,
        "next_page_token": next_token(offset, 1, matching.len()),
    }))
    .into_response()
}

async fn tiingo(
    State(fake): State<Arc<Fake>>,
    Path(symbol): Path<String>,
    Query(params): Query<Params>,
) -> Response {
    if let Some(limited) = fake.answer("tiingo") {
        return limited;
    }
    let (start, end): (NaiveDate, NaiveDate) = (
        param(&params, "startDate").parse().expect("tiingo start"),
        param(&params, "endDate").parse().expect("tiingo end"),
    );
    let symbol = Symbol::new(&symbol);
    let rows: Vec<Value> = fake
        .world()
        .tiingo
        .iter()
        .filter(|bar| bar.symbol == symbol)
        .filter(|bar| (start..=end).contains(&bar.start.exchange_date()))
        .map(|bar| {
            json!({
                "date": format!("{}T00:00:00.000Z", bar.start.exchange_date()),
                "open": number(bar.open),
                "high": number(bar.high),
                "low": number(bar.low),
                "close": number(bar.close),
                "volume": bar.volume,
                "divCash": 0.0,
                "splitFactor": 1.0,
            })
        })
        .collect();
    Json(rows).into_response()
}

async fn tickers(State(fake): State<Arc<Fake>>) -> Response {
    if let Some(limited) = fake.answer("tickers") {
        return limited;
    }
    let entries: BTreeMap<String, Value> = fake
        .world()
        .companies
        .iter()
        .enumerate()
        .map(|(index, company)| {
            (
                index.to_string(),
                json!({"cik_str": company.cik, "ticker": company.ticker, "title": company.ticker}),
            )
        })
        .collect();
    Json(entries).into_response()
}

async fn fund_tickers(State(fake): State<Arc<Fake>>) -> Response {
    if let Some(limited) = fake.answer("fund_tickers") {
        return limited;
    }
    let rows: Vec<Value> = fake
        .world()
        .instruments
        .iter()
        .filter(|instrument| instrument.asset_class == AssetClass::Etf)
        .enumerate()
        .map(|(index, instrument)| {
            json!([
                index,
                format!("S{index:09}"),
                format!("C{index:09}"),
                instrument.symbol
            ])
        })
        .collect();
    Json(json!({"fields": ["cik", "seriesId", "classId", "symbol"], "data": rows})).into_response()
}

async fn submissions(State(fake): State<Arc<Fake>>, Path(file): Path<String>) -> Response {
    if let Some(limited) = fake.answer("submissions") {
        return limited;
    }
    let world = fake.world();
    world
        .companies
        .iter()
        .find(|company| file == format!("CIK{:010}.json", company.cik))
        .map_or_else(
            || StatusCode::NOT_FOUND.into_response(),
            |company| Json(company.submissions.clone()).into_response(),
        )
}

async fn wikipedia(State(fake): State<Arc<Fake>>, Query(params): Query<Params>) -> Response {
    if let Some(limited) = fake.answer("wikipedia") {
        return limited;
    }
    let world = fake.world();
    match world.wikipedia.get(param(&params, "page")) {
        Some(wikitext) => {
            Json(json!({"parse": {"title": param(&params, "page"), "wikitext": wikitext}}))
                .into_response()
        }
        None => Json(json!({"error": {"code": "missingtitle"}})).into_response(),
    }
}

async fn serve_ftp(listener: TcpListener, fake: Arc<Fake>) {
    while let Ok((control, _)) = listener.accept().await {
        tokio::spawn(ftp_session(control, fake.clone()));
    }
}

async fn ftp_session(control: TcpStream, fake: Arc<Fake>) -> std::io::Result<()> {
    let (reader, mut writer) = control.into_split();
    let mut lines = BufReader::new(reader).lines();
    writer.write_all(b"220 fake short stock server\r\n").await?;
    let mut passive: Option<TcpListener> = None;
    while let Some(line) = lines.next_line().await? {
        let (command, argument) = line.split_once(' ').unwrap_or((line.as_str(), ""));
        let reply = match command.to_ascii_uppercase().as_str() {
            "USER" => "331 password please".to_owned(),
            "PASS" => "230 logged in".to_owned(),
            "TYPE" => "200 type set".to_owned(),
            "PASV" => {
                let listener = TcpListener::bind("127.0.0.1:0").await?;
                let port = listener.local_addr()?.port();
                passive = Some(listener);
                format!(
                    "227 Entering Passive Mode (127,0,0,1,{},{})",
                    port >> 8,
                    port & 0xff
                )
            }
            "RETR" => {
                let file = (argument == "usa.txt")
                    .then(|| {
                        let _ = fake.answer("borrow");
                        fake.world().borrow.clone()
                    })
                    .flatten();
                match (file, passive.take()) {
                    (Some(file), Some(listener)) => {
                        writer.write_all(b"150 sending\r\n").await?;
                        let (mut data, _) = listener.accept().await?;
                        data.write_all(&file).await?;
                        data.shutdown().await?;
                        "226 done".to_owned()
                    }
                    _ => "550 no such file".to_owned(),
                }
            }
            "QUIT" => {
                writer.write_all(b"221 bye\r\n").await?;
                return Ok(());
            }
            _ => "502 not implemented".to_owned(),
        };
        writer.write_all(format!("{reply}\r\n").as_bytes()).await?;
    }
    Ok(())
}

async fn company_facts(State(fake): State<Arc<Fake>>, Path(file): Path<String>) -> Response {
    if let Some(limited) = fake.answer("company_facts") {
        return limited;
    }
    let world = fake.world();
    world
        .companies
        .iter()
        .find(|company| file == format!("CIK{:010}.json", company.cik))
        .map_or_else(
            || StatusCode::NOT_FOUND.into_response(),
            |company| Json(company.facts.clone()).into_response(),
        )
}

async fn treasury(State(fake): State<Arc<Fake>>, Path(year): Path<i32>) -> Response {
    if let Some(limited) = fake.answer("treasury") {
        return limited;
    }
    let mut csv = String::from("Date,\"3 Mo\"\n");
    for rate in fake
        .world()
        .rates
        .iter()
        .filter(|rate| rate.date.year() == year)
    {
        csv.push_str(&format!(
            "{},{}\n",
            rate.date.format("%m/%d/%Y"),
            rate.percent
        ));
    }
    csv.into_response()
}

async fn fred(State(fake): State<Arc<Fake>>, Query(params): Query<Params>) -> Response {
    if let Some(limited) = fake.answer("fred") {
        return limited;
    }
    let series = param(&params, "series_id");
    let offset: usize = param(&params, "offset").parse().unwrap_or(0);
    let world = fake.world();
    let vintages: Vec<Value> = world
        .macro_vintages
        .iter()
        .filter(|vintage| vintage.series.fred_id() == Some(series))
        .map(|vintage| {
            json!({
                "realtime_start": vintage.published,
                "realtime_end": "9999-12-31",
                "date": vintage.date,
                "value": vintage.value.map_or_else(|| ".".to_owned(), |value| value.to_string()),
            })
        })
        .collect();
    Json(json!({
        "count": vintages.len(),
        "offset": offset,
        "observations": vintages.iter().skip(offset).take(PAGE_ROWS).collect::<Vec<_>>(),
    }))
    .into_response()
}

async fn release(State(fake): State<Arc<Fake>>, Json(request): Json<Value>) -> Response {
    let minutes = request["minutes"].as_i64().expect("minutes to release");
    let previous = fake.released.borrow().expect("the world releases bars");
    let through = previous + TimeDelta::minutes(minutes);
    fake.released.send_replace(Some(through));
    Json(json!({
        "from": (previous + TimeDelta::minutes(1)).to_string(),
        "through": through.to_string(),
    }))
    .into_response()
}

async fn stream(State(fake): State<Arc<Fake>>, upgrade: WebSocketUpgrade) -> Response {
    upgrade.on_upgrade(move |socket| converse(fake, socket))
}

fn stream_frames(
    fake: &Fake,
    symbols: &BTreeSet<Symbol>,
    after: Option<Timestamp>,
    through: Option<Timestamp>,
) -> Vec<String> {
    let world = fake.world();
    let Some(from) = world.streamed_from else {
        return Vec::new();
    };
    let mut minutes: BTreeMap<Timestamp, Vec<Value>> = BTreeMap::new();
    for bar in &world.minute {
        let fresh = bar.start >= from && after.is_none_or(|after| bar.start > after);
        let released = through.is_none_or(|through| bar.start <= through);
        if fresh && released && symbols.contains(&bar.symbol) {
            let mut value = wire_bar(bar, bar.start.to_string());
            value["T"] = json!("b");
            value["S"] = json!(bar.symbol);
            minutes.entry(bar.start).or_default().push(value);
        }
    }
    minutes
        .into_values()
        .map(|frame| Value::Array(frame).to_string())
        .collect()
}

async fn send(socket: &mut WebSocket, frame: String) -> bool {
    socket.send(Message::text(frame)).await.is_ok()
}

async fn converse(fake: Arc<Fake>, mut socket: WebSocket) {
    let _ = fake.answer("stream");
    let mut released = fake.released.subscribe();
    let mut disconnects = fake.disconnects.subscribe();
    if !send(
        &mut socket,
        r#"[{"T":"success","msg":"connected"}]"#.to_owned(),
    )
    .await
    {
        return;
    }
    loop {
        match socket.recv().await {
            Some(Ok(Message::Text(text))) if text.contains(r#""auth""#) => break,
            Some(Ok(_)) => {}
            _ => return,
        }
    }
    if !send(
        &mut socket,
        r#"[{"T":"success","msg":"authenticated"}]"#.to_owned(),
    )
    .await
    {
        return;
    }
    let mut subscribed: BTreeSet<Symbol> = BTreeSet::new();
    let mut sent_through = *released.borrow_and_update();
    loop {
        let frames = tokio::select! {
            message = socket.recv() => match message {
                Some(Ok(Message::Text(text))) => {
                    let request: Value = serde_json::from_str(&text).unwrap_or_default();
                    let listed: BTreeSet<Symbol> = request["bars"]
                        .as_array()
                        .into_iter()
                        .flatten()
                        .filter_map(Value::as_str)
                        .map(Symbol::new)
                        .collect();
                    let added: BTreeSet<Symbol> = if request["action"] == "unsubscribe" {
                        subscribed.retain(|symbol| !listed.contains(symbol));
                        BTreeSet::new()
                    } else {
                        let added = listed.difference(&subscribed).cloned().collect();
                        subscribed.extend(listed);
                        added
                    };
                    let ack = json!([{"T": "subscription", "trades": [], "quotes": [], "bars": subscribed}]);
                    std::iter::once(ack.to_string())
                        .chain(stream_frames(&fake, &added, None, sent_through))
                        .collect()
                }
                Some(Ok(_)) => Vec::new(),
                _ => return,
            },
            changed = released.changed() => {
                if changed.is_err() {
                    return;
                }
                let through = *released.borrow_and_update();
                let frames = stream_frames(&fake, &subscribed, sent_through, through);
                sent_through = through;
                frames
            }
            _ = disconnects.recv() => {
                let _ = socket.send(Message::Close(None)).await;
                return;
            }
        };
        for frame in frames {
            if !send(&mut socket, frame).await {
                return;
            }
        }
    }
}
