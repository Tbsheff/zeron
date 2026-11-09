pub mod driver;
pub mod fake;
pub mod store;
pub mod trace;

#[cfg(test)]
mod faults_tests;
#[cfg(test)]
mod trace_tests;

pub use driver::{ApplyResult, Driver, EffectSink, FaultPoint, SinkError, effect_id};
pub use fake::RecordingSink;
pub use store::{Applied, OutboxRow};

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
