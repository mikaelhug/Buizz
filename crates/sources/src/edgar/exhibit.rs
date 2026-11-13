use scraper::{ElementRef, Html, Node, Selector};

const PRESS_RELEASE: &str = "EX-99";
const SKIPPED: [&str; 6] = ["head", "script", "style", "title", "noscript", "template"];
const BLOCKS: [&str; 26] = [
    "address",
    "article",
    "aside",
    "blockquote",
    "body",
    "caption",
    "center",
    "dd",
    "div",
    "dl",
    "dt",
    "footer",
    "h1",
    "h2",
    "h3",
    "h4",
    "h5",
    "h6",
    "header",
    "hr",
    "li",
    "ol",
    "pre",
    "section",
    "table",
    "ul",
];
const WINDOWS_1252: [char; 32] = [
    '€', '\u{81}', '‚', 'ƒ', '„', '…', '†', '‡', 'ˆ', '‰', 'Š', '‹', 'Œ', '\u{8d}', 'Ž', '\u{8f}',
    '\u{90}', '‘', '’', '“', '”', '•', '–', '—', '˜', '™', 'š', '›', 'œ', '\u{9d}', 'ž', 'Ÿ',
];

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct IndexedDocument {
    pub sequence: u32,
    pub description: Option<String>,
    pub document: String,
    pub kind: String,
}

pub fn indexed_documents(index: &str) -> Vec<IndexedDocument> {
    let html = Html::parse_document(index);
    let (Ok(rows), Ok(cells)) = (
        Selector::parse(r#"table[summary="Document Format Files"] tr"#),
        Selector::parse("td"),
    ) else {
        return Vec::new();
    };
    html.select(&rows)
        .filter_map(|row| {
            let cells: Vec<String> = row
                .select(&cells)
                .map(|cell| cell.text().collect::<String>().trim().to_owned())
                .collect();
            let document = cells.get(2)?.split_whitespace().next()?.to_owned();
            let kind = cells.get(3)?.clone();
            Some(IndexedDocument {
                sequence: cells.first()?.parse().ok()?,
                description: cells.get(1).filter(|text| !text.is_empty()).cloned(),
                document,
                kind,
            })
        })
        .collect()
}

pub fn press_release_document(index: &str) -> Option<IndexedDocument> {
    indexed_documents(index)
        .into_iter()
        .find(|listed| listed.kind.to_ascii_uppercase().starts_with(PRESS_RELEASE))
}

pub fn is_text_document(document: &str) -> bool {
    let lowered = document.to_ascii_lowercase();
    [".htm", ".html", ".txt"]
        .iter()
        .any(|suffix| lowered.ends_with(suffix))
}

pub fn exhibit_text(document: &str, payload: &[u8]) -> String {
    let decoded = decode(payload);
    let body = submission_text(&decoded);
    let lowered = document.to_ascii_lowercase();
    if lowered.ends_with(".htm") || lowered.ends_with(".html") {
        html_text(body)
    } else {
        plain_text(body)
    }
}

fn decode(payload: &[u8]) -> String {
    String::from_utf8(payload.to_vec()).unwrap_or_else(|_| {
        payload
            .iter()
            .map(|byte| match byte {
                0x80..=0x9f => WINDOWS_1252[usize::from(byte - 0x80)],
                _ => char::from(*byte),
            })
            .collect()
    })
}

fn submission_text(document: &str) -> &str {
    if !document.trim_start().starts_with("<DOCUMENT>") {
        return document;
    }
    let start = document
        .find("<TEXT>")
        .map_or(0, |index| index + "<TEXT>".len());
    let end = document[start..]
        .rfind("</TEXT>")
        .map_or(document.len(), |index| start + index);
    &document[start..end]
}

fn plain_text(text: &str) -> String {
    let lines: Vec<&str> = text.lines().map(str::trim_end).collect();
    tidy(lines.iter().map(|line| (*line).to_owned()).collect())
}

fn html_text(html: &str) -> String {
    let document = Html::parse_document(html);
    let mut writer = Writer::default();
    writer.element(document.root_element());
    writer.break_line();
    tidy(writer.lines)
}

fn tidy(lines: Vec<String>) -> String {
    let mut kept: Vec<String> = Vec::new();
    for line in lines {
        let blank = line.trim().is_empty();
        if blank && kept.last().is_none_or(|last| last.is_empty()) {
            continue;
        }
        kept.push(if blank { String::new() } else { line });
    }
    while kept.last().is_some_and(String::is_empty) {
        kept.pop();
    }
    kept.join("\n")
}

#[derive(Default)]
struct Writer {
    lines: Vec<String>,
    line: String,
    space: bool,
    row: Option<Vec<String>>,
    cell: Option<String>,
}

impl Writer {
    fn element(&mut self, element: ElementRef<'_>) {
        let name = element.value().name();
        if SKIPPED.contains(&name) {
            return;
        }
        match name {
            "br" => return self.line_break(),
            "td" | "th" if self.row.is_some() && self.cell.is_none() => {
                self.cell = Some(String::new());
                self.space = false;
                self.children(element);
                let cell = self.cell.take().unwrap_or_default();
                let cell = cell.trim();
                if !cell.is_empty() {
                    self.row.get_or_insert_default().push(cell.to_owned());
                }
                return;
            }
            "tr" if self.cell.is_none() => {
                self.break_line();
                self.row = Some(Vec::new());
                self.children(element);
                let row = self.row.take().unwrap_or_default();
                if !row.is_empty() {
                    self.lines.push(row.join("\t"));
                }
                self.space = false;
                return;
            }
            _ => {}
        }
        let block = BLOCKS.contains(&name) || name == "p";
        if block {
            self.break_line();
        }
        self.children(element);
        if block {
            self.break_line();
        }
        if name == "p" && self.cell.is_none() {
            self.lines.push(String::new());
        }
    }

    fn children(&mut self, element: ElementRef<'_>) {
        for child in element.children() {
            match child.value() {
                Node::Text(text) => self.text(text),
                Node::Element(_) => {
                    if let Some(element) = ElementRef::wrap(child) {
                        self.element(element);
                    }
                }
                _ => {}
            }
        }
    }

    fn target(&mut self) -> Option<&mut String> {
        match (&mut self.cell, &self.row) {
            (Some(cell), _) => Some(cell),
            (None, Some(_)) => None,
            (None, None) => Some(&mut self.line),
        }
    }

    fn text(&mut self, text: &str) {
        for letter in text.chars() {
            if letter.is_whitespace() {
                self.space = true;
                continue;
            }
            let space = std::mem::take(&mut self.space);
            if let Some(target) = self.target() {
                if space && !target.is_empty() {
                    target.push(' ');
                }
                target.push(letter);
            }
        }
    }

    fn break_line(&mut self) {
        if self.cell.is_some() {
            self.space = true;
            return;
        }
        let line = std::mem::take(&mut self.line);
        if !line.is_empty() {
            self.lines.push(line);
        }
        self.space = false;
    }

    fn line_break(&mut self) {
        if self.cell.is_some() {
            self.space = true;
            return;
        }
        let line = std::mem::take(&mut self.line);
        self.lines.push(line);
        self.space = false;
    }
}
