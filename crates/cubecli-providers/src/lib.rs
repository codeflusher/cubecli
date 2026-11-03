//! HTTP providers for server cores, addons, and JDKs.
//!
//! Every download URL comes verbatim from an API response — URLs are never
//! constructed client-side (Paper's are content-addressed by sha256).

pub mod adoptium;
pub mod cf_categories;
pub mod checksum;
pub mod curseforge;
pub mod download;
pub mod error;
pub mod fabric;
pub mod forge;
pub mod http;
pub mod modrinth;
pub mod mojang;
pub mod neoforge;
pub mod paper;
pub mod purpur;
pub mod registry;
pub mod version_sort;

pub use error::{ProviderError, Result};
pub use registry::{AddonRegistry, CoreRegistry};
