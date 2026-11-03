//! Java runtime discovery (owned JDKs → sdkman → system PATH).

pub mod scan;

pub use scan::FsJavaLocator;
