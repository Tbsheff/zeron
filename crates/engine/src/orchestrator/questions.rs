use super::{Driver, DriverError};
use zeron_orch::questions::QuestionEvent;
use zeron_orch::{Effect, Event, OrchState, ThreadId};
use zeron_proto::{UserInputAnswer, UserInputQuestion};

pub fn request_id(driver: &Driver, child: ThreadId, request: &str) -> Result<u64, DriverError> {
    let (qid, _) = driver.intern(&format!("question:{child:?}:{request}"), "question-counter")?;
    driver.put_meta(&format!("question-id:{qid}"), request)?;
    Ok(qid)
}
pub fn raised(
    driver: &Driver,
    child: ThreadId,
    request: &str,
    questions: &[UserInputQuestion],
) -> Result<(), DriverError> {
    if !driver.state()?.thread(child).is_ok_and(|t| t.is_live()) {
        return Ok(());
    }
    let qid = request_id(driver, child, request)?;
    driver.put_meta(
        &format!("question-body:{qid}"),
        &serde_json::to_string(questions)?,
    )?;
    driver.apply(
        &format!("question-raise:{child:?}:{qid}"),
        Event::Question(QuestionEvent::Raise { child, qid }),
    )?;
    Ok(())
}
pub fn answer(
    driver: &Driver,
    agent: Option<ThreadId>,
    child: ThreadId,
    qid: u64,
    epoch: u32,
    answers: &[UserInputAnswer],
) -> Result<(), DriverError> {
    let body = serde_json::to_string(answers)?;
    let (answer, _) = driver.intern(
        &format!("question-answer:{child:?}:{qid}:{epoch}:{body}"),
        "question-answer-counter",
    )?;
    driver.put_meta(&format!("question-answer-body:{answer}"), &body)?;
    let event = match agent {
        Some(agent) => QuestionEvent::AgentAnswer {
            agent,
            child,
            qid,
            epoch,
            answer,
        },
        None => QuestionEvent::PersonAnswer {
            child,
            qid,
            epoch,
            answer,
        },
    };
    driver.apply(
        &format!("question-answer:{child:?}:{qid}:{epoch}:{agent:?}:{answer}"),
        Event::Question(event),
    )?;
    Ok(())
}
pub fn resolved(
    driver: &Driver,
    child: ThreadId,
    request: &str,
    answers: &[UserInputAnswer],
) -> Result<(), DriverError> {
    let Some(qid) = driver.meta(&format!("question:{child:?}:{request}"))? else {
        return Ok(());
    };
    let qid = qid
        .parse()
        .map_err(|_| DriverError::Corrupt("invalid question id".into()))?;
    let s = driver.state()?;
    if let Some(q) = s.questions.get(&(child, qid)) {
        if q.pending {
            answer(driver, None, child, qid, q.epoch, answers)?;
        }
    }
    Ok(())
}
pub fn response(
    driver: &Driver,
    effect: &Effect,
) -> Result<Option<(String, Vec<UserInputAnswer>)>, DriverError> {
    let Effect::RespondInput { qid, answer, .. } = effect else {
        return Ok(None);
    };
    let request = driver
        .meta(&format!("question-id:{qid}"))?
        .ok_or_else(|| DriverError::Corrupt("missing Input request id".into()))?;
    let body = driver
        .meta(&format!("question-answer-body:{answer}"))?
        .ok_or_else(|| DriverError::Corrupt("missing Input answer".into()))?;
    Ok(Some((request, serde_json::from_str(&body)?)))
}
pub fn prompt(driver: &Driver, state: &OrchState, viewer: ThreadId) -> Result<String, DriverError> {
    let mut parts = Vec::new();
    for q in state
        .questions
        .values()
        .filter(|q| q.pending && q.holder == Some(viewer))
    {
        let body = driver
            .meta(&format!("question-body:{}", q.qid))?
            .unwrap_or_default();
        parts.push(format!(
            "Child {:?} asks Input {} at epoch {}: {}",
            q.child, q.qid, q.epoch, body
        ));
    }
    Ok(parts.join("\n"))
}
pub fn next(state: &OrchState, t: ThreadId) -> Option<Event> {
    let th = state.thread(t).ok()?;
    if !th.is_live()
        && th.held == zeron_orch::Hold::None
        && !zeron_orch::questions::blocked(state, t)
        && state
            .questions
            .values()
            .any(|q| q.pending && q.holder == Some(t) && q.views.contains(&t))
    {
        Some(Event::Question(QuestionEvent::Start { t }))
    } else {
        None
    }
}
