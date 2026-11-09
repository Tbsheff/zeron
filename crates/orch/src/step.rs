use crate::event::{Effect, Event, Reject};
use crate::state::OrchState;
use crate::{delegation, outbox, run};

pub fn step(s: &OrchState, e: Event) -> Result<(OrchState, Vec<Effect>), Reject> {
    let mut next = s.clone();
    let fx = step_in_place(&mut next, e)?;
    Ok((next, fx))
}

pub(crate) fn step_in_place(s: &mut OrchState, e: Event) -> Result<Vec<Effect>, Reject> {
    let fx = apply_event(s, e)?;
    s.retire_runs();
    Ok(fx)
}

pub(crate) fn apply_event(s: &mut OrchState, e: Event) -> Result<Vec<Effect>, Reject> {
    let mut fx = Vec::new();
    match e {
        Event::Run(e) => run::apply(s, e, &mut fx)?,
        Event::Del(e) => delegation::apply(s, e, &mut fx)?,
        Event::Outbox(e) => outbox::apply(s, e, &mut fx)?,
    }
    Ok(fx)
}
