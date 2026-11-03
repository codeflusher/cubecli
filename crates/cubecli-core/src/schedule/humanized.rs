//! The `Nd | Kh | Mm | Ts` humanized duration format (e.g. `7d`, `1d6h`, `30m`).

use std::time::Duration;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum HumanizedParse {
    /// Input does not look like a humanized duration at all (fall through to cron).
    NotHumanized,
    Valid(Duration),
    Invalid(String),
}

/// Parse a compound duration like `1d6h`, `30m`, `15s`. Units are case
/// insensitive; whitespace between groups is allowed. Anything containing
/// characters outside `[0-9dhmsDHMS ]` is [`HumanizedParse::NotHumanized`]
/// so cron expressions take the other path.
pub fn parse(input: &str) -> HumanizedParse {
    let s = input.trim();
    if s.is_empty() {
        return HumanizedParse::NotHumanized;
    }

    let mut total_secs: u64 = 0;
    let mut groups = 0usize;
    let bytes = s.as_bytes();
    let mut i = 0usize;

    while i < bytes.len() {
        match bytes[i] {
            b' ' => {
                i += 1;
            }
            b'0'..=b'9' => {
                let start = i;
                while i < bytes.len() && bytes[i].is_ascii_digit() {
                    i += 1;
                }
                let Ok(n) = s[start..i].parse::<u64>() else {
                    return HumanizedParse::Invalid(format!("number too large in `{input}`"));
                };
                if i >= bytes.len() {
                    // digits with no unit — not humanized (maybe a cron field)
                    return HumanizedParse::NotHumanized;
                }
                let unit = bytes[i].to_ascii_lowercase();
                let factor: u64 = match unit {
                    b'd' => 86_400,
                    b'h' => 3_600,
                    b'm' => 60,
                    b's' => 1,
                    _ => return HumanizedParse::NotHumanized,
                };
                i += 1;
                let Some(secs) = n.checked_mul(factor) else {
                    return HumanizedParse::Invalid(format!("duration `{input}` overflows"));
                };
                let Some(sum) = total_secs.checked_add(secs) else {
                    return HumanizedParse::Invalid(format!("duration `{input}` overflows"));
                };
                total_secs = sum;
                groups += 1;
            }
            _ => return HumanizedParse::NotHumanized,
        }
    }

    if groups == 0 || total_secs == 0 {
        return HumanizedParse::Invalid("duration must be greater than zero".into());
    }
    HumanizedParse::Valid(Duration::from_secs(total_secs))
}

/// Canonical compact form: `1d6h30m15s`, omitting zero components.
pub fn format(d: Duration) -> String {
    let mut secs = d.as_secs();
    let days = secs / 86_400;
    secs %= 86_400;
    let hours = secs / 3_600;
    secs %= 3_600;
    let minutes = secs / 60;
    secs %= 60;

    let mut out = String::new();
    if days > 0 {
        out.push_str(&format!("{days}d"));
    }
    if hours > 0 {
        out.push_str(&format!("{hours}h"));
    }
    if minutes > 0 {
        out.push_str(&format!("{minutes}m"));
    }
    if secs > 0 || out.is_empty() {
        out.push_str(&format!("{secs}s"));
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn single_units() {
        assert_eq!(
            parse("7d"),
            HumanizedParse::Valid(Duration::from_secs(7 * 86_400))
        );
        assert_eq!(
            parse("12H"),
            HumanizedParse::Valid(Duration::from_secs(12 * 3_600))
        );
        assert_eq!(
            parse("90m"),
            HumanizedParse::Valid(Duration::from_secs(90 * 60))
        );
        assert_eq!(parse("15s"), HumanizedParse::Valid(Duration::from_secs(15)));
    }

    #[test]
    fn compound_with_spaces() {
        assert_eq!(
            parse("1d 6h 30m"),
            HumanizedParse::Valid(Duration::from_secs(86_400 + 6 * 3_600 + 30 * 60))
        );
        assert_eq!(
            parse("1d6h"),
            HumanizedParse::Valid(Duration::from_secs(86_400 + 6 * 3_600))
        );
    }

    #[test]
    fn rejects_zero_and_garbage() {
        assert!(matches!(parse("0d"), HumanizedParse::Invalid(_)));
        assert_eq!(parse(""), HumanizedParse::NotHumanized);
        assert_eq!(parse("0 4 * * *"), HumanizedParse::NotHumanized);
        assert_eq!(parse("weekly"), HumanizedParse::NotHumanized);
        assert_eq!(parse("60"), HumanizedParse::NotHumanized);
        assert_eq!(parse("10x"), HumanizedParse::NotHumanized);
    }

    #[test]
    fn canonical_format() {
        assert_eq!(format(Duration::from_secs(86_400 + 6 * 3_600)), "1d6h");
        assert_eq!(format(Duration::from_secs(30 * 60)), "30m");
        assert_eq!(format(Duration::from_secs(0)), "0s");
        assert_eq!(format(Duration::from_secs(90)), "1m30s");
    }
}
