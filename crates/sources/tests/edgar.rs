use axum::Router;
use axum::http::{HeaderMap, StatusCode, header::USER_AGENT};
use axum::routing::get;
use buizz_core::market::{
    Calendar, FundamentalFact, FundamentalMetric, InsiderOwner, InsiderTransaction, PeriodType,
    Session, Symbol,
};
use buizz_core::time::Timestamp;
use buizz_sources::edgar::{
    EdgarClient, EdgarConfig, RATE_LIMIT, normalize_company_facts, normalize_daily_index,
    normalize_ownership,
};
use buizz_sources::http::SourceError;
use chrono::{Datelike, NaiveDate, NaiveTime, TimeDelta, Weekday};
use rust_decimal::Decimal;

const USER_AGENT_VALUE: &str = "buizz-test contact@example.test";
const COMPANY_FACTS: &str = include_str!("../fixtures/edgar/companyfacts_sample.json");
const TICKERS: &str = include_str!("../fixtures/edgar/company_tickers.json");
const FUND_TICKERS: &str = include_str!("../fixtures/edgar/company_tickers_mf.json");
const SUBMISSIONS: &str = include_str!("../fixtures/edgar/submissions_sample.json");
const SUBMISSIONS_PAGE: &str = include_str!("../fixtures/edgar/submissions_page_sample.json");
const DAILY_INDEX: &str = include_str!("../fixtures/edgar/master_sample.idx");
const FORM_4: &str = include_str!("../fixtures/edgar/form4_sample.xml");

fn date(year: i32, month: u32, day: u32) -> NaiveDate {
    NaiveDate::from_ymd_opt(year, month, day).unwrap()
}

fn open_of(date: NaiveDate) -> Timestamp {
    Timestamp::at_exchange_time(date, NaiveTime::from_hms_opt(9, 30, 0).unwrap()).unwrap()
}

fn weekday_calendar(first_year: i32, last_year: i32) -> Calendar {
    let close = NaiveTime::from_hms_opt(16, 0, 0).unwrap();
    let sessions = (0..)
        .map(|offset| date(first_year, 1, 1) + TimeDelta::days(offset))
        .take_while(|day| day.year() <= last_year)
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
        .route("/files/company_tickers_mf.json", get(guarded(FUND_TICKERS)))
        .route(
            "/api/xbrl/companyfacts/CIK0000320193.json",
            get(guarded(COMPANY_FACTS)),
        )
        .route("/submissions/CIK0000320193.json", get(guarded(SUBMISSIONS)))
        .route(
            "/submissions/CIK0000320193-submissions-001.json",
            get(guarded(SUBMISSIONS_PAGE)),
        )
        .route(
            "/Archives/edgar/daily-index/2026/QTR4/master.20261109.idx",
            get(guarded(DAILY_INDEX)),
        )
        .route(
            "/Archives/edgar/data/320193/000114036126042149/form4.xml",
            get(guarded(FORM_4)),
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
    let funds = client.fund_tickers().await.unwrap();
    assert!(funds.contains(&Symbol::new("SPY")) && !funds.contains(&apple));

    let facts = client
        .company_facts(&apple, tickers[&apple], &weekday_calendar(2021, 2024))
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

#[test]
fn a_later_filing_in_a_preferred_concept_keeps_the_original_filing() {
    let revenue = |end: &str, val: u64, filed: &str, accn: &str| {
        format!(
            r#"{{"start":"{}-01-01","end":"{end}","val":{val},"fy":2019,"fp":"FY","form":"10-K","filed":"{filed}","accn":"{accn}"}}"#,
            &end[..4]
        )
    };
    let payload = format!(
        r#"{{"cik":1,"facts":{{"us-gaap":{{
            "SalesRevenueNet":{{"units":{{"USD":[{}]}}}},
            "RevenueFromContractWithCustomerExcludingAssessedTax":{{"units":{{"USD":[{},{}]}}}}
        }}}}}}"#,
        revenue("2017-12-31", 100, "2018-02-20", "0000000001-18-000001"),
        revenue("2017-12-31", 100, "2020-02-20", "0000000001-20-000001"),
        revenue("2019-12-31", 120, "2020-02-20", "0000000001-20-000001"),
    );
    let facts = normalize_company_facts(
        &Symbol::new("XYZ"),
        payload.as_bytes(),
        &weekday_calendar(2017, 2021),
    )
    .unwrap();
    let fy2017: Vec<_> = select(&facts, FundamentalMetric::Revenue, PeriodType::Annual)
        .into_iter()
        .filter(|fact| fact.period_end == date(2017, 12, 31))
        .collect();
    assert_eq!(fy2017.len(), 1);
    assert_eq!(fy2017[0].concept, "SalesRevenueNet");
    assert_eq!(fy2017[0].filed, date(2018, 2, 20));
    assert_eq!(facts.len(), 2);
}

