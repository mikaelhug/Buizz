use buizz_core::market::EarningsInsight;
use buizz_sources::factset::{Issue, parse, read_pdf};

#[test]
#[ignore]
fn scratch() {
    let dir = std::env::var("SCRATCH_PDFS").unwrap();
    let mut paths: Vec<_> = std::fs::read_dir(&dir)
        .unwrap()
        .map(|entry| entry.unwrap().path())
        .filter(|path| {
            path.extension()
                .is_some_and(|ext| ext == "pdf" || ext == "txt")
        })
        .collect();
    paths.sort();
    for path in paths {
        let name = path.file_stem().unwrap().to_string_lossy().to_string();
        let date = &name[..10];
        let issue = Issue {
            report: name.clone(),
            issue_date: date.parse().unwrap(),
            url: String::new(),
        };
        let (text, stamped) = if path.extension().unwrap() == "pdf" {
            read_pdf(&std::fs::read(&path).unwrap()).unwrap()
        } else {
            (std::fs::read_to_string(&path).unwrap(), Vec::new())
        };
        let report = EarningsInsight {
            report: issue.report,
            issue_date: issue.issue_date,
            published: issue.issue_date,
            url: String::new(),
            retrieved_from: String::new(),
            digest: String::new(),
            text,
        };
        println!("== {name} stamped {stamped:?}");
        for fact in parse(&report) {
            println!(
                "{name} {:?} {:?} {} {:?} {}",
                fact.metric,
                fact.scope,
                fact.period.map(|p| p.to_string()).unwrap_or_default(),
                fact.as_of,
                fact.value
            );
        }
    }
}
