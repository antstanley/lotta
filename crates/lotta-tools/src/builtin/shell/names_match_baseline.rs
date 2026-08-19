use super::{ShellToolBundle, definitions};
use crate::{ToolRegistry, ToolsetId};
use lotta_runtime::{
    bounds::{EXTERNAL_TOOL_CALL_TIMEOUT_MS, TOOL_RESULT_BYTES_MAX, TOOL_RESULT_MODEL_CHARS_MAX},
    ports::{ParallelSafety, ToolApprovalPolicy, ToolExecutionOwner},
};
use std::{collections::BTreeSet, sync::Arc};

const INTERNALS: [&str; 7] = [
    "Bash",
    "Monitor",
    "TaskOutput",
    "TaskStop",
    "exec_command",
    "write_stdin",
    "run_shell_command",
];
const SCHEMA_HASHES: [&str; 7] = [
    "a696f5e13b9dde61201945bc585adcb2f77135bbaa990d20958a96ca807ee01e",
    "2f972e981c5341e4a22fb7b236f230a2c55e3a963cb064d1d7ce6e0f84fcb472",
    "df9d934620ea28cced046e4db60190d62b8605be657ddec5d93acfc91db53363",
    "da3ced2f8ff932287a8a18ceb4dfc3fe21e5d77bce5773160482dab4c52d7a6d",
    "b954cdc0ccc0899fe4a0214ac60fc800a08ea19d5d55dc6aa801bbcff8492234",
    "c15b990c0961dc8bc0b834688bd80d4714b2d271c4f6f2b4e7221d6f0ba915aa",
    "bceedd4f2ad4d7b44ea0d4d0a4910ad62dbe8689d84227a66d0c5003953f1718",
];
const DESCRIPTION_HASHES: [&str; 7] = [
    "08532d295b4dcf5d4596e7bc04e9e0f5108bb8c8b5f9a485e13ca215cdf2420b",
    "09a3740a1a10d0c9be5a1df857bfa96ba1878cca199474054c263bcf5293cb79",
    "38940e7dee8299e8045104ffad125744b243752785a7a11d8b9a958d14bc809d",
    "4a19db25ac5aa5a7aeb749122375c8e71ae48d52c0f9612ef976d306e8e90753",
    "038b85781f4679aef37765857e2840ce9c141bc4c289f3aa9ebf1766b6271207",
    "8923c9793da9381d2e5df535773a7b69f19e75f7467d96f162e0932e74793b01",
    "86c260f575071dcc1f63cbdd41358ee13ee00aae643fd627f34306494a8f6924",
];

#[test]
fn registrations_names_assets_and_contract_match_baseline() {
    let fixture = super::test_support::Fixture::new("names");
    let bundle = ShellToolBundle::new(
        &fixture.workspace,
        super::test_support::scope(),
        Arc::new(NoopSandbox),
    )
    .unwrap();
    assert_eq!(bundle.registrations().len(), INTERNALS.len());
    for ((registration, name), (schema_hash, description_hash)) in bundle
        .registrations()
        .iter()
        .zip(INTERNALS)
        .zip(SCHEMA_HASHES.into_iter().zip(DESCRIPTION_HASHES))
    {
        assert_eq!(registration.definition.internal_name.as_str(), name);
        assert_definition(&registration.definition, name);
        assert_eq!(
            sha256(definitions::schema_asset(name).unwrap().as_bytes()),
            schema_hash
        );
        assert_eq!(
            sha256(definitions::description(name).as_bytes()),
            description_hash
        );
    }
}

