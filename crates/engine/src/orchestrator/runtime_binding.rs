#[derive(Clone)]
pub struct AgentRuntime {
    pub runtime_id: String,
    pub attempt: zeron_orch::AttemptId,
    pub effective_access: Option<u8>,
    pub origin_id: Option<String>,
    pub source_id: Option<String>,
    pub source_revision: Option<u64>,
}

use serde::Serialize;

#[derive(Clone, Serialize)]
pub(crate) struct RuntimeBinding {
    pub chat: String,
    pub runtime_id: String,
    pub run: Option<u32>,
    pub attempt: Option<u32>,
    pub effective_access: u8,
    pub origin_id: String,
    pub operation: &'static str,
    pub source_id: String,
    pub source_revision: u64,
    pub validated: bool,
}

pub(crate) fn record(driver: &super::Driver, binding: &RuntimeBinding) -> Result<(), super::DriverError> {
    driver.observe_mechanism(
        &format!("runtime-binding:{}:{}", binding.source_id, binding.source_revision),
        "runtime_binding",
        serde_json::to_value(binding)?,
    )
}
