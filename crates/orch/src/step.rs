use crate::event::{Effect, Event, Reject};
use crate::state::OrchState;
use crate::{delegation, outbox, run};

pub fn step(s: &OrchState, e: Event) -> Result<(OrchState, Vec<Effect>), Reject> {
    let mut next = s.clone();
    let mut fx = Vec::new();
    match e {
        Event::Run(e) => run::apply(&mut next, e, &mut fx)?,
        Event::Del(e) => delegation::apply(&mut next, e, &mut fx)?,
        Event::Outbox(e) => outbox::apply(&mut next, e, &mut fx)?,
    }
    Ok((next, fx))
}
