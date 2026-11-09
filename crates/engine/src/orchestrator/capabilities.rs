use serde::Serialize;
use std::collections::BTreeMap;
use zeron_harness::Harness;

use super::{Driver, DriverError};

#[derive(Serialize)]
struct ProviderCapability<'a> {
    provider_id: &'a str,
    accepts_handoff: bool,
    native_fork: bool,
    inject_history: bool,
    person_selectable: bool,
    delivery_path: &'static str,
    handoff_scope: &'static str,
    declared_native_fork: bool,
    declared_inject_history: bool,
    effective_native_fork: bool,
    effective_native_injection: bool,
    native_resume: bool,
    supports_steering: bool,
    supports_plan: bool,
    wakes_on_background: bool,
    stops_turn_in_place: bool,
    steer_stops_tool: bool,
    source: &'static str,
    field_sources: BTreeMap<&'static str, &'static str>,
}

#[derive(Default, Serialize)]
struct FinitePolicy {
    forks: Option<bool>,
    spawns: Option<bool>,
    cuts: Option<bool>,
    budget_fail: Option<bool>,
    questions: Option<bool>,
    split_questions: Option<bool>,
    stop_check_anc: Option<bool>,
    late_ack: Option<bool>,
    partial_crash: Option<bool>,
    fix_limit_release: Option<bool>,
    fix_wake_skips_fail_hold: Option<bool>,
    tool_nodes: Option<bool>,
    in_place_access: Option<Vec<String>>,
}

#[derive(Serialize)]
struct CapabilityCatalog<'a> {
    scope: &'static str,
    complete: bool,
    providers: Vec<ProviderCapability<'a>>,
    policy: FinitePolicy,
    policy_sources: BTreeMap<&'static str, &'static str>,
    core_config: zeron_orch::Config,
    validated: bool,
}

pub(crate) fn record_selected(
    driver: &Driver,
    harness: &dyn Harness,
    person_selectable: bool,
) -> Result<(), DriverError> {
    if !driver.mechanism_capture_enabled() {
        return Ok(());
    }
    let id = harness.id();
    let provider = ProviderCapability {
        provider_id: id.as_str(),
        accepts_handoff: true,
        native_fork: false,
        inject_history: false,
        person_selectable,
        delivery_path: "portable_prompt",
        handoff_scope: "normal_prompt",
        declared_native_fork: harness.native_fork(),
        declared_inject_history: harness.inject_history(),
        effective_native_fork: false,
        effective_native_injection: false,
        native_resume: harness.native_resume(),
        supports_steering: harness.supports_steering(),
        supports_plan: harness.supports_plan(),
        wakes_on_background: harness.wakes_on_background(),
        stops_turn_in_place: harness.stops_turn_in_place(),
        steer_stops_tool: harness.steer_stops_tool(),
        source: "sessions.dispatch_inner:resolved_harness",
        field_sources: BTreeMap::from([
            ("accepts_handoff", "sessions.drive_run:prepare_run_for_normal_prompt"),
            ("native_fork", "orchestrator.transfer.prepare_run:portable_prompt_only"),
            ("inject_history", "orchestrator.transfer.prepare_run:portable_prompt_only"),
            ("person_selectable", "HarnessRegistry.enabled_set:selected_provider_membership"),
            ("declared_native_fork", "Harness.native_fork"),
            ("declared_inject_history", "Harness.inject_history"),
            ("other_traits", "selected_Harness_trait_methods"),
        ]),
    };
    let policy_sources = [
        "forks", "spawns", "cuts", "budget_fail", "questions", "split_questions",
        "stop_check_anc", "late_ack", "partial_crash", "fix_limit_release",
        "fix_wake_skips_fail_hold", "tool_nodes", "in_place_access",
    ].into_iter().map(|field| (field, if field == "in_place_access" {
        "unknown:Harness.reconfigures_in_place_requires_actual_request_pair"
    } else {
        "unknown:finite_model_parameter_has_no_runtime_config_field"
    })).collect();
    let catalog = CapabilityCatalog {
        scope: "selected_harnesses",
        complete: true,
        providers: vec![provider],
        policy: FinitePolicy::default(),
        policy_sources,
        core_config: driver.state()?.config,
        validated: true,
    };
    let data = serde_json::to_value(catalog)?;
    use sha2::{Digest, Sha256};
    let digest = Sha256::digest(serde_json::to_vec(&data)?);
    driver.observe_mechanism(&format!("capability-catalog:{digest:x}"), "capability_catalog", data)
}
