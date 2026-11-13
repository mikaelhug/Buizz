use axum::Router;
use axum::http::{HeaderMap, StatusCode, header::USER_AGENT};
use axum::routing::get;
use buizz_core::market::{
    Calendar, FundamentalFact, FundamentalMetric, InsiderOwner, InsiderTransaction, PeriodType,
    Session, Symbol,
};
use buizz_core::time::Timestamp;
use buizz_sources::edgar::{
    EdgarClient, EdgarConfig, RATE_LIMIT, exhibit_text, normalize_company_facts,
    normalize_daily_index, normalize_ownership, split_sections,
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
const FILING_INDEX: &str = include_str!("../fixtures/edgar/filing_index_sample.htm");
const EX_99: &str = include_str!("../fixtures/edgar/ex99_sample.htm");
const DAILY_INDEX_LISTING: &str = r#"{"directory":{"item":[{"last-modified":"11/09/2026 10:02:13 PM","name":"master.20261109.idx","type":"file","href":"master.20261109.idx","size":"1 KB"}],"name":"daily-index/2026/QTR4/","parent-dir":"../"}}"#;

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
            "/Archives/edgar/daily-index/2026/QTR4/index.json",
            get(guarded(DAILY_INDEX_LISTING)),
        )
        .route(
            "/Archives/edgar/daily-index/2026/QTR4/{file}",
            get(|| async { StatusCode::FORBIDDEN }),
        )
        .route(
            "/Archives/edgar/data/320193/000114036126042149/form4.xml",
            get(guarded(FORM_4)),
        )
        .route(
            "/Archives/edgar/data/320193/000032019326000024/0000320193-26-000024-index.htm",
            get(guarded(FILING_INDEX)),
        )
        .route(
            "/Archives/edgar/data/320193/000032019326000024/a8-kex991q4202609262026.htm",
            get(guarded(EX_99)),
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
        1,
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
fn a_filer_with_one_earnings_per_share_tag_and_a_padded_cik_has_diluted_eps_in_its_reporting_currency()
 {
    let payload = r#"{"cik":"0000000002","facts":{"us-gaap":{
        "EarningsPerShareBasicAndDiluted":{"units":{"JPY/shares":[
            {"start":"2023-04-01","end":"2024-03-31","val":312.5,"fy":2023,"fp":"FY","form":"10-K","filed":"2024-06-20","accn":"0000000002-24-000001"}
        ]}}
    }}}"#;
    let facts = normalize_company_facts(
        &Symbol::new("XYZ"),
        2,
        payload.as_bytes(),
        &weekday_calendar(2024, 2024),
    )
    .unwrap();
    let eps = select(&facts, FundamentalMetric::EpsDiluted, PeriodType::Annual);
    assert_eq!(eps.len(), 1);
    assert_eq!(eps[0].value, Decimal::new(3125, 1));
    assert_eq!(eps[0].unit, "JPY/shares");
}

type Filing<'a> = (&'a str, &'a str, &'a str, &'a str);

fn share_count(start: Option<&str>, end: &str, val: &str, filing: Filing) -> String {
    let (form, fp, filed, accn) = filing;
    let start = start.map_or_else(String::new, |start| format!(r#""start":"{start}","#));
    format!(
        r#"{{{start}"end":"{end}","val":{val},"fp":"{fp}","form":"{form}","filed":"{filed}","accn":"{accn}"}}"#
    )
}

fn instant_counts(payload: &str, year: i32) -> Vec<(NaiveDate, &'static str, Decimal)> {
    let facts = normalize_company_facts(
        &Symbol::new("XYZ"),
        3,
        payload.as_bytes(),
        &weekday_calendar(year - 1, year + 1),
    )
    .unwrap();
    let mut counts: Vec<_> = facts
        .into_iter()
        .filter(|fact| {
            fact.metric == Some(FundamentalMetric::SharesOutstanding) && fact.period_start.is_none()
        })
        .map(|fact| {
            let source = if fact.taxonomy == "dei" {
                "cover"
            } else {
                "balance sheet"
            };
            (fact.period_end, source, fact.value)
        })
        .collect();
    counts.sort();
    counts
}

#[test]
fn a_share_count_tagged_a_thousand_or_a_million_times_off_takes_the_scale_the_earlier_filings_agree_on()
 {
    let earlier = ("10-Q", "Q3", "2015-10-28", "0000000003-15-000003");
    let annual = ("10-K", "FY", "2016-02-17", "0000000003-16-000001");
    let q1 = ("10-Q", "Q1", "2016-04-27", "0000000003-16-000002");
    let q2 = ("10-Q", "Q2", "2016-07-27", "0000000003-16-000003");
    let q3 = ("10-Q", "Q3", "2016-10-26", "0000000003-16-000004");
    let payload = format!(
        r#"{{"cik":3,"facts":{{"dei":{{"EntityCommonStockSharesOutstanding":{{"units":{{"shares":[{},{},{},{}]}}}}}},
        "us-gaap":{{"CommonStockSharesOutstanding":{{"units":{{"shares":[{},{},{},{},{}]}}}}}}}}}}"#,
        share_count(None, "2015-10-20", "190277418", earlier),
        share_count(None, "2016-02-12", "208077418", annual),
        share_count(None, "2016-04-25", "208077418000", q1),
        share_count(None, "2016-07-25", "208077418000", q2),
        share_count(None, "2015-09-26", "190342000", earlier),
        share_count(None, "2015-12-26", "189722", annual),
        share_count(None, "2016-03-26", "189193000", q1),
        share_count(None, "2016-06-25", "188877000000000", q2),
        share_count(None, "2016-09-24", "188446000", q3),
    );
    let whole = |value: i64| Decimal::from(value);
    assert_eq!(
        instant_counts(&payload, 2016),
        vec![
            (date(2015, 9, 26), "balance sheet", whole(190_342_000)),
            (date(2015, 12, 26), "balance sheet", whole(189_722_000)),
            (date(2016, 3, 26), "balance sheet", whole(189_193_000)),
            (date(2016, 6, 25), "balance sheet", whole(188_877_000)),
            (date(2016, 9, 24), "balance sheet", whole(188_446_000)),
        ]
    );
}

