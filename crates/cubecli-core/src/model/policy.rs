use std::fmt;

use serde::{Deserialize, Serialize};

/// Which backend owns the server process lifecycle.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Backend {
    #[default]
    Native,
    Compose,
}

impl Backend {
    pub fn slug(self) -> &'static str {
        match self {
            Self::Native => "native",
            Self::Compose => "compose",
        }
    }

    pub fn parse(s: &str) -> Option<Self> {
        match s.to_ascii_lowercase().as_str() {
            "native" => Some(Self::Native),
            "compose" | "docker" => Some(Self::Compose),
            _ => None,
        }
    }
}

impl fmt::Display for Backend {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.slug())
    }
}

/// Restart policy enforced by the daemon. Explicit `cubecli project stop`
/// always stops, regardless of policy.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum RestartPolicy {
    #[default]
    ManualOnly,
    UnlessStopped,
    Cron,
}

impl RestartPolicy {
    pub fn slug(self) -> &'static str {
        match self {
            Self::ManualOnly => "manual-only",
            Self::UnlessStopped => "unless-stopped",
            Self::Cron => "cron",
        }
    }

    pub fn parse(s: &str) -> Option<Self> {
        match s.to_ascii_lowercase().as_str() {
            "manual-only" | "manual" => Some(Self::ManualOnly),
            "unless-stopped" => Some(Self::UnlessStopped),
            "cron" => Some(Self::Cron),
            _ => None,
        }
    }
}

impl fmt::Display for RestartPolicy {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.slug())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse() {
        assert_eq!(Backend::parse("docker"), Some(Backend::Compose));
        assert_eq!(
            RestartPolicy::parse("UNLESS-STOPPED"),
            Some(RestartPolicy::UnlessStopped)
        );
        assert_eq!(RestartPolicy::parse("bogus"), None);
    }

    #[test]
    fn serde() {
        assert_eq!(
            serde_json::to_string(&RestartPolicy::UnlessStopped).unwrap(),
            "\"unless-stopped\""
        );
    }
}
