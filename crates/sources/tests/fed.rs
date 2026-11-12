use axum::Router;
use axum::extract::Path;
use axum::routing::get;
use buizz_core::market::FomcMeeting;
use buizz_sources::fed::{FedClient, FedConfig};

const CALENDAR: &str = include_str!("../fixtures/fed/fomccalendars.htm");
const HISTORY_2011: &str = include_str!("../fixtures/fed/fomchistorical2011.htm");
const HISTORY_2020: &str = include_str!("../fixtures/fed/fomchistorical2020.htm");

fn scheduled(date: &str, statement_at: &str) -> FomcMeeting {
    FomcMeeting {
        date: date.parse().unwrap(),
        scheduled: true,
        statement_at: Some(statement_at.parse().unwrap()),
    }
}

fn unscheduled(date: &str) -> FomcMeeting {
    FomcMeeting {
        date: date.parse().unwrap(),
        scheduled: false,
        statement_at: None,
    }
}

#[tokio::test]
async fn scheduled_meetings_carry_their_statement_time_and_unscheduled_moves_their_release_day() {
    let router = Router::new().route(
        "/monetarypolicy/{page}",
        get(|Path(page): Path<String>| async move {
            match page.as_str() {
                "fomccalendars.htm" => CALENDAR,
                "fomchistorical2011.htm" => HISTORY_2011,
                "fomchistorical2020.htm" => HISTORY_2020,
                other => panic!("unexpected {other}"),
            }
        }),
    );
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let base_url = format!("http://{}", listener.local_addr().unwrap());
    tokio::spawn(async move { axum::serve(listener, router).await.unwrap() });
    let client = FedClient::new(FedConfig { base_url }).unwrap();

    assert_eq!(
        client.calendar().await.unwrap(),
        [
            scheduled("2025-01-29", "2025-01-29T19:00:00Z"),
            scheduled("2025-03-19", "2025-03-19T18:00:00Z"),
            scheduled("2025-05-07", "2025-05-07T18:00:00Z"),
            scheduled("2025-06-18", "2025-06-18T18:00:00Z"),
            scheduled("2025-07-30", "2025-07-30T18:00:00Z"),
            scheduled("2025-09-17", "2025-09-17T18:00:00Z"),
            scheduled("2025-10-29", "2025-10-29T18:00:00Z"),
            scheduled("2025-12-10", "2025-12-10T19:00:00Z"),
            scheduled("2027-01-27", "2027-01-27T19:00:00Z"),
            scheduled("2027-03-17", "2027-03-17T18:00:00Z"),
            scheduled("2027-04-28", "2027-04-28T18:00:00Z"),
            scheduled("2027-06-09", "2027-06-09T18:00:00Z"),
            scheduled("2027-07-28", "2027-07-28T18:00:00Z"),
            scheduled("2027-09-15", "2027-09-15T18:00:00Z"),
            scheduled("2027-10-27", "2027-10-27T18:00:00Z"),
            scheduled("2027-12-08", "2027-12-08T19:00:00Z"),
        ]
    );
    assert_eq!(
        client.history(2011).await.unwrap(),
        [
            scheduled("2011-01-26", "2011-01-26T19:15:00Z"),
            scheduled("2011-03-15", "2011-03-15T18:15:00Z"),
            scheduled("2011-04-27", "2011-04-27T16:30:00Z"),
            scheduled("2011-06-22", "2011-06-22T16:30:00Z"),
            scheduled("2011-08-09", "2011-08-09T18:15:00Z"),
            scheduled("2011-09-21", "2011-09-21T18:15:00Z"),
            scheduled("2011-11-02", "2011-11-02T16:30:00Z"),
            scheduled("2011-12-13", "2011-12-13T19:15:00Z"),
        ]
    );
    assert_eq!(
        client.history(2020).await.unwrap(),
        [
            scheduled("2020-01-29", "2020-01-29T19:00:00Z"),
            unscheduled("2020-03-03"),
            unscheduled("2020-03-15"),
            scheduled("2020-03-18", "2020-03-18T18:00:00Z"),
            unscheduled("2020-03-23"),
            scheduled("2020-04-29", "2020-04-29T18:00:00Z"),
            scheduled("2020-06-10", "2020-06-10T18:00:00Z"),
            scheduled("2020-07-29", "2020-07-29T18:00:00Z"),
            scheduled("2020-09-16", "2020-09-16T18:00:00Z"),
            scheduled("2020-11-05", "2020-11-05T19:00:00Z"),
            scheduled("2020-12-16", "2020-12-16T19:00:00Z"),
        ]
    );
}
