pub(crate) fn table_rows(wikitext: &str, id: &str) -> Option<Vec<Vec<String>>> {
    let marker = format!("id=\"{id}\"");
    let start = wikitext.find(&marker)?;
    let body = without_comments(&wikitext[start..]);
    let end = body.find("\n|}").unwrap_or(body.len());
    Some(
        body[..end]
            .split("\n|-")
            .skip(1)
            .filter(|row| !row.trim_start().starts_with('!'))
            .map(cells)
            .filter(|cells| !cells.is_empty())
            .collect(),
    )
}

fn without_comments(text: &str) -> String {
    let mut kept = String::with_capacity(text.len());
    let mut rest = text;
    while let Some(start) = rest.find("<!--") {
        kept.push_str(&rest[..start]);
        rest = rest[start..]
            .find("-->")
            .map_or("", |end| &rest[start + end + 3..]);
    }
    kept.push_str(rest);
    kept
}

fn cells(row: &str) -> Vec<String> {
    let mut cells = Vec::new();
    let mut current = String::new();
    let mut depth = 0usize;
    let mut in_cell = false;
    let mut chars = row.chars().peekable();
    let mut at_line_start = false;
    while let Some(letter) = chars.next() {
        let next = chars.peek().copied();
        match (letter, next) {
            ('[', Some('[')) | ('{', Some('{')) => {
                depth += 1;
                current.push(letter);
                current.push(chars.next().unwrap_or_default());
                at_line_start = false;
                continue;
            }
            (']', Some(']')) | ('}', Some('}')) => {
                depth = depth.saturating_sub(1);
                current.push(letter);
                current.push(chars.next().unwrap_or_default());
                at_line_start = false;
                continue;
            }
            ('\n', _) if depth == 0 => {
                at_line_start = true;
                current.push(letter);
                continue;
            }
            ('|', Some('|')) if depth == 0 => {
                chars.next();
                if in_cell {
                    cells.push(cell_text(&current));
                }
                current.clear();
                in_cell = true;
            }
            ('|', _) if depth == 0 && at_line_start => {
                if in_cell {
                    cells.push(cell_text(&current));
                }
                current.clear();
                in_cell = true;
            }
            _ => current.push(letter),
        }
        at_line_start = false;
    }
    if in_cell {
        cells.push(cell_text(&current));
    }
    cells
}

fn cell_text(raw: &str) -> String {
    let text = raw.trim();
    match split_attributes(text) {
        Some((_, content)) if !content.trim().is_empty() => content.trim(),
        Some((attributes, _)) => attributes.trim(),
        None => text,
    }
    .to_owned()
}

fn split_attributes(text: &str) -> Option<(&str, &str)> {
    let mut depth = 0usize;
    let bytes = text.as_bytes();
    let mut index = 0;
    let mut split = None;
    while index < bytes.len() {
        match (bytes[index], bytes.get(index + 1)) {
            (b'[', Some(b'[')) | (b'{', Some(b'{')) => {
                depth += 1;
                index += 2;
                continue;
            }
            (b']', Some(b']')) | (b'}', Some(b'}')) => {
                depth = depth.saturating_sub(1);
                index += 2;
                continue;
            }
            (b'<', _) if text[index..].starts_with("<ref") => break,
            (b'|', _) if depth == 0 => split = Some(index),
            _ => {}
        }
        index += 1;
    }
    split.map(|at| (&text[..at], &text[at + 1..]))
}

pub(crate) fn first_link(cell: &str) -> Option<String> {
    let start = cell.find("[[")? + 2;
    let end = start + cell[start..].find("]]")?;
    let target = cell[start..end].split('|').next()?.trim();
    let target = target.split('#').next()?.replace('_', " ");
    let mut letters = target.chars();
    let first = letters.next()?;
    Some(first.to_uppercase().chain(letters).collect())
}

pub(crate) fn ticker(cell: &str) -> Option<String> {
    let text = cell.split("<ref").next()?.trim();
    let inner = match text.strip_prefix("{{") {
        Some(template) => template
            .trim_end_matches("}}")
            .rsplit('|')
            .next()?
            .to_owned(),
        None => match first_link(text) {
            Some(_) => {
                let start = text.find("[[")? + 2;
                let end = start + text[start..].find("]]")?;
                text[start..end].rsplit('|').next()?.to_owned()
            }
            None => text.to_owned(),
        },
    };
    let symbol = inner.trim().trim_end_matches('|').trim();
    let valid = !symbol.is_empty()
        && symbol
            .chars()
            .all(|letter| letter.is_ascii_alphanumeric() || letter == '.' || letter == '-');
    valid.then(|| symbol.to_ascii_uppercase())
}
