use std::str::FromStr;
use std::sync::LazyLock;

use regex::Regex;
use rust_decimal::Decimal;

static NUMBER: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(
        r"(?i)(?P<digits>\d{1,3}(?:,\d{3})+(?:\.\d+)?|\d+(?:\.\d+)?|\.\d+)|\b(?P<word>zero|one|two|three|four|five|six|seven|eight|nine|ten|eleven|twelve|thirteen|fourteen|fifteen|sixteen|seventeen|eighteen|nineteen|twenty|thirty|forty|fifty|sixty|seventy|eighty|ninety|first|second|third|fourth|fifth|sixth|seventh|eighth|ninth|tenth|eleventh|twelfth)(?:[-\s](?P<unit>one|two|three|four|five|six|seven|eight|nine)\b)?\b",
    )
    .expect("a valid number pattern")
});

static SCALE_WORD: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"(?i)^\s*(hundred|thousand|million|billion|trillion)\b")
        .expect("a valid scale pattern")
});

static SCALE_SUFFIX: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"(?i)^\s?(k|mm|mn|m|bn|b|tn)\b").expect("a valid suffix pattern"));

static PERCENT: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"(?i)^\s*(%|percent\b|per\s+cent\b)").expect("a valid percent pattern")
});

static RANGE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"(?i)^\s*(?:%|percent)?\s*(?:to|-|–|—|and|or|through)\s*(?:US\$|\$|USD)?\s*$")
        .expect("a valid range pattern")
});

static DECLARED: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"(?i)\bin\s+(thousands|millions|billions)\b").expect("a valid declaration pattern")
});

struct Token {
    value: Decimal,
    start: usize,
    end: usize,
    scale: Option<Decimal>,
    percent: bool,
}

fn power(exponent: u32) -> Decimal {
    Decimal::from(10u64.pow(exponent))
}

fn scale_of(word: &str) -> Decimal {
    match word.to_ascii_lowercase().as_str() {
        "hundred" => power(2),
        "thousand" | "k" | "thousands" => power(3),
        "million" | "mm" | "mn" | "m" | "millions" => power(6),
        "billion" | "bn" | "b" | "billions" => power(9),
        _ => power(12),
    }
}

fn word_value(word: &str) -> u32 {
    const WORDS: [&str; 20] = [
        "zero",
        "one",
        "two",
        "three",
        "four",
        "five",
        "six",
        "seven",
        "eight",
        "nine",
        "ten",
        "eleven",
        "twelve",
        "thirteen",
        "fourteen",
        "fifteen",
        "sixteen",
        "seventeen",
        "eighteen",
        "nineteen",
    ];
    const TENS: [&str; 8] = [
        "twenty", "thirty", "forty", "fifty", "sixty", "seventy", "eighty", "ninety",
    ];
    const ORDINALS: [&str; 12] = [
        "first", "second", "third", "fourth", "fifth", "sixth", "seventh", "eighth", "ninth",
        "tenth", "eleventh", "twelfth",
    ];
    let word = word.to_ascii_lowercase();
    let position = |list: &[&str]| list.iter().position(|known| *known == word);
    position(&WORDS)
        .or_else(|| position(&TENS).map(|tens| (tens + 2) * 10))
        .or_else(|| position(&ORDINALS).map(|ordinal| ordinal + 1))
        .and_then(|value| u32::try_from(value).ok())
        .unwrap_or(0)
}

pub(crate) fn declared_scales(section: &str) -> Vec<Decimal> {
    let mut scales: Vec<Decimal> = DECLARED
        .captures_iter(section)
        .map(|found| scale_of(&found[1]))
        .collect();
    scales.sort();
    scales.dedup();
    scales
}

fn tokens(quote: &str) -> Vec<Token> {
    let mut tokens: Vec<Token> = NUMBER
        .captures_iter(quote)
        .filter_map(|found| {
            let whole = found.get(0)?;
            let value = match (found.name("digits"), found.name("word")) {
                (Some(digits), _) => Decimal::from_str(&digits.as_str().replace(',', "")).ok()?,
                (None, Some(word)) => Decimal::from(
                    word_value(word.as_str())
                        + found
                            .name("unit")
                            .map_or(0, |unit| word_value(unit.as_str())),
                ),
                (None, None) => return None,
            };
            let rest = &quote[whole.end()..];
            let scale = SCALE_WORD
                .captures(rest)
                .or_else(|| SCALE_SUFFIX.captures(rest))
                .map(|scale| scale_of(&scale[1]));
            Some(Token {
                value,
                start: whole.start(),
                end: whole.end(),
                scale,
                percent: scale.is_none() && PERCENT.is_match(rest),
            })
        })
        .collect();
    for index in (0..tokens.len().saturating_sub(1)).rev() {
        let (head, tail) = tokens.split_at_mut(index + 1);
        let (token, next) = (&mut head[index], &tail[0]);
        if token.scale.is_none() && !token.percent && RANGE.is_match(&quote[token.end..next.start])
        {
            token.scale = next.scale;
            token.percent = next.percent;
        }
    }
    tokens
}

pub(crate) fn written_values(quote: &str, declared: &[Decimal]) -> Vec<Decimal> {
    let mut values = Vec::new();
    for token in tokens(quote) {
        values.push(token.value);
        if let Some(scale) = token.scale {
            values.extend(token.value.checked_mul(scale));
        } else if token.percent {
            values.extend(token.value.checked_div(power(2)));
        } else {
            values.extend(
                declared
                    .iter()
                    .filter_map(|scale| token.value.checked_mul(*scale)),
            );
        }
    }
    values
}

pub(crate) fn is_written(value: Decimal, written: &[Decimal]) -> bool {
    written.contains(&value.abs())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn written(quote: &str, section: &str, value: &str) -> bool {
        is_written(
            Decimal::from_str(value).unwrap(),
            &written_values(quote, &declared_scales(section)),
        )
    }

    #[test]
    fn numbers_count_only_in_the_forms_the_quote_writes_them() {
        let guidance =
            "expects revenue of $1.20 to $1.25 billion and EPS of $2.10, a margin of 38%";
        for value in ["1200000000", "1250000000", "2.1", "38", "0.38"] {
            assert!(written(guidance, guidance, value), "{value} is written");
        }
        for value in ["1300000000", "12", "120", "2.01", "380"] {
            assert!(!written(guidance, guidance, value), "{value} is invented");
        }
        let table = "Revenue\t$\t1,250\t(1,100)";
        let section = format!("(in millions, except per share data)\n{table}");
        assert!(written(table, &section, "1250000000"));
        assert!(written(table, &section, "-1100000000"));
        assert!(!written(table, table, "1250000000"));
        assert!(written(
            "cut about twenty-five positions in the third quarter",
            "",
            "25"
        ));
        assert!(written("for Q3 of fiscal 2026", "", "3"));
        assert!(written("a $500M buyback", "", "500000000"));
    }
}
