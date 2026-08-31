//! `GET /v1/models` OpenAI list projection.

use axum::Json;
use lotta_domain::{Agent, Timestamp};
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

fn project(agents: &[Agent]) -> ModelList {
    let visible = agents
        .iter()
        .filter(|agent| !agent.hidden.flatten().unwrap_or(false))
        .collect::<Vec<_>>();
    let advertised = advertised_ids(agents);
    assert_eq!(visible.len(), advertised.len());
    let data = visible
        .into_iter()
        .zip(advertised)
        .take(OPENAI_MODELS_ITEMS_MAX)
        .map(|(agent, id)| model(agent, id))
        .collect::<Vec<_>>();
    assert!(data.len() <= OPENAI_MODELS_ITEMS_MAX);
    ModelList {
        object: "list",
        data,
    }
}

fn model(agent: &Agent, id: String) -> Model {
    Model {
        id,
        object: "model",
        created: created_at_seconds(agent),
        owned_by: "letta",
    }
}

fn created_at_seconds(agent: &Agent) -> i64 {
    agent
        .extras
        .get("created_at")
        .and_then(serde_json::Value::as_str)
        .and_then(|value| Timestamp::parse_persisted_rfc3339(value).ok())
        .map_or(0, |timestamp| timestamp.as_utc().timestamp())
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;

    use lotta_domain::EntityExtras;
    use serde_json::{Value, json};

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

    fn with_created_at(value: Option<Value>) -> lotta_domain::Agent {
        let mut result = agent("agent-local-created", "created-model", false);
        let values = value
            .map(|value| BTreeMap::from([("created_at".to_owned(), value)]))
            .unwrap_or_default();
        result.extras = EntityExtras::new(values, &[])
            .unwrap_or_else(|error| panic!("created_at extras: {error}"));
        result
    }

    fn encoded_created(value: Option<Value>) -> String {
        serde_json::to_string(&project(&[with_created_at(value)]))
            .unwrap_or_else(|error| panic!("models JSON: {error}"))
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

    #[test]
    fn valid_created_at_has_exact_json_and_unix_seconds() {
        assert_eq!(
            encoded_created(Some(json!("2026-08-14T12:34:56Z"))),
            concat!(
                r#"{"object":"list","data":[{"id":"created-model","object":"model","#,
                r#""created":1786710896,"owned_by":"letta"}]}"#
            )
        );
    }

    #[test]
    fn absent_created_at_has_exact_zero_json() {
        assert_eq!(
            encoded_created(None),
            concat!(
                r#"{"object":"list","data":[{"id":"created-model","object":"model","#,
                r#""created":0,"owned_by":"letta"}]}"#
            )
        );
    }

    #[test]
    fn invalid_created_at_has_exact_zero_json() {
        assert_eq!(
            encoded_created(Some(json!("not-a-timestamp"))),
            concat!(
                r#"{"object":"list","data":[{"id":"created-model","object":"model","#,
                r#""created":0,"owned_by":"letta"}]}"#
            )
        );
    }
}
