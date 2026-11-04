//! cubecli-core: pure domain layer for cubecli.
//!
//! No HTTP, no sockets, no process spawning — those live in `cubecli-providers`
//! and `cubecli-runtime`. Everything here is testable without I/O beyond tempdirs.

pub mod backup;
pub mod config;
pub mod daemon;
pub mod error;
pub mod java;
pub mod jvm;
pub mod managed_files;
pub mod meta;
pub mod model;
pub mod paths;
pub mod pipeline;
pub mod project;
pub mod properties;
pub mod registry;
pub mod schedule;
pub mod traits;

pub use config::GlobalConfig;
pub use error::{CoreError, Result};
pub use java::FsJavaLocator;
pub use meta::ProjectMeta;
pub use paths::{GlobalLayout, META_FILE, ProjectLayout};
pub use project::ProjectHandle;
