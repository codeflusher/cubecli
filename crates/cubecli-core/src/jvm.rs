//! JVM heap-flag synthesis shared by the native and compose backends.

/// `-Xmx{n}` from the configured `max-heap` value.
pub fn heap_flags(max_heap: Option<&str>) -> Vec<String> {
    max_heap
        .map(|v| vec![format!("-Xmx{v}")])
        .unwrap_or_default()
}

/// Drop provider-authored `-Xms`/`-Xmx` tokens — an explicit `max-heap`
/// config wins over what the installer suggested.
pub fn strip_heap_flags(args: &[String]) -> Vec<String> {
    args.iter()
        .filter(|arg| !is_heap_flag(arg))
        .cloned()
        .collect()
}

fn is_heap_flag(arg: &str) -> bool {
    arg.starts_with("-Xms") || arg.starts_with("-Xmx")
}

/// Merge provider-authored JVM args with an explicit `max-heap` override.
/// No override → provider args untouched (their defaults stand).
pub fn merge_jvm_args(provider: &[String], max_heap: Option<&str>) -> Vec<String> {
    match max_heap {
        None => provider.to_vec(),
        Some(v) => {
            let mut args = heap_flags(Some(v));
            args.extend(strip_heap_flags(provider));
            args
        }
    }
}

/// Validate a `max-heap` value: `4G`, `512M`, `2g`, `1024m`, or empty
/// (clears the override). The unit is required — a bare number is rejected
/// because the JVM would refuse it anyway.
pub fn parse_max_heap(value: &str) -> Result<(), String> {
    let Some(unit) = value.chars().next_back() else {
        return Err(heap_error(value));
    };
    if !matches!(unit, 'M' | 'm' | 'G' | 'g') {
        return Err(heap_error(value));
    }
    let digits =
        value.chars().rev().skip(1).all(|c| c.is_ascii_digit()) && value.chars().count() >= 2;
    if digits {
        Ok(())
    } else {
        Err(heap_error(value))
    }
}

fn heap_error(value: &str) -> String {
    format!("`{value}` is not a heap size (e.g. `4G`, `512M`; empty clears the override)")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn heap_flags_and_strip_roundtrip() {
        assert!(heap_flags(None).is_empty());
        assert_eq!(heap_flags(Some("4G")), vec!["-Xmx4G"]);

        let provider: Vec<String> = vec!["-Xms1G".into(), "-Xmx4G".into(), "-XX:+UseG1GC".into()];
        assert_eq!(
            strip_heap_flags(&provider),
            vec!["-XX:+UseG1GC".to_string()]
        );
    }

    #[test]
    fn merge_preserves_provider_defaults_without_override() {
        let provider: Vec<String> = vec!["-Xmx4G".into(), "-XX:+UseG1GC".into()];
        assert_eq!(
            merge_jvm_args(&provider, None),
            vec!["-Xmx4G".to_string(), "-XX:+UseG1GC".to_string()],
            "no override — provider flags stand"
        );
        assert_eq!(
            merge_jvm_args(&provider, Some("2G")),
            vec!["-Xmx2G".to_string(), "-XX:+UseG1GC".to_string()]
        );
    }

    #[test]
    fn parse_max_heap_accepts_sizes_and_empty_rejection() {
        for v in ["4G", "512M", "2g", "1024m"] {
            assert!(parse_max_heap(v).is_ok(), "{v}");
        }
        for v in ["4", "G", "4T", "abc", "4GB", ""] {
            assert!(parse_max_heap(v).is_err(), "{v}");
        }
    }
}
