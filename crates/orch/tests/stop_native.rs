mod common;
use common::{H, R, del};
use zeron_orch::{DelEvent, Delivery, Effect, Event, Outcome};
use zeron_orch::native::{NativeEvent, NativeStatus};

#[test]
fn stop_parked_background_native_emits_runtime_interrupt() {
    let mut h = H::new();
    h.live(R);
    h.ok(Event::Native(NativeEvent::Start { parent: R, key: 1, background: true }));
    h.end(R, Outcome::Completed);
    assert!(!h.th(R).is_live());
    assert_eq!(h.s.natives[&(R, 1)].status, NativeStatus::Running);
    let run = h.th(R).run_id();
    let fx = h.ok(del(DelEvent::Stop { t: R }));
    assert!(fx.contains(&Effect::InterruptRun { run }));
    assert_eq!(h.s.natives[&(R, 1)].status, NativeStatus::Running);
    assert_eq!(h.s.natives[&(R, 1)].delivery, Delivery::Pending);
}

#[test]
fn stop_parked_completed_native_does_not_interrupt_runtime() {
    let mut h = H::new();
    h.live(R);
    h.ok(Event::Native(NativeEvent::Start { parent: R, key: 1, background: true }));
    h.ok(Event::Native(NativeEvent::End { parent: R, key: 1, failed: false }));
    h.end(R, Outcome::Completed);
    let fx = h.ok(del(DelEvent::Stop { t: R }));
    assert!(!fx.iter().any(|effect| matches!(effect, Effect::InterruptRun { .. })));
}
