use std::collections::HashMap;

use axum::Router;
use axum::extract::Query;
use axum::http::{HeaderMap, StatusCode, header::USER_AGENT};
use axum::routing::get;
use buizz_core::market::{IndexMember, StockIndex, Symbol};
use buizz_sources::http::SourceError;
use buizz_sources::wikipedia::{
    SP500_CHANGES, SP500_CONSTITUENTS, WikipediaClient, WikipediaConfig, normalize_sp500,
};
use chrono::NaiveDate;

const USER_AGENT_VALUE: &str = "buizz-test contact@example.test";
const CONSTITUENTS: &str = include_str!("../fixtures/wikipedia/sp500_constituents.json");
const CHANGES: &str = include_str!("../fixtures/wikipedia/sp500_changes.json");

fn date(text: &str) -> NaiveDate {
    text.parse().unwrap()
}

async fn parse(
    headers: HeaderMap,
    Query(query): Query<HashMap<String, String>>,
) -> (StatusCode, &'static str) {
    if headers
        .get(USER_AGENT)
        .is_none_or(|value| value != USER_AGENT_VALUE)
    {
        return (StatusCode::FORBIDDEN, "set a user agent");
    }
    match (
        query.get("action").map(String::as_str),
        query.get("page").map(String::as_str),
    ) {
        (Some("parse"), Some(SP500_CONSTITUENTS)) => (StatusCode::OK, CONSTITUENTS),
        (Some("parse"), Some(SP500_CHANGES)) => (StatusCode::OK, CHANGES),
        _ => (StatusCode::NOT_FOUND, "{}"),
    }
}

#[tokio::test]
async fn membership_is_rebuilt_backwards_from_todays_constituents_through_the_change_log() {
    let router = Router::new().route("/w/api.php", get(parse));
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let base_url = format!("http://{}", listener.local_addr().unwrap());
    tokio::spawn(async move { axum::serve(listener, router).await.unwrap() });
    let client = WikipediaClient::new(WikipediaConfig {
        user_agent: USER_AGENT_VALUE.to_owned(),
        base_url,
    })
    .unwrap();

    let parsed = client.sp500(date("2026-11-11")).await.unwrap();
    assert_eq!(parsed.unparsed_rows, 0);
    let history = parsed.history;

    let spans = |symbol: &str| -> Vec<(Option<NaiveDate>, Option<NaiveDate>)> {
        history
            .members
            .iter()
            .filter(|member| member.symbol == Symbol::new(symbol))
            .map(|member| (member.start, member.end))
            .collect()
    };
    assert_eq!(
        spans("META"),
        [(Some(date("2013-12-23")), None)],
        "the Facebook addition is matched to today's META through its article"
    );
    assert_eq!(
        spans("TWTR"),
        [(Some(date("2018-06-07")), Some(date("2022-11-01")))]
    );
    assert_eq!(spans("OKTA"), [(Some(date("2026-10-30")), None)]);
    assert_eq!(spans("KVUE"), [(None, Some(date("2026-10-30")))]);
    assert_eq!(
        spans("BRK.B"),
        [(None, None)],
        "a ticker comment is not part of it"
    );
    assert_eq!(
        spans("GOOGL").len() + spans("GOOG").len(),
        2,
        "both share classes stay members"
    );
    assert_eq!(
        spans("CTVA"),
        [(Some(date("2019-06-03")), Some(date("2026-10-06")))]
    );
    let twitter_day = date("2020-01-02");
    assert!(
        history
            .members
            .iter()
            .any(|member: &IndexMember| member.symbol == Symbol::new("TWTR")
                && member.member_on(twitter_day))
    );
    assert_eq!(history.index, StockIndex::Sp500);
    assert_eq!(history.changes, 9);
    assert_eq!(
        history.reliable_from, None,
        "nine changes over fifty years are a thin log"
    );
}

fn wikitext(page: &str) -> String {
    let page: serde_json::Value = serde_json::from_str(page).unwrap();
    page["parse"]["wikitext"].as_str().unwrap().to_owned()
}

#[test]
fn same_day_changes_apply_additions_first_and_unreadable_rows_are_counted() {
    let log = "{| class=\"wikitable\" id=\"changes\"\n\
        |-\n|March 1, 2024\n|\n|\n|MMMX\n|[[3M]]\n|Ticker change.\n\
        |-\n|March 1, 2024\n|MMM\n|[[3M]]\n|\n|\n|Ticker change.\n\
        |-\n|Sometime in 2023\n|AOS\n|[[A. O. Smith]]\n|\n|\n|Undated.\n|}";

    let parsed = normalize_sp500(&wikitext(CONSTITUENTS), log, date("2026-11-11")).unwrap();

    assert_eq!(parsed.unparsed_rows, 1);
    assert!(
        parsed
            .history
            .members
            .iter()
            .any(|member| member.symbol == Symbol::new("MMM")
                && member.member_on(date("2024-02-01"))),
        "3M stays a member across its same-day removal and addition"
    );
}

#[test]
fn a_parse_far_from_five_hundred_members_is_refused() {
    let truncated = wikitext(CONSTITUENTS)
        .split("\n|-")
        .take(300)
        .collect::<Vec<_>>()
        .join("\n|-")
        + "\n|}";

    let Err(SourceError::Decode(message)) =
        normalize_sp500(&truncated, &wikitext(CHANGES), date("2026-11-11"))
    else {
        panic!("a 298-member S&P 500 parsed");
    };
    assert!(message.contains("298 members"), "{message}");
}
