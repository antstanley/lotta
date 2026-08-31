//! Shared collision-free model identifier resolution.

use std::collections::{BTreeMap, BTreeSet};

use lotta_domain::Agent;
use lotta_store::{AGENTS_MAX, query::QUERY_PAGE_ITEMS_MAX};

use crate::{errors::AppServerError, ws::agents::AgentsBridge};

/// Largest model identifier accepted by a future `OpenAI` request decoder.
pub const OPENAI_MODEL_ID_BYTES_MAX: usize = 8 * 1_024 * 1_024;
/// Maximum repository pages needed to inspect the bounded complete agent set.
const VISIBLE_AGENT_PAGES_MAX: usize = AGENTS_MAX.div_ceil(QUERY_PAGE_ITEMS_MAX);

/// Loads every visible agent in deterministic identifier order.
///
/// # Errors
/// Returns a scrubbed availability error when canonical repository reads fail.
///
/// # Panics
/// Panics only if the repository returns more than its canonical agent bound.
pub async fn visible_agents(bridge: &AgentsBridge) -> Result<Vec<Agent>, AppServerError> {
    let mut agents = Vec::new();
    let mut after = None;
    for _ in 0..VISIBLE_AGENT_PAGES_MAX {
        let page = bridge.visible_page(after).await?;
        agents.extend(page.items);
        assert!(agents.len() <= AGENTS_MAX);
        match page.next {
            Some(next) => after = Some(next),
            None => return Ok(agents),
        }
    }
    assert_eq!(agents.len(), AGENTS_MAX);
    Ok(agents)
}

/// Returns the collision-free advertised identifier for every visible agent.
///
/// # Panics
/// Panics when the caller supplies more than the canonical agent bound.
#[must_use]
pub fn advertised_ids(agents: &[Agent]) -> Vec<String> {
    assert!(agents.len() <= AGENTS_MAX);
    let ids: BTreeSet<&str> = agents.iter().map(|agent| agent.id.as_str()).collect();
    let mut counts = BTreeMap::<&str, usize>::new();
    for agent in agents {
        let count = counts.entry(agent.name.as_str()).or_default();
        *count += 1;
    }
    agents
        .iter()
        .map(|agent| advertised_id(agent, &ids, &counts))
        .collect()
}

fn advertised_id(agent: &Agent, ids: &BTreeSet<&str>, counts: &BTreeMap<&str, usize>) -> String {
    let name = agent.name.as_str();
    if counts.get(name) == Some(&1) && !ids.contains(name) {
        name.to_owned()
    } else {
        agent.id.as_str().to_owned()
    }
}

/// Resolves raw agent IDs first, then one unambiguous advertised name.
#[must_use]
pub fn resolve<'a>(agents: &'a [Agent], model: &str) -> Option<&'a Agent> {
    if model.len() > OPENAI_MODEL_ID_BYTES_MAX || agents.len() > AGENTS_MAX {
        return None;
    }
    if let Some(agent) = agents.iter().find(|agent| agent.id.as_str() == model) {
        return Some(agent);
    }
    let mut matches = agents.iter().filter(|agent| agent.name.as_str() == model);
    let agent = matches.next()?;
    if matches.next().is_some() {
        return None;
    }
    Some(agent)
}