#[test]
fn a_share_count_is_rescaled_only_from_filings_made_by_the_time_it_was_filed() {
    let first = ("10-Q", "Q1", "2015-04-29", "0000000003-15-000001");
    let annual = ("10-K", "FY", "2016-02-18", "0000000003-16-000001");
    let payload = format!(
        r#"{{"cik":3,"facts":{{"dei":{{"EntityCommonStockSharesOutstanding":{{"units":{{"shares":[{}]}}}}}},
        "us-gaap":{{"CommonStockSharesOutstanding":{{"units":{{"shares":[{},{}]}}}},
        "WeightedAverageNumberOfSharesOutstandingBasic":{{"units":{{"shares":[{},{}]}}}}}}}}}}"#,
        share_count(None, "2016-02-12", "492830471", annual),
        share_count(None, "2015-03-31", "479490832000000", first),
        share_count(None, "2015-12-31", "492025443", annual),
        share_count(Some("2015-01-01"), "2015-03-31", "479000000000000", first),
        share_count(Some("2015-01-01"), "2015-12-31", "484000000", annual),
    );
    assert_eq!(
        instant_counts(&payload, 2015),
        vec![
            (
                date(2015, 3, 31),
                "balance sheet",
                Decimal::from(479_490_832_000_000_i64)
            ),
            (
                date(2015, 12, 31),
                "balance sheet",
                Decimal::from(492_025_443)
            ),
        ]
    );
}

#[test]
fn a_filer_that_tags_only_weighted_averages_keeps_its_cover_page_share_counts() {
    let q1 = ("10-Q", "Q1", "2025-05-01", "0000000003-25-000001");
    let q2 = ("10-Q", "Q2", "2025-08-07", "0000000003-25-000002");
    let payload = format!(
        r#"{{"cik":3,"facts":{{"dei":{{"EntityCommonStockSharesOutstanding":{{"units":{{"shares":[{},{}]}}}}}},
        "us-gaap":{{"WeightedAverageNumberOfSharesOutstandingBasic":{{"units":{{"shares":[{},{}]}}}}}}}}}}"#,
        share_count(None, "2025-04-25", "896000000", q1),
        share_count(None, "2025-07-25", "897000000", q2),
        share_count(Some("2025-01-01"), "2025-03-31", "898000000", q1),
        share_count(Some("2025-04-01"), "2025-06-30", "896500000", q2),
    );
    assert_eq!(
        instant_counts(&payload, 2025),
        vec![
            (date(2025, 4, 25), "cover", Decimal::from(896_000_000)),
            (date(2025, 7, 25), "cover", Decimal::from(897_000_000)),
        ]
    );
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

    let reports: Vec<(&str, &str, Vec<String>)> = submissions
        .filings
        .reports
        .iter()
        .map(|report| {
            (
                report.accession.as_str(),
                report.form.as_str(),
                report.items.clone(),
            )
        })
        .collect();
    assert_eq!(
        reports,
        [
            (
                "0001140361-26-042798",
                "8-K",
                vec!["8.01".to_owned(), "9.01".to_owned()]
            ),
            (
                "0000320193-26-000024",
                "8-K",
                vec!["2.02".to_owned(), "9.01".to_owned()]
            ),
            ("0001140361-26-035325", "8-K/A", vec!["5.02".to_owned()]),
            (
                "0000320193-26-000018",
                "8-K",
                vec!["2.02".to_owned(), "9.01".to_owned()]
            ),
        ],
        "every 8-K and 8-K/A with its items"
    );
    assert!(
        submissions
            .filings
            .reports
            .iter()
            .all(|report| report.available_at == report.accepted_at)
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

    let report = client.ownership(320193, listed).await.unwrap();
    let filing = listed.dated(&apple, 320193, report);
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

    assert_eq!(filing.plan_10b5_1, Some(true));

    let about_another_issuer = listed.dated(
        &Symbol::new("BRK.B"),
        1067983,
        normalize_ownership(&listed.accession, FORM_4.as_bytes()).unwrap(),
    );
    assert_eq!(about_another_issuer.issuer_cik, 320193);
    assert!(
        about_another_issuer.transactions.is_empty(),
        "a filing listed under a company that is the reporting owner is not its own insiders' trade"
    );
}

