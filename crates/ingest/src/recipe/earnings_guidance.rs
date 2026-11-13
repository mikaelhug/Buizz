use serde_json::Value;

use super::gold::same_number;

pub(super) fn same_fact(gold: &Value, answer: &Value) -> bool {
    gold["fiscal_year"] == answer["fiscal_year"]
        && gold["fiscal_quarter"] == answer["fiscal_quarter"]
        && (same_number(&gold["low"], &answer["low"])
            || same_number(&gold["high"], &answer["high"]))
}