#[test]
fn exact_toolset_rows_and_alias_identity() {
    let fixture = super::test_support::Fixture::new("rows");
    let bundle = ShellToolBundle::new(
        &fixture.workspace,
        super::test_support::scope(),
        Arc::new(NoopSandbox),
    )
    .unwrap();
    let registry = ToolRegistry::new(bundle.registrations().to_vec()).unwrap();
    let internals = BTreeSet::from(INTERNALS);
    for toolset in ToolsetId::ALL {
        let rows: Vec<_> = crate::names::rows()
            .iter()
            .filter(|row| row.toolset == toolset && internals.contains(row.internal))
            .collect();
        let allow: Vec<_> = rows.iter().map(|row| row.internal).collect();
        let snapshot = registry.update(toolset, &[], Some(&allow)).unwrap();
        let actual: BTreeSet<_> = snapshot
            .model_names()
            .into_iter()
            .map(|model| {
                (
                    model,
                    snapshot
                        .by_model(model)
                        .unwrap()
                        .definition
                        .internal_name
                        .as_str(),
                )
            })
            .collect();
        let expected: BTreeSet<_> = rows.iter().map(|row| (row.model, row.internal)).collect();
        assert_eq!(actual, expected, "{}", toolset.as_str());
    }
    alias_identity(
        &registry,
        ToolsetId::Codex,
        "exec_command",
        ToolsetId::CodexSnake,
    );
    alias_identity(
        &registry,
        ToolsetId::Gemini,
        "RunShellCommand",
        ToolsetId::GeminiSnake,
    );
}

fn assert_definition(definition: &lotta_runtime::ports::ToolDefinition, name: &str) {
    let expected_schema: serde_json::Value =
        serde_json::from_str(definitions::schema_asset(name).unwrap()).unwrap();
    assert_eq!(definition.input_schema.as_value(), &expected_schema);
    assert_eq!(
        definition.description.as_str(),
        definitions::description(name).trim()
    );
    assert_eq!(definition.execution_owner, ToolExecutionOwner::Rust);
    assert_eq!(definition.permission_action.as_str(), "execute");
    assert_eq!(definition.parallel_safety, ParallelSafety::Sequential);
    assert_eq!(definition.timeout.get().as_millis(), 3_600_000);
    assert_eq!(EXTERNAL_TOOL_CALL_TIMEOUT_MS, 300_000);
    assert_eq!(
        definition.output_limit.bytes_max(),
        TOOL_RESULT_BYTES_MAX.value
    );
    assert_eq!(
        definition.output_limit.model_chars_max(),
        TOOL_RESULT_MODEL_CHARS_MAX.value
    );
    assert!(definition.secret_redaction.fields().is_empty());
    let approval = match name {
        "TaskOutput" | "write_stdin" => ToolApprovalPolicy::Never,
        _ => ToolApprovalPolicy::Always,
    };
    assert_eq!(definition.approval_policy, approval);
}

fn alias_identity(registry: &ToolRegistry, left_set: ToolsetId, left: &str, right_set: ToolsetId) {
    let left_snapshot = registry.update(left_set, &[], Some(&[left])).unwrap();
    let left_tool = left_snapshot.by_model(left).unwrap().clone();
    let internal = left_tool.definition.internal_name.as_str();
    let right = crate::names::model_name(right_set, internal).unwrap();
    let right_snapshot = registry.update(right_set, &[], Some(&[right])).unwrap();
    let right_tool = right_snapshot.by_model(right).unwrap();
    assert!(Arc::ptr_eq(&left_tool.definition, &right_tool.definition));
    assert!(Arc::ptr_eq(&left_tool.executor, &right_tool.executor));
}

fn sha256(bytes: &[u8]) -> String {
    use sha2::{Digest, Sha256};
    format!("{:x}", Sha256::digest(bytes))
}

struct NoopSandbox;
impl lotta_runtime::ports::InteractiveSandboxPort for NoopSandbox {
    fn start_session(
        &self,
        _: lotta_runtime::ports::ProcessRequest,
        _: tokio_util::sync::CancellationToken,
    ) -> lotta_runtime::ports::ProcessSessionFuture<'_> {
        Box::pin(async {
            Err(lotta_runtime::RuntimeError::Cancelled {
                context: "test".into(),
            })
        })
    }
}
impl lotta_runtime::ports::SandboxPort for NoopSandbox {
    fn execute(
        &self,
        _: lotta_runtime::ports::ProcessRequest,
        _: tokio::sync::mpsc::Sender<lotta_runtime::ports::ProcessEvent>,
        _: tokio_util::sync::CancellationToken,
    ) -> lotta_runtime::ports::PortFuture<'_, lotta_runtime::ports::ProcessOutcome> {
        Box::pin(async {
            Err(lotta_runtime::RuntimeError::Cancelled {
                context: "test".into(),
            })
        })
    }
}
