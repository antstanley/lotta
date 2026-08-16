use super::*;
use crate::{
    pipeline::{RawToolExecutionRequest, RawToolOutcome},
    registry::ToolRegistry,
};
use lotta_domain::BoundedJsonValue;
use lotta_runtime::ports::{ModelFacingToolName, ValidatedToolInput};
use serde_json::json;
use tokio_util::sync::CancellationToken;

pub(super) async fn registered_names_case() {
    let port = Arc::new(PlanningPort::new());
    let registrations = registrations(Arc::clone(&port)).unwrap();
    assert_eq!(registrations.len(), 2);
    let plan_executor = registrations[0].executor.clone();
    let registry = ToolRegistry::new(registrations).unwrap();
    registry
        .update(crate::ToolsetId::Codex, &[], Some(&["update_plan"]))
        .unwrap();
    let pascal = registry
        .snapshot()
        .unwrap()
        .by_model("UpdatePlan")
        .unwrap()
        .clone();
    registry
        .update(crate::ToolsetId::CodexSnake, &[], Some(&["update_plan"]))
        .unwrap();
    let snake = registry
        .snapshot()
        .unwrap()
        .by_model("update_plan")
        .unwrap()
        .clone();
    assert!(Arc::ptr_eq(&pascal.executor, &snake.executor));
    assert!(Arc::ptr_eq(&plan_executor, &snake.executor));
    execute(
        &snake,
        json!({"explanation":"bounded","plan":[{"step":"Ship","status":"in_progress"}]}),
    )
    .await;
    assert_eq!(port.plan().unwrap()[0].step, "Ship");
    assert_eq!(
        crate::rows()
            .iter()
            .filter(|row| row.internal == "update_plan")
            .count(),
        2
    );
}

#[test]
fn registered_descriptions_are_exact_asset_bytes() {
    let registrations = registrations(Arc::new(PlanningPort::new())).unwrap();
    let plan = registrations
        .iter()
        .find(|item| item.definition.internal_name.as_str() == "update_plan")
        .unwrap();
    let todos = registrations
        .iter()
        .find(|item| item.definition.internal_name.as_str() == "TodoWrite")
        .unwrap();
    assert_eq!(
        plan.definition.description.as_str().as_bytes(),
        include_bytes!("assets/descriptions/UpdatePlan.md")
    );
    assert_eq!(
        todos.definition.description.as_str().as_bytes(),
        include_bytes!("assets/descriptions/TodoWrite.md")
    );
}

#[tokio::test]
async fn todo_write_replaces_shared_state_and_bounds() {
    let port = Arc::new(PlanningPort::new());
    let registrations = registrations(Arc::clone(&port)).unwrap();
    let todo = registrations
        .iter()
        .find(|item| item.definition.internal_name.as_str() == "TodoWrite")
        .unwrap();
    let registered = crate::registry::RegisteredTool {
        definition: todo.definition.clone(),
        model_name: ModelFacingToolName::new("TodoWrite".into()).unwrap(),
        executor: todo.executor.clone(),
    };
    execute(
        &registered,
        json!({"todos":[{"content":"Test","status":"pending","activeForm":"Testing"}]}),
    )
    .await;
    assert_eq!(port.todos().unwrap()[0].content, "Test");
    let exact = vec![
        PlanStep {
            step: "x".into(),
            status: PlanStatus::Pending
        };
        PLANNING_ITEMS_MAX
    ];
    assert!(
        validate_plan(&UpdatePlanInput {
            explanation: None,
            plan: exact
        })
        .is_ok()
    );
    let above = vec![
        PlanStep {
            step: "x".into(),
            status: PlanStatus::Pending
        };
        PLANNING_ITEMS_MAX + 1
    ];
    assert!(
        validate_plan(&UpdatePlanInput {
            explanation: None,
            plan: above
        })
        .is_err()
    );
}

async fn execute(tool: &crate::registry::RegisteredTool, value: serde_json::Value) {
    let input = ValidatedToolInput::new(BoundedJsonValue::new(value).unwrap()).unwrap();
    let outcome = tool
        .executor
        .execute(RawToolExecutionRequest {
            input,
            cancellation: CancellationToken::new(),
            deadline: tool.definition.timeout,
            definition: tool.definition.clone(),
            model_name: tool.model_name.clone(),
            secrets: empty_delivery(),
        })
        .await
        .unwrap();
    assert!(matches!(outcome, RawToolOutcome::Success(_)));
}

fn empty_delivery() -> crate::pipeline::SecretDelivery {
    crate::pipeline::test_empty_secret_delivery()
}
