use std::collections::BTreeMap;

use zeron_orch::Effect;

use super::driver::{EffectSink, SinkError};

#[derive(Debug, Default, Clone)]
pub struct RecordingSink {
    pub applied: BTreeMap<String, Effect>,
    pub calls: Vec<String>,
    pub fail_next: Option<String>,
}

impl RecordingSink {
    pub fn redeliveries(&self) -> usize {
        self.calls.len() - self.applied.len()
    }
}

impl EffectSink for RecordingSink {
    fn execute(&mut self, effect_id: &str, effect: &Effect) -> Result<(), SinkError> {
        if let Some(reason) = self.fail_next.take() {
            return Err(SinkError(reason));
        }
        self.calls.push(effect_id.to_owned());
        self.applied
            .entry(effect_id.to_owned())
            .or_insert_with(|| effect.clone());
        Ok(())
    }
}
