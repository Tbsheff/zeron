#[path = "mbt/abstraction.rs"]
mod abstraction;
#[path = "mbt/events.rs"]
mod events;
#[path = "mbt/fixture.rs"]
mod fixture;
#[path = "mbt/universe.rs"]
mod universe;

use std::collections::{BTreeMap, BTreeSet};

use serde_json::{Map, Value};
use zeron_orch::{DelEvent, Event, OrchState, Reject, check_invariants, step, well_formed};

use abstraction::abstract_state;
use events::{Ctx, events, exempt};
use fixture::{Fixture, Module, fixture_paths};

const BUDGETS: [&str; 9] = [
    "personLeft",
    "serverLeft",
    "stopLeft",
    "ulLeft",
    "crashLeft",
    "failLeft",
    "abandonLeft",
    "narrowLeft",
    "interruptLeft",
];

#[derive(Default)]
struct Coverage {
    accepted: BTreeMap<String, usize>,
    rejected: BTreeMap<String, BTreeMap<Reject, usize>>,
    exempt: BTreeMap<String, usize>,
    states: usize,
    edges: usize,
}

fn init_state(fx: &Fixture) -> OrchState {
    let root = fx.threads[0].1;
    let open = Event::Del(DelEvent::OpenRoot {
        t: root,
        access: 1,
        reconfigure: true,
    });
    step(&OrchState::new(fx.config()), open)
        .expect("open root")
        .0
}

fn run_seq(s: &OrchState, evs: &[Event]) -> Result<OrchState, Reject> {
    let mut cur = s.clone();
    for e in evs {
        cur = step(&cur, *e)?.0;
    }
    Ok(cur)
}

fn mapped(spec: &Map<String, Value>) -> Map<String, Value> {
    spec.iter()
        .filter(|(k, _)| !BUDGETS.contains(&k.as_str()))
        .map(|(k, v)| (k.clone(), v.clone()))
        .collect()
}

fn diff(want: &Map<String, Value>, got: &Map<String, Value>) -> String {
    let mut out = String::new();
    for (k, v) in want {
        if got.get(k) != Some(v) {
            out.push_str(&format!(
                "\n  {k}: spec {v} rust {}",
                got.get(k).unwrap_or(&Value::Null)
            ));
        }
    }
    out
}

fn check_matches(fx: &Fixture, node: usize, s: &OrchState, context: &str) {
    let want = mapped(&fx.spec_state(node));
    let got = abstract_state(fx, s);
    assert_eq!(
        want.len(),
        got.len(),
        "{}: unmapped vars at {context}",
        fx.name
    );
    assert!(
        want == got,
        "{}: abstraction differs at {context}:{}",
        fx.name,
        diff(&want, &got)
    );
    check_invariants(s).unwrap_or_else(|v| panic!("{}: {v:?} at {context}", fx.name));
    well_formed(s).unwrap_or_else(|m| panic!("{}: {m:?} at {context}", fx.name));
}

fn label(act: &fixture::Action) -> String {
    format!("{}{}", act.name, Value::Array(act.params.clone()))
}

fn replay(fx: &Fixture, cov: &mut Coverage) {
    let mut out: Vec<Vec<(usize, usize)>> = vec![Vec::new(); fx.states.len()];
    for [from, to, a] in &fx.edges {
        out[*from].push((*to, *a));
    }
    let mut seen = vec![false; fx.states.len()];
    let init = init_state(fx);
    check_matches(fx, fx.init, &init, "init");
    let mut stack = vec![(fx.init, init)];
    let mut msg = 1_u64;
    while let Some((node, s)) = stack.pop() {
        if std::mem::replace(&mut seen[node], true) {
            continue;
        }
        cov.states += 1;
        for &(to, a) in &out[node] {
            msg += 1;
            let next = accept(fx, node, &s, to, a, msg, cov);
            if !seen[to] {
                stack.push((to, next));
            }
        }
        let enabled: BTreeSet<usize> = out[node].iter().map(|(_, a)| *a).collect();
        refuse_disabled(fx, node, &s, &enabled, cov);
    }
    assert!(
        seen.iter().all(|x| *x),
        "{}: unreachable fixture states",
        fx.name
    );
}

