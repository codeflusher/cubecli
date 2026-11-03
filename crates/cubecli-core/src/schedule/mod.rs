//! Schedule rules: a cron expression or a fixed interval (`6h`, `1d6h`).
//! The single API consumers use is [`ScheduleRule::next_after`].

pub mod humanized;

use std::time::Duration;

use chrono::{DateTime, Local, Utc};

use crate::error::{CoreError, Result};

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ScheduleRule {
    Cron(String),
    Fixed(Duration),
}

impl ScheduleRule {
    /// Parse either format. Humanized wins when the input fits the grammar;
    /// everything else is validated as a cron expression.
    pub fn parse(raw: &str) -> Result<Self> {
        let s = raw.trim();
        if s.is_empty() {
            return Err(CoreError::InvalidSchedule {
                value: raw.into(),
                message: "empty rule".into(),
            });
        }
        match humanized::parse(s) {
            humanized::HumanizedParse::Valid(d) => Ok(Self::Fixed(d)),
            humanized::HumanizedParse::Invalid(msg) => Err(CoreError::InvalidSchedule {
                value: raw.into(),
                message: msg,
            }),
            humanized::HumanizedParse::NotHumanized => {
                validate_cron(s).map_err(|msg| CoreError::InvalidSchedule {
                    value: raw.into(),
                    message: format!(
                        "not a duration like `6h` and not a valid cron expression: {msg}"
                    ),
                })?;
                Ok(Self::Cron(s.to_string()))
            }
        }
    }

    /// Canonical string stored in metadata (compact humanized, or trimmed cron).
    pub fn normalize(&self) -> String {
        match self {
            Self::Fixed(d) => humanized::format(*d),
            Self::Cron(expr) => expr.split_whitespace().collect::<Vec<_>>().join(" "),
        }
    }

    /// Next fire time strictly after `after`. Cron evaluates in the local
    /// timezone (documented: rules follow wall-clock local time).
    pub fn next_after(&self, after: DateTime<Utc>) -> Result<Option<DateTime<Utc>>> {
        match self {
            Self::Fixed(d) => {
                let step =
                    chrono::Duration::from_std(*d).map_err(|_| CoreError::InvalidSchedule {
                        value: self.normalize(),
                        message: "interval too large".into(),
                    })?;
                Ok(Some(after + step))
            }
            Self::Cron(expr) => cron_next(expr, after).map(Some),
        }
    }
}

fn cron_error(expr: &str, msg: impl std::fmt::Display) -> CoreError {
    CoreError::InvalidSchedule {
        value: expr.into(),
        message: msg.to_string(),
    }
}

fn validate_cron(expr: &str) -> std::result::Result<(), String> {
    expr.parse::<croner::Cron>()
        .map(|_| ())
        .map_err(|e| e.to_string())
}

fn cron_next(expr: &str, after: DateTime<Utc>) -> Result<DateTime<Utc>> {
    use std::str::FromStr as _;
    let cron = croner::Cron::from_str(expr).map_err(|e| cron_error(expr, e))?;
    let local_after: DateTime<Local> = after.into();
    let next = cron
        .find_next_occurrence(&local_after, false)
        .map_err(|e| cron_error(expr, e))?;
    Ok(next.with_timezone(&Utc))
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::TimeZone;

    /// 2026-10-31 12:00:00 UTC (a Saturday).
    fn at(h: u32, m: u32) -> DateTime<Utc> {
        Utc.with_ymd_and_hms(2026, 10, 31, h, m, 0).unwrap()
    }

    #[test]
    fn fixed_intervals() {
        let rule = ScheduleRule::parse("6h").unwrap();
        assert_eq!(rule, ScheduleRule::Fixed(Duration::from_secs(6 * 3_600)));
        assert_eq!(rule.normalize(), "6h");
        assert_eq!(rule.next_after(at(12, 0)).unwrap(), Some(at(18, 0)));

        let rule = ScheduleRule::parse("1d 12h 30m").unwrap();
        assert_eq!(rule.normalize(), "1d12h30m");
        assert_eq!(
            rule.next_after(at(0, 0)).unwrap(),
            Some(Utc.with_ymd_and_hms(2026, 11, 1, 12, 30, 0).unwrap())
        );
    }

    #[test]
    fn cron_5_field() {
        let rule = ScheduleRule::parse("0 4 * * *").unwrap();
        assert_eq!(rule, ScheduleRule::Cron("0 4 * * *".into()));
        assert_eq!(rule.normalize(), "0 4 * * *");
        // next occurrence after noon is tomorrow at 4am local
        let next = rule.next_after(at(12, 0)).unwrap().unwrap();
        assert!(next > at(12, 0));
    }

    #[test]
    fn cron_fire_is_once_per_window() {
        // Monotonic query: next_after from the fired time never returns the
        // same instant again.
        let rule = ScheduleRule::parse("*/15 * * * *").unwrap();
        let mut t = at(3, 0);
        let mut seen = Vec::new();
        for _ in 0..5 {
            t = rule.next_after(t).unwrap().unwrap();
            seen.push(t);
        }
        let unique: std::collections::HashSet<_> = seen.iter().collect();
        assert_eq!(unique.len(), 5);
        assert!(seen.windows(2).all(|w| w[1] > w[0]));
    }

    #[test]
    fn invalid_rules() {
        assert!(matches!(
            ScheduleRule::parse("nonsense"),
            Err(CoreError::InvalidSchedule { .. })
        ));
        assert!(matches!(
            ScheduleRule::parse(""),
            Err(CoreError::InvalidSchedule { .. })
        ));
        assert!(matches!(
            ScheduleRule::parse("0d"),
            Err(CoreError::InvalidSchedule { message, .. }) if message.contains("greater than zero")
        ));
        assert!(matches!(
            ScheduleRule::parse("99 99 * * *"),
            Err(CoreError::InvalidSchedule { .. })
        ));
    }

    #[test]
    fn error_hint_is_actionable() {
        let err = ScheduleRule::parse("weekly").unwrap_err();
        assert!(err.hint().unwrap().contains("cron"));
    }
}
