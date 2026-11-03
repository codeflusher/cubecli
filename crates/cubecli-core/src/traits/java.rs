//! Java runtime discovery and provisioning.
//!
//! Resolution order: owned JDKs (`~/.cubecli/jdks/<feature>`) → sdkman →
//! system PATH (warned as an unrecommended fallback).

use std::path::PathBuf;

use async_trait::async_trait;

use super::progress::ProgressReporter;
use crate::error::Result;
use crate::model::JavaRequirement;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum JavaOrigin {
    /// Downloaded and owned by cubecli.
    Owned,
    /// Found in ~/.sdkman/candidates/java.
    Sdkman,
    /// Found on PATH — unrecommended, may not match the required version.
    System,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct JavaInstall {
    pub feature: u32,
    pub origin: JavaOrigin,
    /// JAVA_HOME.
    pub home: PathBuf,
    /// Path to the `java` binary.
    pub java_bin: PathBuf,
}

/// Sync filesystem scan — cheap, callable anywhere.
pub trait JavaLocator: Send + Sync {
    fn locate(&self, requirement: JavaRequirement) -> Result<JavaInstall>;

    /// All discovered runtimes (for `cubecli setup java list`).
    fn all(&self) -> Vec<JavaInstall>;
}

/// The async half: download + install a missing JDK.
#[async_trait]
pub trait JavaProvisioner: Send + Sync {
    async fn ensure(
        &self,
        requirement: JavaRequirement,
        progress: &dyn ProgressReporter,
    ) -> Result<JavaInstall>;
}