#[tokio::test]
async fn a_daily_index_missing_from_its_quarter_listing_is_skipped_and_a_refusal_is_a_rate_limit() {
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

#[tokio::test]
async fn an_earnings_release_is_the_text_of_the_first_press_release_exhibit_in_its_filing_index() {
    let url = serve_sec().await;
    let client = EdgarClient::new(EdgarConfig {
        user_agent: USER_AGENT_VALUE.to_owned(),
        data_url: url.clone(),
        www_url: url,
    })
    .unwrap();
    let accepted_at: Timestamp = "2026-11-02T21:30:23Z".parse().unwrap();

    let release = client
        .press_release(320193, "0000320193-26-000024", accepted_at)
        .await
        .unwrap();
    assert_eq!(release.accepted_at, accepted_at);
    let exhibit = release.exhibit.expect("an EX-99.1 exhibit");
    assert_eq!(exhibit.document, "a8-kex991q4202609262026.htm");
    assert_eq!(exhibit.kind, "EX-99.1");
    let lines: Vec<&str> = exhibit.text.lines().collect();
    assert_eq!(
        lines[..4],
        [
            "Exhibit 99.1",
            "",
            "Apple reports fourth quarter results",
            ""
        ],
        "the SGML envelope EDGAR serves around the document is not part of it"
    );
    assert!(lines.contains(&"Services revenue reaches new all-time high"));
    assert!(
        lines[8].starts_with("CUPERTINO, CALIFORNIA — Apple® today announced"),
        "{lines:#?}"
    );
    assert_eq!(
        lines[lines.len() - 3..],
        [
            "Three Months Ended\tTwelve Months Ended",
            "Net sales:",
            "Products\t$\t82,904\t$\t73,716\t$\t355,533\t$\t307,003",
        ],
        "a table keeps one line per row with its non-empty cells tab-separated"
    );
}

fn current_report_items(
    form: &str,
    document: &str,
    items: &str,
    filed: &str,
) -> Result<Vec<(String, String)>, String> {
    let text = exhibit_text(document, filed.as_bytes());
    let listed: Vec<String> = items.split(',').map(str::to_owned).collect();
    let chars: Vec<char> = text.chars().collect();
    Ok(split_sections(form, &listed, &text)?
        .into_iter()
        .map(|section| {
            let body: String = chars[section.start as usize..section.end as usize]
                .iter()
                .collect();
            (section.key, body.trim().to_owned())
        })
        .collect())
}

#[test]
fn an_8k_is_cut_into_the_items_its_filing_lists() {
    let heading_blocks = r#"<html><body>
<div style="text-align:center"><b>UNITED STATES SECURITIES AND EXCHANGE COMMISSION</b></div>
<div>FORM 8-K</div>
<table><tr><td>&#9744;</td><td>Written communications pursuant to Rule 425 under the Securities Act</td></tr></table>
<div><span style="font-weight:bold">Item 1.01</span><span style="font-weight:bold"> Entry into a Material Definitive Agreement.</span></div>
<div>On May 4, 2023, the Company entered into a credit agreement. The information set forth under Item 2.03 of this Current Report is incorporated herein by reference.</div>
<div><span style="font-weight:bold">Item 2.03 Creation of a Direct Financial Obligation.</span></div>
<div>The credit agreement provides for a $500 million revolving facility.</div>
<div><span style="font-weight:bold">Item 9.01 Financial Statements and Exhibits.</span></div>
<table><tr><td>Exhibit No.</td><td>Description</td></tr><tr><td>10.1</td><td>Credit Agreement</td></tr></table>
<div style="text-align:center"><b>SIGNATURES</b></div>
<div>Pursuant to the requirements of the Securities Exchange Act of 1934, the registrant has duly caused this report to be signed.</div>
</body></html>"#;
    assert_eq!(
        current_report_items("8-K", "a8-k.htm", "1.01,2.03,9.01", heading_blocks),
        Ok(vec![
            (
                "8-K:1.01".to_owned(),
                "Item 1.01 Entry into a Material Definitive Agreement.\nOn May 4, 2023, the Company entered into a credit agreement. The information set forth under Item 2.03 of this Current Report is incorporated herein by reference.".to_owned()
            ),
            (
                "8-K:2.03".to_owned(),
                "Item 2.03 Creation of a Direct Financial Obligation.\nThe credit agreement provides for a $500 million revolving facility.".to_owned()
            ),
            (
                "8-K:9.01".to_owned(),
                "Item 9.01 Financial Statements and Exhibits.\nExhibit No.\tDescription\n10.1\tCredit Agreement".to_owned()
            ),
        ]),
        "headings in their own blocks, a cross-reference inside a sentence is not one, and the signatures are left out"
    );

    let bold_paragraphs = r#"<html><body>
<p><b>Item 5.02 Departure of Directors or Certain Officers.</b> On March 3, 2015, Jane Roe notified the Board that she will retire as Chief Financial Officer.</p>
<p><b>Item 8.01</b> <b>Other Events.</b> The Board declared a quarterly dividend.</p>
<p>SIGNATURE</p>
</body></html>"#;
    assert_eq!(
        current_report_items("8-K/A", "form8ka.htm", "5.02", bold_paragraphs),
        Ok(vec![(
            "8-K:5.02".to_owned(),
            "Item 5.02 Departure of Directors or Certain Officers. On March 3, 2015, Jane Roe notified the Board that she will retire as Chief Financial Officer.".to_owned()
        )]),
        "a heading in bold at the start of a paragraph, and a heading of an item the filing does not list still ends the one before"
    );

    let table_rows = r#"<html><body>
<table><tr><td><b>Item 2.02.</b></td><td><b>Results of Operations and Financial Condition.</b></td></tr></table>
<p>On July 25, 2019, the Company issued a press release announcing its results.</p>
<table><tr><td><b>Item 9.01.</b></td><td><b>Financial Statements and Exhibits.</b></td></tr></table>
<p>99.1 Press release dated July 25, 2019.</p>
</body></html>"#;
    assert_eq!(
        current_report_items("8-K", "d8k.htm", "2.02,9.01", table_rows),
        Ok(vec![
            (
                "8-K:2.02".to_owned(),
                "Item 2.02.\tResults of Operations and Financial Condition.\nOn July 25, 2019, the Company issued a press release announcing its results.".to_owned()
            ),
            (
                "8-K:9.01".to_owned(),
                "Item 9.01.\tFinancial Statements and Exhibits.\n99.1 Press release dated July 25, 2019.".to_owned()
            ),
        ]),
        "a heading set as a table row"
    );

    let plain_2005 = "<DOCUMENT>\n<TYPE>8-K\n<TEXT>\n                       SECURITIES AND EXCHANGE COMMISSION\n\n\
SECTION 2 - FINANCIAL INFORMATION\n\n\
ITEM 2.02.  RESULTS OF OPERATIONS AND FINANCIAL CONDITION.\n\n\
     On February 10, 2005, the Registrant issued a press release\n\
announcing its fourth quarter results.\n\n\
ITEMS 7.01 AND 9.01  REGULATION FD DISCLOSURE; FINANCIAL STATEMENTS AND EXHIBITS.\n\n\
     (c) Exhibits. 99.1 Press release.\n\n\
                                   SIGNATURE\n\n\
     Pursuant to the requirements of the Securities Exchange Act of 1934\n</TEXT>\n</DOCUMENT>\n";
    assert_eq!(
        current_report_items("8-K", "form8k.txt", "2.02,7.01,9.01", plain_2005),
        Ok(vec![
            (
                "8-K:2.02".to_owned(),
                "ITEM 2.02.  RESULTS OF OPERATIONS AND FINANCIAL CONDITION.\n\nOn February 10, 2005, the Registrant issued a press release\nannouncing its fourth quarter results.".to_owned()
            ),
            (
                "8-K:7.01".to_owned(),
                "ITEMS 7.01 AND 9.01  REGULATION FD DISCLOSURE; FINANCIAL STATEMENTS AND EXHIBITS.\n\n(c) Exhibits. 99.1 Press release.".to_owned()
            ),
            (
                "8-K:9.01".to_owned(),
                "ITEMS 7.01 AND 9.01  REGULATION FD DISCLOSURE; FINANCIAL STATEMENTS AND EXHIBITS.\n\n(c) Exhibits. 99.1 Press release.".to_owned()
            ),
        ]),
        "a 2005 plain-text filing in capitals, where a combined heading gives each of its items the same span"
    );

    assert_eq!(
        current_report_items(
            "8-K",
            "form8k.htm",
            "5.07",
            "<p>The annual meeting was held on May 1, 2012.</p>"
        ),
        Err("no heading for items 5.07 (found none)".to_owned()),
        "an 8-K whose listed items cannot be found says so"
    );
}
