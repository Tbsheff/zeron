pub(crate) mod capabilities;
pub mod command;
pub mod driver;
pub mod fake;
pub mod mechanism;
pub mod native;
pub mod placement;
pub mod questions;
pub(crate) mod runtime_binding;
mod sink;
pub mod store;
pub mod trace;
pub mod transfer;
pub mod wire;

#[cfg(test)]
mod mechanism_tests;
#[cfg(test)]
mod faults_tests;
#[cfg(test)]
mod trace_tests;

pub use command::{CommandDisposition, CommandReceipt, CommandRefusal, CommandSource};
pub use driver::{ApplyResult, Driver, EffectSink, FaultPoint, SinkError, effect_id};
pub use fake::RecordingSink;
pub use store::{Applied, OutboxRow};
pub use wire::{FLAG_ENV, OrchOptions, OrchWire, flag_from_env};

#[derive(Debug, thiserror::Error)]
pub enum DriverError {
    #[error("sqlite: {0}")]
    Sqlite(#[from] rusqlite::Error),
    #[error("store: {0}")]
    Store(#[from] zeron_sync::StoreError),
    #[error("json: {0}")]
    Json(#[from] serde_json::Error),
    #[error("io: {0}")]
    Io(#[from] std::io::Error),
    #[error("orchestrator store corrupt: {0}")]
    Corrupt(String),
    #[error("effect {0} failed: {1}")]
    Effect(String, String),
    #[error("injected crash at {0:?}")]
    Crashed(FaultPoint),
}
