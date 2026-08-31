//! Visible-agent model advertisement and shared resolution.

use std::collections::{BTreeMap, BTreeSet};

use lotta_domain::Agent;
use lotta_store::{AGENTS_MAX, query::QUERY_PAGE_ITEMS_MAX};

use crate::{errors::AppServerError, ws::agents::AgentsBridge};

use super::errors::{ErrorEnvelope, model_not_found};

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

/// Returns collision-free advertised identifiers for visible agents only.
///
/// # Panics
/// Panics when the caller supplies more than the canonical agent bound.
#[must_use]
pub fn advertised_ids(agents: &[Agent]) -> Vec<String> {
    let visible = agents
        .iter()
        .filter(|agent| is_visible(agent))
        .collect::<Vec<_>>();
    advertised_ids_for_visible(&visible)
}

/// Resolves a raw ID or one unambiguous name among visible agents only.
///
/// Missing and hidden models produce the exact shared `model_not_found` contract.
/// Request-size bounds belong at the future model-taking HTTP decoder trust boundary.
///
/// # Errors
/// Returns the stable `OpenAI` missing-model envelope when no visible agent resolves.
///
/// # Panics
/// Panics when the caller supplies more than the canonical agent bound.
pub fn resolve<'a>(agents: &'a [Agent], model: &str) -> Result<&'a Agent, ErrorEnvelope> {
    assert!(agents.len() <= AGENTS_MAX);
    resolve_visible(agents, model).ok_or_else(|| model_not_found(model))
}

fn advertised_ids_for_visible(agents: &[&Agent]) -> Vec<String> {
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

fn resolve_visible<'a>(agents: &'a [Agent], model: &str) -> Option<&'a Agent> {
    let visible = agents.iter().filter(|agent| is_visible(agent));
    if let Some(agent) = visible.clone().find(|agent| agent.id.as_str() == model) {
        return Some(agent);
    }
    let mut matches = visible.filter(|agent| agent.name.as_str() == model);
    let agent = matches.next()?;
    matches.next().is_none().then_some(agent)
}

fn is_visible(agent: &Agent) -> bool {
    !agent.hidden.flatten().unwrap_or(false)
}
