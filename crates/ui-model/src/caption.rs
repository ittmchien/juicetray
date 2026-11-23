/// Short labels telling same-kind devices apart; `None` when a device stands alone.
pub fn captions(names: &[&str]) -> Vec<Option<String>> {
    if names.len() < 2 {
        return vec![None; names.len()];
    }
    let suffixes: Vec<String> = names.iter().map(|name| name_suffix(name)).collect();
    let distinct = suffixes
        .iter()
        .enumerate()
        .all(|(i, suffix)| !suffix.is_empty() && !suffixes[..i].contains(suffix));
    if distinct {
        return suffixes.into_iter().map(Some).collect();
    }
    (1..=names.len())
        .map(|index| Some(index.to_string()))
        .collect()
}

/// Last three characters of the last whitespace/hyphen separated token.
fn name_suffix(name: &str) -> String {
    let token = name
        .split(|c: char| c.is_whitespace() || c == '-')
        .rfind(|token| !token.is_empty())
        .unwrap_or_default();
    let tail: Vec<char> = token.chars().rev().take(3).collect();
    tail.into_iter().rev().collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn captions_distinguish_same_kind() {
        assert_eq!(
            captions(&["WH-1000XM4", "WH-1000XM5"]),
            [Some("XM4".to_owned()), Some("XM5".to_owned())]
        );
        assert_eq!(
            captions(&["Pods", "Pods"]),
            [Some("1".to_owned()), Some("2".to_owned())]
        );
        assert_eq!(captions(&["Pods"]), [None]);
    }
}
