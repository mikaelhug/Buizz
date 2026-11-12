use std::collections::BTreeMap;

use buizz_core::market::{EarningsInsight, EstimateMetric, EstimatePeriod, EstimateScope};
use buizz_sources::factset::{Issue, parse, read_pdf};
use chrono::NaiveDate;
use rust_decimal::Decimal;

type Key = (EstimateMetric, EstimateScope, Option<EstimatePeriod>);

fn date(text: &str) -> NaiveDate {
    text.parse().unwrap()
}

async fn facts(file: &str, url: &str) -> BTreeMap<Key, (Decimal, Option<NaiveDate>)> {
    let path = format!("{}/fixtures/factset/{file}", env!("CARGO_MANIFEST_DIR"));
    let (text, stamped) = read_pdf(&std::fs::read(path).unwrap()).await.unwrap();
    let issue = Issue::from_url(url).unwrap();
    assert!(
        stamped.iter().all(|day| *day == issue.issue_date),
        "{stamped:?}"
    );
    let report = EarningsInsight {
        report: issue.report,
        issue_date: issue.issue_date,
        published: issue.issue_date,
        url: issue.url,
        retrieved_from: String::new(),
        digest: String::new(),
        text,
    };
    parse(&report)
        .into_iter()
        .map(|fact| {
            (
                (fact.metric, fact.scope, fact.period),
                (fact.value, fact.as_of),
            )
        })
        .collect()
}

#[tokio::test]
async fn each_era_of_the_report_yields_the_figures_it_states_and_nothing_it_does_not() {
    use EstimateMetric::*;
    use EstimateScope::*;
    let quarter = |year, quarter| Some(EstimatePeriod::Quarter(year, quarter));
    let index = |metric| (metric, Sp500, None);
    let stated = |value: &str| (value.parse::<Decimal>().unwrap(), None);
    let dated = |value: &str, day: &str| (value.parse::<Decimal>().unwrap(), Some(date(day)));

    let tables = facts(
        "earningsinsight_10.7.11.pdf",
        "http://www.factset.com/websitefiles/PDFs/earningsinsight/earningsinsight_10.7.11",
    )
    .await;
    assert_eq!(tables[&index(ForwardPe)], stated("10.9"));
    assert_eq!(tables[&index(ForwardEps)], stated("107.12"));
    assert_eq!(tables[&index(IndexPrice)], stated("1164.97"));
    assert_eq!(tables[&index(ForwardPe10yAvg)], stated("15.0"));
    assert_eq!(tables[&(ForwardPe, TelecomServices, None)], stated("14.5"));
    assert_eq!(
        tables[&(ForwardPe, InformationTechnology, None)],
        stated("11.6")
    );
    assert_eq!(
        tables[&(NegativeGuidance, Sp500, quarter(2011, 3))],
        stated("71")
    );
    assert_eq!(tables[&(EpsBeatPct, Sp500, quarter(2011, 3))], stated("75"));
    assert!(!tables.contains_key(&index(TrailingPe)));
    assert!(!tables.contains_key(&index(TargetPrice)));

    let wednesday = facts(
        "earningsinsight_2.1.13.pdf",
        "http://www.factset.com:80/websitefiles/PDFs/earningsinsight/earningsinsight_2.1.13",
    )
    .await;
    assert_eq!(wednesday[&index(ForwardPe)], dated("13.3", "2013-01-31"));
    assert_eq!(wednesday[&index(ForwardEps)], dated("112.79", "2013-01-31"));
    assert_eq!(
        wednesday[&index(IndexPrice)],
        dated("1498.11", "2013-01-31")
    );
    assert_eq!(wednesday[&index(ForwardPe5yAvg)], stated("12.8"));
    assert_eq!(
        wednesday[&(ForwardPe, TelecomServices, None)],
        stated("16.6")
    );
    assert_eq!(
        wednesday[&(RevenueGrowth, Sp500, quarter(2012, 4))],
        stated("2.3")
    );
    assert!(
        !wednesday.contains_key(&index(ForwardPe10yAvg)),
        "this issue quotes only the 5-year average"
    );

    let modern = facts(
        "EarningsInsight_112224.pdf",
        "https://advantage.factset.com/hubfs/Website/Resources%20Section/Research%20Desk/Earnings%20Insight/EarningsInsight_112224.pdf",
    ).await;
    for (key, value) in [
        (index(ForwardPe), "22.0"),
        (index(ForwardPe5yAvg), "19.6"),
        (index(ForwardPe10yAvg), "18.1"),
        (index(TrailingPe), "28.1"),
        (index(TargetPrice), "6609.91"),
        (index(TargetUpsidePct), "11.1"),
        (index(IndexPrice), "5948.71"),
        (index(BuyPct), "53.7"),
        (index(Ratings), "12045"),
        ((ForwardPe, InformationTechnology, None), "29.2"),
        ((EpsGrowth, Sp500, quarter(2024, 3)), "5.8"),
        ((EpsGrowth, Sp500, Some(EstimatePeriod::Year(2025))), "15.0"),
        ((RevenueBeatPct, Sp500, quarter(2024, 3)), "61"),
        ((PositiveGuidance, Sp500, quarter(2024, 4)), "33"),
    ] {
        assert_eq!(modern[&key], stated(value), "{key:?}");
    }
    assert!(
        !modern.contains_key(&index(ForwardEps)),
        "this issue states no forward EPS, and it is never derived from price and P/E"
    );
}

#[test]
fn report_names_follow_the_url_patterns_factset_used_since_2011() {
    let named = |url: &str| Issue::from_url(url).map(|issue| (issue.report, issue.issue_date));
    assert_eq!(
        named(
            "http://www.factset.com:80/websitefiles/PDFs/earningsinsight/earningsinsight_10.7.11/"
        ),
        Some(("2011-10-07".to_owned(), date("2011-10-07")))
    );
    assert_eq!(
        named(
            "https://go.factset.com/hubfs/Resources%20Section/Research%20Desk/Earnings%20Insight/EarningsInsight_092818B.pdf?hsCtaTracking=1"
        ),
        Some(("2018-09-28B".to_owned(), date("2018-09-28")))
    );
    assert_eq!(
        named(
            "https://insight.factset.com/hubfs/Resources%20Section/Research%20Desk/Earnings%20Insight/EarningsInsight2_010518.pdf"
        ),
        Some(("2018-01-05v2".to_owned(), date("2018-01-05")))
    );
    assert_eq!(
        named(
            "https://www.factset.com/websitefiles/PDFs/earningsinsight/earnings-insight-home-link"
        ),
        None
    );
}
