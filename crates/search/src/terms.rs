const FUZZY_MIN_TERM_LEN: usize = 2;
const FUZZY_MAX_TERM_LEN: usize = 32;
const PREFIX_MIN_TERM_LEN: usize = 1;
const PREFIX_MAX_TERM_LEN: usize = 32;
pub(crate) const SUBSTRING_GRAM_CHARS: usize = 3;

pub(crate) fn path_key(path: &std::path::Path) -> String {
    path.to_string_lossy().into_owned()
}

pub(crate) fn is_fuzzy_term(term: &str) -> bool {
    let mut count = 0;
    let mut has_alpha = false;
    let mut consecutive_digits = 0;
    for ch in term.chars() {
        count += 1;
        if ch.is_alphabetic() {
            has_alpha = true;
            consecutive_digits = 0;
        } else if ch.is_ascii_digit() {
            consecutive_digits += 1;
            if consecutive_digits > 4 {
                return false;
            }
        } else {
            consecutive_digits = 0;
        }
    }
    (FUZZY_MIN_TERM_LEN..=FUZZY_MAX_TERM_LEN).contains(&count) && has_alpha
}

pub(crate) fn is_prefix_term(term: &str) -> bool {
    (PREFIX_MIN_TERM_LEN..=PREFIX_MAX_TERM_LEN).contains(&term.chars().count())
}

pub(crate) fn token_prefixes(term: &str) -> impl Iterator<Item = String> + '_ {
    term.char_indices()
        .map(|(index, _)| index)
        .skip(1)
        .chain(std::iter::once(term.len()))
        .take(PREFIX_MAX_TERM_LEN)
        .map(|end| term[..end].to_string())
}

pub(crate) fn substring_grams(value: &str) -> Vec<String> {
    let mut starts = value
        .char_indices()
        .map(|(index, _)| index)
        .collect::<Vec<_>>();
    starts.push(value.len());
    if starts.len() <= SUBSTRING_GRAM_CHARS {
        return Vec::new();
    }
    let mut grams = starts
        .windows(SUBSTRING_GRAM_CHARS + 1)
        .map(|window| value[window[0]..window[SUBSTRING_GRAM_CHARS]].to_string())
        .collect::<Vec<_>>();
    grams.sort();
    grams.dedup();
    grams
}

pub(crate) fn is_substring_gram(value: &str) -> bool {
    value.chars().count() == SUBSTRING_GRAM_CHARS
}
