use buizz_core::market::FilingDocument;

use super::exhibit::{exhibit_text, is_text_document};
use crate::http::SourceError;

const SKIPPED_KINDS: [&str; 4] = ["GRAPHIC", "ZIP", "EX-101", "JSON"];
const GENERATED_BY_EDGAR: &str = "IDEA:";
const WRAPPERS: [&str; 3] = ["XBRL", "XML", "PDF"];
const WRAPPED_LINE: usize = 1023;
const DOCUMENT: &[u8] = b"<DOCUMENT>";
const DOCUMENT_END: &[u8] = b"</DOCUMENT>";
const TEXT: &[u8] = b"<TEXT>";
const TEXT_END: &[u8] = b"</TEXT>";

pub fn submission_documents(
    accession: &str,
    submission: &[u8],
) -> Result<Vec<FilingDocument>, SourceError> {
    let mut rest = submission;
    let mut blocks = Vec::new();
    while let Some(start) = find(rest, DOCUMENT) {
        rest = &rest[start + DOCUMENT.len()..];
        let (block, after) = rest.split_at(find(rest, DOCUMENT_END).unwrap_or(rest.len()));
        blocks.push(block);
        rest = after;
    }
    if blocks.is_empty() {
        return Err(SourceError::Decode(format!(
            "complete submission {accession} has no documents"
        )));
    }
    Ok(blocks
        .into_iter()
        .filter_map(|block| filed_document(accession, block))
        .collect())
}

fn filed_document(accession: &str, block: &[u8]) -> Option<FilingDocument> {
    let text_start = find(block, TEXT)?;
    let header = String::from_utf8_lossy(&block[..text_start]);
    let field = |tag: &str| {
        header.lines().find_map(|line| {
            let value = line.trim().strip_prefix(&format!("<{tag}>"))?.trim();
            (!value.is_empty()).then(|| value.to_owned())
        })
    };
    let kind = field("TYPE")?;
    let sequence: u32 = field("SEQUENCE")?.parse().ok()?;
    let description = field("DESCRIPTION");
    let generated = description
        .as_deref()
        .is_some_and(|description| description.starts_with(GENERATED_BY_EDGAR));
    let skipped = SKIPPED_KINDS
        .iter()
        .any(|skipped| kind.to_ascii_uppercase().starts_with(skipped));
    if generated || skipped {
        return None;
    }
    let document = field("FILENAME").unwrap_or_else(|| format!("{accession}-{sequence}.txt"));
    let body = &block[text_start + TEXT.len()..];
    let body = &body[..rfind(body, TEXT_END).unwrap_or(body.len())];
    let body = unwrapped(body.strip_prefix(b"\n").unwrap_or(body));
    let raw = uudecoded(body).unwrap_or_else(|| rejoined(body));
    let text = is_text_document(&document)
        .then(|| exhibit_text(&document, &raw))
        .filter(|text| !text.is_empty());
    Some(FilingDocument {
        sequence,
        document,
        kind,
        description,
        raw,
        text,
    })
}

fn unwrapped(body: &[u8]) -> &[u8] {
    WRAPPERS
        .iter()
        .find_map(|tag| {
            let inner = body.strip_prefix(format!("<{tag}>").as_bytes())?;
            let inner = inner.strip_prefix(b"\n").unwrap_or(inner);
            let end = rfind(inner, format!("</{tag}>").as_bytes())?;
            Some(&inner[..end])
        })
        .unwrap_or(body)
}

fn uudecoded(body: &[u8]) -> Option<Vec<u8>> {
    let mut lines = body
        .split(|byte| *byte == b'\n')
        .map(|line| line.strip_suffix(b"\r").unwrap_or(line));
    let mode = lines.next()?.strip_prefix(b"begin ")?;
    if !matches!(mode, [b'0'..=b'7', b'0'..=b'7', b'0'..=b'7', b' ', ..]) {
        return None;
    }
    let mut bytes = Vec::new();
    for line in lines.take_while(|line| *line != b"end") {
        let Some((&length, encoded)) = line.split_first() else {
            continue;
        };
        let length = usize::from(sextet(length));
        let mut sextets: Vec<u8> = encoded.iter().copied().map(sextet).collect();
        sextets.resize(length.div_ceil(3) * 4, 0);
        let decoded = sextets.chunks(4).flat_map(|quad| {
            [
                quad[0] << 2 | quad[1] >> 4,
                quad[1] << 4 | quad[2] >> 2,
                quad[2] << 6 | quad[3],
            ]
        });
        bytes.extend(decoded.take(length));
    }
    Some(bytes)
}

fn sextet(letter: u8) -> u8 {
    letter.wrapping_sub(b' ') & 0x3f
}

fn rejoined(body: &[u8]) -> Vec<u8> {
    let mut joined = Vec::with_capacity(body.len());
    for line in body.split_inclusive(|byte| *byte == b'\n') {
        let wrapped = line
            .strip_suffix(b"\n")
            .filter(|content| content.len() == WRAPPED_LINE);
        joined.extend_from_slice(wrapped.unwrap_or(line));
    }
    joined
}

fn find(haystack: &[u8], needle: &[u8]) -> Option<usize> {
    haystack
        .windows(needle.len())
        .position(|window| window == needle)
}

fn rfind(haystack: &[u8], needle: &[u8]) -> Option<usize> {
    haystack
        .windows(needle.len())
        .rposition(|window| window == needle)
}
