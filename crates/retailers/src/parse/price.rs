//! Mexican price parsing from JSON values and page text.

use domain::PriceMxn;
use serde_json::Value;

/// A price from a JSON number or string (`1299.5`, `"1299.50"`, `"$1,299"`).
#[must_use]
pub fn from_json(value: &Value) -> Option<PriceMxn> {
    match value {
        Value::Number(n) => PriceMxn::from_decimal_str(&n.to_string()).ok(),
        Value::String(s) => from_str(s),
        _ => None,
    }
}

/// A price from a plain decimal or displayed price string.
#[must_use]
pub fn from_str(s: &str) -> Option<PriceMxn> {
    PriceMxn::from_decimal_str(s)
        .or_else(|_| PriceMxn::parse_mxn(s))
        .ok()
}

/// The first `$` amount in a text such as `"Antes $1,499.00 Ahora $999"`.
#[must_use]
pub fn first_in_text(text: &str) -> Option<PriceMxn> {
    all_in_text(text).into_iter().next()
}

/// Every `$` amount in a text, in order.
#[must_use]
pub fn all_in_text(text: &str) -> Vec<PriceMxn> {
    let mut out = Vec::new();
    for (i, _) in text.match_indices('$') {
        let rest = text[i + 1..].trim_start();
        let amount: String = rest
            .chars()
            .take_while(|c| c.is_ascii_digit() || *c == ',' || *c == '.')
            .collect();
        let amount = amount.trim_end_matches(['.', ',']);
        if let Ok(p) = PriceMxn::parse_mxn(amount) {
            out.push(p);
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    fn c(p: Option<PriceMxn>) -> Option<i64> {
        p.map(PriceMxn::centavos)
    }

    #[test]
    fn json_numbers_and_strings() {
        assert_eq!(c(from_json(&json!(1299.5))), Some(129_950));
        assert_eq!(c(from_json(&json!(1299))), Some(129_900));
        assert_eq!(c(from_json(&json!("1299.00"))), Some(129_900));
        assert_eq!(c(from_json(&json!("$1,299.00"))), Some(129_900));
        assert_eq!(c(from_json(&json!(-1))), None);
        assert_eq!(c(from_json(&json!(null))), None);
    }

    #[test]
    fn text_amounts() {
        assert_eq!(c(first_in_text("Precio: $ 1,299.00 MXN")), Some(129_900));
        let all: Vec<i64> = all_in_text("Antes $1,499. Ahora $999")
            .into_iter()
            .map(PriceMxn::centavos)
            .collect();
        assert_eq!(all, vec![149_900, 99_900]);
        assert_eq!(first_in_text("sin precio"), None);
    }
}
