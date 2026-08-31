//! `GET /v1/models` OpenAI list projection.

use axum::Json;
use serde::Serialize;

use crate::{errors::AppServerError, ws::agents::AgentsBridge};

use super::resolve::{advertised_ids, visible_agents};

/// Maximum visible models emitted by one list response.
pub const OPENAI_MODELS_ITEMS_MAX: usize = 1_000;

/// `OpenAI` model list envelope.
#[derive(Debug, Serialize)]
pub struct ModelList {
    object: &'static str,
    data: Vec<Model>,
}

/// `OpenAI` model object projected from one visible agent.
#[derive(Debug, Serialize)]
pub struct Model {
    id: String,
    object: &'static str,
    created: i64,
    owned_by: &'static str,
}

/// Lists visible local agents as deterministic `OpenAI` model objects.
///
/// # Errors
/// Returns a scrubbed availability error when the canonical repository fails.
///
/// # Panics
/// Panics only if the repository violates its bounded-page cardinality contract.
pub async fn list(bridge: &AgentsBridge) -> Result<Json<ModelList>, AppServerError> {
    let agents = visible_agents(bridge).await?;
    Ok(Json(project(&agents)))
}

fn project(agents: &[lotta_domain::Agent]) -> ModelList {
    let advertised = advertised_ids(agents);
    assert_eq!(agents.len(), advertised.len());
    let data = advertised
        .into_iter()
        .take(OPENAI_MODELS_ITEMS_MAX)
        .map(|id| Model {
            id,
            object: "model",
            created: 0,
            owned_by: "letta",
        })
        .collect::<Vec<_>>();
    assert!(data.len() <= OPENAI_MODELS_ITEMS_MAX);
    ModelList {
        object: "list",
        data,
    }
}

#[cfg(test)]
mod tests {
    use super::{OPENAI_MODELS_ITEMS_MAX, project};
    use crate::openai::test_support::agent;

    fn projected_count(agent_count: usize) -> usize {
        let agents = (0..agent_count)
            .map(|index| {
                agent(
                    &format!("agent-local-bound-{index:04}"),
                    &format!("bound-name-{index:04}"),
                    false,
                )
            })
            .collect::<Vec<_>>();
        project(&agents).data.len()
    }

    #[test]
    fn model_cap_below() {
        assert_eq!(projected_count(OPENAI_MODELS_ITEMS_MAX - 1), 999);
    }

    #[test]
    fn model_cap_at() {
        assert_eq!(projected_count(OPENAI_MODELS_ITEMS_MAX), 1_000);
    }

    #[test]
    fn model_cap_above() {
        assert_eq!(projected_count(OPENAI_MODELS_ITEMS_MAX + 1), 1_000);
    }
}
