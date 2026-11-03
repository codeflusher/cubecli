//! cubecli-runtime: process management, RCON, and service installation.
//! Owns the two [`RuntimeBackend`](cubecli_core::traits::runtime::RuntimeBackend)
//! implementations (native process manager, docker compose).

pub mod compose;
pub mod error;
pub mod native;
pub mod rcon;
pub mod service;
pub mod stop;

pub use compose::ComposeBackend;
pub use error::{Result, RuntimeError};
pub use native::NativeBackend;
