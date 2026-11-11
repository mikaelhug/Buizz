use axum::Router;
use axum::http::{HeaderMap, StatusCode, header::USER_AGENT};
use axum::routing::get;
use buizz_core::market::{
    Calendar, FundamentalFact, FundamentalMetric, PeriodType, Session, Symbol,
};
use buizz_core::time::Timestamp;
use buizz_sources::edgar::{EdgarClient, EdgarConfig};
use chrono::{Datelike, NaiveDate, NaiveTime, TimeDelta, Weekday};
use rust_decimal::Decimal;

const USER_AGENT_VALUE: &str = "buizz-test contact@example.test";
const COMPANY_FACTS: &str = include_str!("../fixtures/edgar/companyfacts_sample.json");
const TICKERS: &str = include_str!("../fixtures/edgar/company_tickers.json");

fn date(year: i32, month: u32, day: u32) -> NaiveDate {
    NaiveDate::from_ymd_opt(year, month, day).unwrap()
}

fn open_of(date: NaiveDate) -> Timestamp {
    Timestamp::at_exchange_time(date, NaiveTime::from_hms_opt(9, 30, 0).unwrap()).unwrap()
}

fn weekday_calendar() -> Calendar {
    let close = NaiveTime::from_hms_opt(16, 0, 0).unwrap();
    let sessions = (0..)
        .map(|offset| date(2021, 1, 1) + TimeDelta::days(offset))
        .take_while(|day| day.year() < 2025)
        .filter(|day| !matches!(day.weekday(), Weekday::Sat | Weekday::Sun))
        .map(|day| Session {
            date: day,
            open: open_of(day),
            close: Timestamp::at_exchange_time(day, close).unwrap(),
        })
        .collect();
    Calendar::new(sessions)
}

fn guarded(
    body: &'static str,
) -> impl Fn(HeaderMap) -> std::future::Ready<(StatusCode, &'static str)> + Clone {
    move |headers| {
        let allowed = headers
            .get(USER_AGENT)
            .is_some_and(|value| value == USER_AGENT_VALUE);
        std::future::ready(if allowed {
            (StatusCode::OK, body)
        } else {
            (StatusCode::FORBIDDEN, "missing user agent")
        })
    }
}

async fn serve_sec() -> String {
    let app = Router::new()
        .route("/files/company_tickers.json", get(guarded(TICKERS)))
        .route(
            "/api/xbrl/companyfacts/CIK0000320193.json",
            get(guarded(COMPANY_FACTS)),
        );
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let url = format!("http://{}", listener.local_addr().unwrap());
    tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
    url
}

fn select(
    facts: &[FundamentalFact],
    metric: FundamentalMetric,
    period_type: PeriodType,
) -> Vec<&FundamentalFact> {
    facts
        .iter()
        .filter(|fact| fact.metric == Some(metric) && fact.period_type == Some(period_type))
        .collect()
}

#[tokio::test]
async fn companyfacts_become_append_only_point_in_time_facts() {
    let url = serve_sec().await;
    let client = EdgarClient::new(EdgarConfig {
        user_agent: USER_AGENT_VALUE.to_owned(),
        data_url: url.clone(),
        www_url: url,
    })
    .unwrap();
    let apple = Symbol::new("aapl");

    let tickers = client.ticker_map().await.unwrap();
    assert_eq!(tickers.get(&apple), Some(&320193));
    assert_eq!(tickers.get(&Symbol::new("BRK.B")), Some(&1067983));

    let facts = client
        .company_facts(&apple, tickers[&apple], &weekday_calendar())
        .await
        .unwrap();

    let revenue = select(&facts, FundamentalMetric::Revenue, PeriodType::Annual);
    let fy2022: Vec<_> = revenue
        .iter()
        .filter(|fact| fact.period_end == date(2022, 9, 24))
        .collect();
    assert_eq!(fy2022.len(), 2);
    assert_eq!(fy2022[0].value, Decimal::from(394_328_000_000_i64));
    assert_eq!(fy2022[0].filed, date(2022, 10, 28));
    assert_eq!(fy2022[0].available_at, open_of(date(2022, 10, 31)));
    assert_eq!(fy2022[1].value, Decimal::from(393_000_000_000_i64));
    assert_eq!(fy2022[1].form, "10-K/A");
    assert_eq!(fy2022[1].accession, "0000320193-23-000120");
    assert_eq!(fy2022[1].available_at, open_of(date(2023, 12, 18)));
    assert_eq!(revenue.len(), 3);

    let net_income = |period_type| select(&facts, FundamentalMetric::NetIncome, period_type);
    assert_eq!(net_income(PeriodType::Annual).len(), 1);
    let quarters = net_income(PeriodType::Quarter);
    assert_eq!(quarters.len(), 1);
    assert_eq!(quarters[0].period_start, Some(date(2022, 3, 27)));
    assert_eq!(quarters[0].value, Decimal::from(19_442_000_000_i64));

    let shares = select(
        &facts,
        FundamentalMetric::SharesOutstanding,
        PeriodType::Annual,
    );
    assert_eq!(shares.len(), 1);
    assert_eq!(shares[0].value, Decimal::from(15_943_425_000_i64));
    assert_eq!(shares[0].unit, "shares");

    assert!(facts.iter().all(|fact| fact.concept != "ConceptWeDoNotMap"));
    assert!(facts.iter().all(|fact| fact.form != "8-K"));
}
