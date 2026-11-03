//! User-facing error rendering with hints, plus exit-code mapping.

use std::fmt;

use cubecli_core::CoreError;

/// An error prepared for display: a one-line message and an optional hint.
pub struct UserError {
    pub message: String,
    pub hint: Option<String>,
}

impl UserError {
    pub fn new(message: impl Into<String>) -> Self {
        Self {
            message: message.into(),
            hint: None,
        }
    }

    pub fn with_hint(mut self, hint: impl Into<String>) -> Self {
        self.hint = Some(hint.into());
        self
    }

    pub fn print(&self) {
        eprintln!("error: {}", self.message);
        if let Some(hint) = &self.hint {
            eprintln!("  hint: {hint}");
        }
    }
}

impl fmt::Display for UserError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.message)?;
        if let Some(hint) = &self.hint {
            write!(f, " (hint: {hint})")?;
        }
        Ok(())
    }
}

impl From<CoreError> for UserError {
    fn from(err: CoreError) -> Self {
        let mut e = UserError::new(err.to_string());
        if let Some(hint) = err.hint() {
            e = e.with_hint(hint);
        }
        e
    }
}

impl From<cubecli_providers::ProviderError> for UserError {
    fn from(err: cubecli_providers::ProviderError) -> Self {
        Self::from(CoreError::from(err))
    }
}

impl From<cubecli_runtime::RuntimeError> for UserError {
    fn from(err: cubecli_runtime::RuntimeError) -> Self {
        Self::from(CoreError::from(err))
    }
}

impl From<anyhow::Error> for UserError {
    fn from(err: anyhow::Error) -> Self {
        // Reach into the chain for the first CoreError hint, if any.
        let hint = err
            .chain()
            .filter_map(|c| c.downcast_ref::<CoreError>())
            .find_map(CoreError::hint);
        let mut e = UserError::new(err.to_string());
        if let Some(hint) = hint {
            e = e.with_hint(hint);
        }
        e
    }
}

/// Exit codes: 0 ok, 1 runtime error, 130 interrupted.
pub const EXIT_ERROR: i32 = 1;
pub const EXIT_INTERRUPTED: i32 = 130;
