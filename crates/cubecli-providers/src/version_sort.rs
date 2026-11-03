//! Version ordering across the two Minecraft versioning schemes
//! (`1.21.11`-style and `26.3`-style) plus provider build numbers.

/// Newest-first sort of version strings, numeric components compared
/// numerically, text components lexicographically, missing < present.
pub fn sort_desc(items: &mut [String]) {
    items.sort_by(|a, b| cmp_versions(b, a));
}

pub fn cmp_versions(a: &str, b: &str) -> std::cmp::Ordering {
    let (ka, kb) = (key(a), key(b));
    for i in 0..ka.len().max(kb.len()) {
        match (ka.get(i), kb.get(i)) {
            (None, None) => return std::cmp::Ordering::Equal,
            (None, Some(_)) => return std::cmp::Ordering::Less,
            (Some(_), None) => return std::cmp::Ordering::Greater,
            (Some(x), Some(y)) => {
                let ord = x.cmp(y);
                if ord != std::cmp::Ordering::Equal {
                    return ord;
                }
            }
        }
    }
    std::cmp::Ordering::Equal
}

#[derive(Debug, PartialEq, Eq, PartialOrd, Ord)]
enum Component {
    /// Sorts below numbers so `1.21` < `1.21.8`.
    Text(String),
    Num(u64),
}

fn key(v: &str) -> Vec<Component> {
    v.split(['.', '-', '+', '_', ' '])
        .filter(|p| !p.is_empty())
        .map(|part| match part.parse::<u64>() {
            Ok(n) => Component::Num(n),
            Err(_) => Component::Text(part.to_lowercase()),
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn numeric_components_beat_text() {
        assert_eq!(cmp_versions("1.21.8", "1.21.11"), std::cmp::Ordering::Less);
        assert_eq!(cmp_versions("26.3", "1.21.11"), std::cmp::Ordering::Greater);
        assert_eq!(cmp_versions("1.21", "1.21.8"), std::cmp::Ordering::Less);
    }

    #[test]
    fn sort_desc_puts_newest_first() {
        let mut v = vec![
            "1.8.9".into(),
            "26.3".into(),
            "1.20.1".into(),
            "1.21.11".into(),
            "1.21.9".into(),
        ];
        sort_desc(&mut v);
        assert_eq!(v, vec!["26.3", "1.21.11", "1.21.9", "1.20.1", "1.8.9"]);
    }
}
