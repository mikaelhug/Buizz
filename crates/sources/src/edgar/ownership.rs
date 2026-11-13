use buizz_core::market::{InsiderFiling, InsiderOwner, InsiderReport, InsiderTransaction, Symbol};
use chrono::NaiveDate;
use roxmltree::{Document, Node};
use rust_decimal::Decimal;

use super::filings::OwnershipFiling;
use crate::http::SourceError;

impl OwnershipFiling {
    pub fn dated(&self, symbol: &Symbol, cik: u32, report: InsiderReport) -> InsiderFiling {
        let InsiderReport {
            issuer_cik,
            plan_10b5_1,
            owners,
            transactions,
            ..
        } = report;
        InsiderFiling {
            symbol: symbol.clone(),
            issuer_cik,
            accession: self.accession.clone(),
            form: self.form.clone(),
            filed: self.filed,
            accepted_at: self.accepted_at,
            plan_10b5_1,
            owners,
            transactions: if issuer_cik == cik {
                transactions
            } else {
                Vec::new()
            },
        }
    }
}

pub fn normalize_ownership(accession: &str, payload: &[u8]) -> Result<InsiderReport, SourceError> {
    let undecodable = |error: &dyn std::fmt::Display| {
        SourceError::Decode(format!("ownership document {accession}: {error}"))
    };
    let text = std::str::from_utf8(payload).map_err(|error| undecodable(&error))?;
    let document = Document::parse(text).map_err(|error| undecodable(&error))?;
    let root = document.root_element();
    let issuer_cik = child(root, "issuer")
        .and_then(|issuer| text_of(issuer, "issuerCik"))
        .and_then(|cik| cik.parse::<u32>().ok())
        .ok_or_else(|| undecodable(&"no issuer CIK"))?;
    Ok(InsiderReport {
        accession: accession.to_owned(),
        issuer_cik,
        plan_10b5_1: text_of(root, "aff10b5One").map(|text| truthy(&text)),
        owners: root
            .children()
            .filter(|node| node.has_tag_name("reportingOwner"))
            .filter_map(owner)
            .collect(),
        transactions: root
            .descendants()
            .filter(|node| node.has_tag_name("nonDerivativeTransaction"))
            .filter_map(transaction)
            .collect(),
    })
}

fn child<'a, 'input>(node: Node<'a, 'input>, name: &str) -> Option<Node<'a, 'input>> {
    node.children().find(|child| child.has_tag_name(name))
}

fn text_of(node: Node<'_, '_>, name: &str) -> Option<String> {
    let text = child(node, name)?.text()?.trim();
    (!text.is_empty()).then(|| text.to_owned())
}

fn value_at(node: Node<'_, '_>, path: &[&str]) -> Option<String> {
    let found = path
        .iter()
        .try_fold(node, |parent, name| child(parent, name))?;
    text_of(found, "value")
}

pub(super) fn truthy(text: &str) -> bool {
    text == "1" || text.eq_ignore_ascii_case("true")
}

fn flag(node: Node<'_, '_>, name: &str) -> bool {
    text_of(node, name).is_some_and(|text| truthy(&text))
}

pub(super) fn amount(text: Option<String>) -> Option<Decimal> {
    text?.replace(',', "").parse().ok()
}

fn owner(node: Node<'_, '_>) -> Option<InsiderOwner> {
    let id = child(node, "reportingOwnerId")?;
    let relationship = child(node, "reportingOwnerRelationship");
    let holds = |name| relationship.is_some_and(|relationship| flag(relationship, name));
    Some(InsiderOwner {
        cik: text_of(id, "rptOwnerCik")?.parse().ok()?,
        name: text_of(id, "rptOwnerName").unwrap_or_default(),
        director: holds("isDirector"),
        officer: holds("isOfficer"),
        ten_percent_owner: holds("isTenPercentOwner"),
        other: holds("isOther"),
        officer_title: relationship.and_then(|relationship| text_of(relationship, "officerTitle")),
    })
}

fn transaction(node: Node<'_, '_>) -> Option<InsiderTransaction> {
    let date = value_at(node, &["transactionDate"])?;
    Some(InsiderTransaction {
        security: value_at(node, &["securityTitle"]).unwrap_or_default(),
        date: NaiveDate::parse_from_str(date.get(..10)?, "%Y-%m-%d").ok()?,
        code: text_of(child(node, "transactionCoding")?, "transactionCode")?,
        acquired: value_at(
            node,
            &["transactionAmounts", "transactionAcquiredDisposedCode"],
        )? == "A",
        shares: amount(value_at(node, &["transactionAmounts", "transactionShares"])),
        price: amount(value_at(
            node,
            &["transactionAmounts", "transactionPricePerShare"],
        )),
        shares_after: amount(value_at(
            node,
            &["postTransactionAmounts", "sharesOwnedFollowingTransaction"],
        )),
        direct: value_at(node, &["ownershipNature", "directOrIndirectOwnership"])
            .is_none_or(|nature| nature == "D"),
    })
}
