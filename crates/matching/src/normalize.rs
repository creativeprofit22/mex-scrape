//! Text normalization for title and brand comparison.

use std::collections::BTreeSet;

/// Spanish and English words that carry no product identity.
const STOPWORDS: &[&str] = &[
    "a", "al", "and", "con", "de", "del", "el", "en", "for", "la", "las", "los", "of", "para",
    "por", "sin", "the", "un", "una", "with", "y",
];

/// Folds Spanish accents to ASCII; other characters pass through.
fn fold(c: char) -> char {
    match c {
        'á' | 'à' | 'ä' | 'â' => 'a',
        'é' | 'è' | 'ë' | 'ê' => 'e',
        'í' | 'ì' | 'ï' | 'î' => 'i',
        'ó' | 'ò' | 'ö' | 'ô' => 'o',
        'ú' | 'ù' | 'ü' | 'û' => 'u',
        'ñ' => 'n',
        'ç' => 'c',
        other => other,
    }
}

/// Distinct normalized tokens of `text`: lowercased, accents folded, split
/// on anything that is not an ASCII letter or digit, stopwords removed.
#[must_use]
pub fn tokens(text: &str) -> BTreeSet<String> {
    let cleaned: String = text
        .chars()
        .flat_map(char::to_lowercase)
        .map(fold)
        .map(|c| if c.is_ascii_alphanumeric() { c } else { ' ' })
        .collect();
    cleaned
        .split_whitespace()
        .filter(|t| !STOPWORDS.contains(t))
        .map(str::to_owned)
        .collect()
}

/// Tokens that contain a digit (model numbers, sizes, capacities).
#[must_use]
pub fn digit_tokens(tokens: &BTreeSet<String>) -> BTreeSet<&str> {
    tokens
        .iter()
        .filter(|t| t.bytes().any(|b| b.is_ascii_digit()))
        .map(String::as_str)
        .collect()
}

/// Token Dice similarity `2|A∩B| / (|A|+|B|)` in basis points (0 when both
/// are empty).
#[must_use]
pub fn dice_bp(a: &BTreeSet<String>, b: &BTreeSet<String>) -> u32 {
    let total = a.len() + b.len();
    if total == 0 {
        return 0;
    }
    let common = a.intersection(b).count();
    u32::try_from(2 * common * 10_000 / total).unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn set(words: &[&str]) -> BTreeSet<String> {
        words.iter().map(|w| (*w).to_owned()).collect()
    }

    #[test]
    fn tokens_fold_accents_and_drop_stopwords() {
        assert_eq!(
            tokens("Licuadora ÓSTER de 10 Velocidades, Acero-Inoxidable ñ"),
            set(&[
                "licuadora",
                "oster",
                "10",
                "velocidades",
                "acero",
                "inoxidable",
                "n"
            ])
        );
        assert_eq!(tokens("  ¡¿ — !? "), set(&[]));
    }

    #[test]
    fn dice_counts_shared_tokens() {
        let a = set(&["a1", "b", "c"]);
        let b = set(&["b", "c", "d"]);
        assert_eq!(dice_bp(&a, &b), 6_666);
        assert_eq!(dice_bp(&a, &a), 10_000);
        assert_eq!(dice_bp(&set(&[]), &set(&[])), 0);
        assert_eq!(digit_tokens(&a), ["a1"].into_iter().collect());
    }
}