fn accept(
    fx: &Fixture,
    node: usize,
    s: &OrchState,
    to: usize,
    a: usize,
    msg: u64,
    cov: &mut Coverage,
) -> OrchState {
    let evs = events(&Ctx { fx, s, msg }, &fx.actions[a]);
    let next = run_seq(s, &evs).unwrap_or_else(|r| {
        panic!(
            "{}: spec enables {} at state {node} but Rust rejects {r:?}",
            fx.name,
            label(&fx.actions[a])
        )
    });
    check_matches(
        fx,
        to,
        &next,
        &format!("{} from state {node}", label(&fx.actions[a])),
    );
    *cov.accepted.entry(fx.actions[a].name.clone()).or_default() += 1;
    cov.edges += 1;
    next
}

fn refuse_disabled(
    fx: &Fixture,
    node: usize,
    s: &OrchState,
    enabled: &BTreeSet<usize>,
    cov: &mut Coverage,
) {
    let spec = fx.spec_state(node);
    for (a, act) in universe::candidates(fx).iter().enumerate() {
        if enabled.contains(&a) {
            continue;
        }
        if exempt(fx, &spec, act) {
            *cov.exempt.entry(act.name.clone()).or_default() += 1;
            continue;
        }
        let evs = events(&Ctx { fx, s, msg: 0 }, act);
        match run_seq(s, &evs) {
            Err(r) => {
                *cov.rejected
                    .entry(act.name.clone())
                    .or_default()
                    .entry(r)
                    .or_default() += 1
            }
            Ok(_) => panic!(
                "{}: spec disables {} at state {node} but Rust accepts",
                fx.name,
                label(act)
            ),
        }
    }
}

fn report(module: Module, cov: &Coverage) -> Vec<String> {
    let mut lines = vec![format!(
        "{module:?}: {} states, {} edges replayed",
        cov.states, cov.edges
    )];
    let names: BTreeSet<&String> = cov.accepted.keys().chain(cov.rejected.keys()).collect();
    for n in names {
        let ok = cov.accepted.get(n).copied().unwrap_or(0);
        let rejects = cov.rejected.get(n).map(|m| {
            m.iter()
                .map(|(r, c)| format!("{r:?}={c}"))
                .collect::<Vec<_>>()
        });
        let ex = cov.exempt.get(n).copied().unwrap_or(0);
        lines.push(format!(
            "  {n}: true={ok} false={} exempt={ex}",
            rejects.map_or("-".into(), |r| r.join(","))
        ));
    }
    lines
}

#[test]
fn mbt_replays_every_edge_and_refuses_every_disabled_action() {
    let mut by_module: BTreeMap<String, (Module, Coverage)> = BTreeMap::new();
    let paths = fixture_paths();
    assert!(!paths.is_empty(), "no MBT fixtures");
    for path in paths {
        let fx = Fixture::load(&path);
        let entry = by_module
            .entry(format!("{:?}", fx.module))
            .or_insert_with(|| (fx.module, Coverage::default()));
        replay(&fx, &mut entry.1);
    }
    let mut all_rejects: BTreeSet<Reject> = BTreeSet::new();
    for (module, cov) in by_module.values() {
        for line in report(*module, cov) {
            println!("{line}");
        }
        for (name, rejects) in &cov.rejected {
            assert!(
                cov.accepted.contains_key(name),
                "{module:?}: {name} never accepted"
            );
            all_rejects.extend(rejects.keys().copied());
        }
        for name in cov.accepted.keys() {
            assert!(
                cov.rejected.contains_key(name) || cov.exempt.contains_key(name),
                "{module:?}: {name} never refused"
            );
        }
    }
    let unseen: Vec<Reject> = Reject::ALL
        .iter()
        .copied()
        .filter(|r| !all_rejects.contains(r))
        .collect();
    println!("Reject variants never seen in MBT: {unseen:?}");
}
