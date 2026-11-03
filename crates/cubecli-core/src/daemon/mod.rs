//! The daemon's decision engine — pure, exhaustive-tested: given project
//! snapshots and persisted state, decide what actions to take. Execution
//! (process control, RCON, zips) lives in the bin's tick loop.

pub mod engine;
pub mod state;

pub use engine::{
    Action, ProjectSnapshot, cron_due, decide, state_after_restart, update_state_after_tick,
};
pub use state::DaemonState;
