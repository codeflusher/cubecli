//! Cross-crate contracts. Concrete implementations live in `cubecli-providers`
//! (cores, addons, JDKs) and `cubecli-runtime` (backends); the `cubecli` bin
//! is the composition root that wires them together.

pub mod addon_source;
pub mod java;
pub mod progress;
pub mod runtime;
pub mod server_core;

pub use addon_source::{
    AddonFileEntry, AddonProject, AddonQuery, AddonSource, DownloadedAddon, IdentifiedFile,
    ServerSide,
};
pub use java::{JavaInstall, JavaLocator, JavaOrigin, JavaProvisioner};
pub use progress::{NoopProgress, ProgressReporter, TracingProgress};
pub use runtime::{RuntimeBackend, RuntimeId, RuntimeStatus, StopGrace, StopOutcome, StopStage};
pub use server_core::{
    DownloadSpec, InstallPlan, InstalledCore, PostInstallStep, ServerCoreProvider,
};