#[test]
fn a_filer_with_one_earnings_per_share_tag_has_diluted_eps_in_its_reporting_currency() {
    let payload = r#"{"cik":2,"facts":{"us-gaap":{
        "EarningsPerShareBasicAndDiluted":{"units":{"JPY/shares":[
            {"start":"2023-04-01","end":"2024-03-31","val":312.5,"fy":2023,"fp":"FY","form":"10-K","filed":"2024-06-20","accn":"0000000002-24-000001"}
        ]}}
    }}}"#;
    let facts = normalize_company_facts(
        &Symbol::new("XYZ"),
        payload.as_bytes(),
        &weekday_calendar(2024, 2024),
    )
    .unwrap();
    let eps = select(&facts, FundamentalMetric::EpsDiluted, PeriodType::Annual);
    assert_eq!(eps.len(), 1);
    assert_eq!(eps[0].value, Decimal::new(3125, 1));
    assert_eq!(eps[0].unit, "JPY/shares");
}

#[tokio::test]
async fn submissions_give_the_sic_code_and_every_results_announcement() {
    let url = serve_sec().await;
    let client = EdgarClient::new(EdgarConfig {
        user_agent: USER_AGENT_VALUE.to_owned(),
        data_url: url.clone(),
        www_url: url,
    })
    .unwrap();
    let apple = Symbol::new("AAPL");

    let submissions = client.submissions(&apple, 320193).await.unwrap();
    assert_eq!(submissions.company.sic, Some(3571));
    assert_eq!(
        submissions.company.sic_description.as_deref(),
        Some("Electronic Computers")
    );
    let accepted: Vec<String> = submissions
        .filings
        .releases
        .iter()
        .map(|release| release.accepted_at.to_string())
        .collect();
    assert_eq!(
        accepted,
        ["2026-11-02T21:30:23Z", "2026-07-30T20:30:28Z"],
        "only 8-Ks with Item 2.02, not the 8.01 or 5.02 ones or the 10-Qs"
    );
    assert_eq!(
        submissions.filings.releases[0].report_date,
        Some(date(2026, 11, 2))
    );

    let page = &submissions.older_pages[0];
    assert_eq!(page.filing_to, date(2015, 10, 5));
    let older = client.filings_page(&apple, 320193, page).await.unwrap();
    assert_eq!(
        older
            .releases
            .iter()
            .map(|release| release.filed)
            .collect::<Vec<_>>(),
        [date(2015, 7, 21), date(2015, 4, 27)]
    );
    let ownership: Vec<(&str, &str, String)> = submissions
        .filings
        .ownership
        .iter()
        .chain(&older.ownership)
        .map(|filing| {
            (
                filing.form.as_str(),
                filing.accession.as_str(),
                filing.accepted_at.to_string(),
            )
        })
        .collect();
    assert_eq!(
        ownership,
        [
            (
                "4",
                "0001140361-26-042149",
                "2026-11-05T23:30:13Z".to_owned()
            ),
            (
                "4/A",
                "0001181431-13-036353",
                "2013-06-22T01:45:37Z".to_owned()
            ),
        ],
        "every Form 4 and 4/A with its acceptance time, not its filing date"
    );
}

