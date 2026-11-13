use std::str::FromStr;
use std::sync::LazyLock;

use regex::Regex;
use rust_decimal::Decimal;

static NUMBER: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(
        r"(?i)(?P<digits>\d{1,3}(?:,\d{3})+(?:\.\d+)?|\d+(?:\.\d+)?|\.\d+)|\b(?P<word>flat|zero|one|two|three|four|five|six|seven|eight|nine|ten|eleven|twelve|thirteen|fourteen|fifteen|sixteen|seventeen|eighteen|nineteen|twenty|thirty|forty|fifty|sixty|seventy|eighty|ninety|first|second|third|fourth|fifth|sixth|seventh|eighth|ninth|tenth|eleventh|twelfth)(?:[-\s](?P<unit>one|two|three|four|five|six|seven|eight|nine)\b)?\b",
    )
    .expect("a valid number pattern")
});

static SCALE_WORD: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"(?i)^\+?\s*(hundred|thousand|million|billion|trillion)\b")
        .expect("a valid scale pattern")
});

static SCALE_SUFFIX: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"(?i)^\s?(k|mm|mn|m|bn|b|tn)\b").expect("a valid suffix pattern"));

static PERCENT: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"(?i)^\s*(%|percent\b|per\s+cent\b)").expect("a valid percent pattern")
});

static CENTS: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"(?i)^\s*cents?\b").expect("a valid cents pattern"));

static RANGE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"(?i)^\s*(?:%|percent)?\s*(?:to|-|–|—|and|or|through)\s*(?:US\$|\$|USD)?\s*$")
        .expect("a valid range pattern")
});

static DECLARED: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(
        r"(?i)\bin\s+(thousands|millions|billions)\b|\(\s*(?:(?:US)?\$\s*)?(?:in\s+)?(thousands|millions|billions)\s*\)|\(\s*(?:US)?\$\s*(?:in\s+)?(K|M|MM|B|BN)\s*\)",
    )
    .expect("a valid declaration pattern")
});

static PLUS_OR_MINUS: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"(?i)^\s*(?:hundred|thousand|million|billion|trillion|k|mm|mn|m|bn|b|tn)?\s*,?\s*(?:plus\s+or\s+minus|\+/-|±)\s*(?:US\$|\$|USD)?\s*$")
        .expect("a valid plus or minus pattern")
});

struct Token {
    value: Decimal,
    start: usize,
    end: usize,
    scale: Option<Decimal>,
    percent: bool,
    cents: bool,
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
        .or_else(|| (word == "flat").then_some(0))
        .or_else(|| position(&TENS).map(|tens| (tens + 2) * 10))
        .or_else(|| position(&ORDINALS).map(|ordinal| ordinal + 1))
        .and_then(|value| u32::try_from(value).ok())
        .unwrap_or(0)
}

pub(crate) fn declared_scales(section: &str) -> Vec<Decimal> {
    let mut scales: Vec<Decimal> = DECLARED
        .captures_iter(section)
        .filter_map(|found| found.iter().skip(1).flatten().next())
        .map(|scale| scale_of(scale.as_str()))
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
                cents: CENTS.is_match(rest),
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

impl Token {
    fn scaled(&self) -> Decimal {
        self.scale
            .and_then(|scale| self.value.checked_mul(scale))
            .unwrap_or(self.value)
    }
}

pub(crate) fn written_values(quote: &str, declared: &[Decimal]) -> Vec<Decimal> {
    let mut values = Vec::new();
    let tokens = tokens(quote);
    for pair in tokens.windows(2) {
        if PLUS_OR_MINUS.is_match(&quote[pair[0].end..pair[1].start]) {
            let (centre, spread) = (pair[0].scaled(), pair[1].scaled());
            values.extend(centre.checked_sub(spread));
            values.extend(centre.checked_add(spread));
        }
    }
    for token in tokens {
        values.push(token.value);
        if let Some(scale) = token.scale {
            values.extend(token.value.checked_mul(scale));
        } else if token.percent || token.cents {
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
        let row = "Interest and other expense, net ($M)\t~$95\t~$95";
        assert!(written(row, row, "95000000"));
        let spread = "net sales to be approximately $6.51 billion, plus or minus $400 million";
        assert!(written(spread, spread, "6110000000"));
        assert!(written(spread, spread, "6910000000"));
        let shares = "Shares used in computing diluted earnings per share (millions)\t43.5";
        assert!(written(shares, shares, "43500000"));
        assert!(!written(
            "Rule 5550(b)(2) requires 1",
            "Rule 5550(b)(2)",
            "1000000000"
        ));
        assert!(written("comparable sales flat to up 1%", "", "0"));
        assert!(written("estimated to be about 35 cents.", "", "0.35"));
        assert!(written(
            "Adjusted free cash flow\t$1.2+ billion",
            "",
            "1200000000"
        ));
    }
}
