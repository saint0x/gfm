pub(crate) fn bounded_levenshtein(left: &str, right: &str, max: usize) -> Option<usize> {
    if left.len().abs_diff(right.len()) > max {
        return None;
    }
    let mut previous: Vec<usize> = (0..=right.len()).collect();
    let mut current = vec![0; right.len() + 1];

    for (i, left_ch) in left.chars().enumerate() {
        current[0] = i + 1;
        let mut row_min = current[0];
        for (j, right_ch) in right.chars().enumerate() {
            let cost = usize::from(left_ch != right_ch);
            current[j + 1] = (previous[j + 1] + 1)
                .min(current[j] + 1)
                .min(previous[j] + cost);
            row_min = row_min.min(current[j + 1]);
        }
        if row_min > max {
            return None;
        }
        std::mem::swap(&mut previous, &mut current);
    }

    (previous[right.len()] <= max).then_some(previous[right.len()])
}

pub(crate) fn deletion_keys(term: &str, max_deletions: usize) -> Vec<String> {
    let chars: Vec<char> = term.chars().collect();
    if chars.is_empty() {
        return Vec::new();
    }

    let mut keys = std::collections::BTreeSet::new();
    collect_deletions(&chars, max_deletions, &mut keys);
    keys.into_iter().collect()
}

fn collect_deletions(
    chars: &[char],
    remaining: usize,
    keys: &mut std::collections::BTreeSet<String>,
) {
    keys.insert(chars.iter().collect());
    if remaining == 0 || chars.len() <= 1 {
        return;
    }

    for index in 0..chars.len() {
        let mut next = Vec::with_capacity(chars.len() - 1);
        next.extend_from_slice(&chars[..index]);
        next.extend_from_slice(&chars[index + 1..]);
        collect_deletions(&next, remaining - 1, keys);
    }
}