#[tokio::test]
async fn a_form_4_gives_each_trade_with_its_insider_and_only_for_its_issuer() {
    let url = serve_sec().await;
    let client = EdgarClient::new(EdgarConfig {
        user_agent: USER_AGENT_VALUE.to_owned(),
        data_url: url.clone(),
        www_url: url,
    })
    .unwrap();
    let apple = Symbol::new("AAPL");
    let submissions = client.submissions(&apple, 320193).await.unwrap();
    let listed = &submissions.filings.ownership[0];

    let filing = client.ownership(&apple, 320193, listed).await.unwrap();
    assert_eq!(filing.issuer_cik, 320193);
    assert_eq!(filing.accepted_at, listed.accepted_at);
    assert_eq!(
        filing.owners,
        [InsiderOwner {
            cik: 1780525,
            name: "Newstead Jennifer".to_owned(),
            director: false,
            officer: true,
            ten_percent_owner: false,
            other: false,
            officer_title: Some("SVP, GC and Government Affairs".to_owned()),
        }]
    );
    assert_eq!(
        filing.transactions,
        [InsiderTransaction {
            security: "Common Stock".to_owned(),
            date: date(2026, 11, 3),
            code: "S".to_owned(),
            acquired: false,
            shares: Some(Decimal::from(2399)),
            price: Some(Decimal::new(32575, 2)),
            shares_after: Some(Decimal::from(29997)),
            direct: true,
        }]
    );

    let about_another_issuer =
        normalize_ownership(&Symbol::new("BRK.B"), 1067983, listed, FORM_4.as_bytes()).unwrap();
    assert_eq!(about_another_issuer.issuer_cik, 320193);
    assert!(
        about_another_issuer.transactions.is_empty(),
        "a filing listed under a company that is the reporting owner is not its own insiders' trade"
    );
}

#[tokio::test]
async fn a_missing_daily_index_is_skipped_and_a_refusal_is_a_rate_limit() {
    let url = serve_sec().await;
    let client = |user_agent: &str| {
        EdgarClient::new(EdgarConfig {
            user_agent: user_agent.to_owned(),
            data_url: url.clone(),
            www_url: url.clone(),
        })
        .unwrap()
    };
    let polite = client(USER_AGENT_VALUE);
    let filed = polite.daily_index(date(2026, 11, 9)).await.unwrap();
    assert_eq!(filed.map(|filings| filings.len()), Some(4));
    assert_eq!(polite.daily_index(date(2026, 11, 11)).await.unwrap(), None);
    let refused = client("anonymous").daily_index(date(2026, 11, 9)).await;
    assert!(
        matches!(refused, Err(SourceError::RateLimited { retry_after }) if retry_after == Some(RATE_LIMIT.cooldown)),
        "{refused:?}"
    );
}

#[test]
fn the_daily_index_keeps_periodic_reports_8ks_and_form_4s_only() {
    let filings = normalize_daily_index(include_bytes!("../fixtures/edgar/master_sample.idx"));
    let kept: Vec<(u32, &str, &str, NaiveDate)> = filings
        .iter()
        .map(|filing| {
            (
                filing.cik,
                filing.form.as_str(),
                filing.accession.as_str(),
                filing.filed,
            )
        })
        .collect();
    assert_eq!(
        kept,
        [
            (320193, "8-K", "0000320193-26-000101", date(2026, 11, 9)),
            (320193, "10-K", "0000320193-26-000102", date(2026, 11, 9)),
            (789019, "10-Q/A", "0000950170-26-000500", date(2026, 11, 9)),
            (789019, "4", "0000950170-26-000501", date(2026, 11, 9)),
        ]
    );
}
