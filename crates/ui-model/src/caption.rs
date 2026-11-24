/// Short labels telling same-kind devices apart; `None` when a device stands alone.
pub fn captions(names: &[&str]) -> Vec<Option<String>> {
    if names.len() < 2 {
        return vec![None; names.len()];
    }
    // Brand first ("ATK" / "VGN"), then the model tail ("XM4" / "XM5"), then numbers.
    for label in [name_prefix, name_suffix] {
        let labels: Vec<String> = names.iter().map(|name| label(name)).collect();
        let distinct = labels
            .iter()
            .enumerate()
            .all(|(i, label)| !label.is_empty() && !labels[..i].contains(label));
        if distinct {
            return labels.into_iter().map(Some).collect();
        }
    }
    (1..=names.len())
        .map(|index| Some(index.to_string()))
        .collect()
}

fn tokens(name: &str) -> impl DoubleEndedIterator<Item = &str> {
    name.split(|c: char| c.is_whitespace() || c == '-')
        .filter(|token| !token.is_empty())
}

/// First three characters of the first token (usually the brand).
fn name_prefix(name: &str) -> String {
    tokens(name)
        .next()
        .unwrap_or_default()
        .chars()
        .take(3)
        .collect()
}

/// Last three characters of the last whitespace/hyphen separated token.
fn name_suffix(name: &str) -> String {
    let token = tokens(name).next_back().unwrap_or_default();
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
        assert_eq!(
            captions(&["ATK X1 SE", "VGN Dragonfly 3 Master+"]),
            [Some("ATK".to_owned()), Some("VGN".to_owned())]
        );
        assert_eq!(
            captions(&["ATK X1 SE", "ATK A9 Ultra"]),
            [Some("SE".to_owned()), Some("tra".to_owned())]
        );
    }
}
