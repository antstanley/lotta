use super::*;
use lotta_domain::{BoundedJsonValue, BoundedVec};
use std::sync::Arc;

fn text(value: &str) -> ToolOutcomeMessage {
    ToolOutcomeMessage::new(value.into()).unwrap()
}
fn output_limit() -> ToolOutputLimit {
    ToolOutputLimit::new(
        TOOL_RESULT_BYTES_MAX.value,
        TOOL_RESULT_MODEL_CHARS_MAX.value,
    )
    .unwrap()
}
fn definition() -> ToolDefinition {
    ToolDefinition::new(
        InternalToolName::new("read".into()).unwrap(),
        ModelFacingToolName::new("Read".into()).unwrap(),
        ToolInputSchema::new(BoundedJsonValue::new(serde_json::json!({"type":"object"})).unwrap())
            .unwrap(),
        ToolDescriptionAsset::new("Read a file".into()).unwrap(),
        ToolExecutionOwner::Rust,
        ToolApprovalPolicy::Never,
        PermissionAction::new("read".into()).unwrap(),
        ToolTimeout::new(Duration::from_millis(EXTERNAL_TOOL_CALL_TIMEOUT_MS as u64)).unwrap(),
        output_limit(),
        SecretRedactionSpec::new(
            BoundedVec::new(Vec::new()).unwrap(),
            SecretRedactionPolicy::Redact,
        )
        .unwrap(),
    )
}

pub(crate) fn tool_definition_fields() {
    let ToolDefinition {
        internal_name,
        model_name,
        input_schema,
        description,
        execution_owner,
        approval_policy,
        permission_action,
        parallel_safety,
        timeout,
        output_limit,
        secret_redaction,
    } = definition();
    let _: InternalToolName = internal_name;
    let _: ModelFacingToolName = model_name;
    let _: ToolInputSchema = input_schema;
    let _: ToolDescriptionAsset = description;
    let _: ToolExecutionOwner = execution_owner;
    let _: ToolApprovalPolicy = approval_policy;
    let _: PermissionAction = permission_action;
    let _: ParallelSafety = parallel_safety;
    let _: ToolTimeout = timeout;
    let _: ToolOutputLimit = output_limit;
    let _: SecretRedactionSpec = secret_redaction;
}

pub(crate) fn parallel_safety_defaults_sequential() {
    assert_eq!(ParallelSafety::default(), ParallelSafety::Sequential);
    assert_eq!(definition().parallel_safety, ParallelSafety::Sequential);
    let certified = definition()
        .with_parallel_certification(ParallelCertificationId::new("audit-1".into()).unwrap());
    assert!(matches!(
        certified.parallel_safety,
        ParallelSafety::CertifiedParallel(_)
    ));
}

pub(crate) fn execution_owner() {
    let owners = [
        ToolExecutionOwner::Rust,
        ToolExecutionOwner::Mcp,
        ToolExecutionOwner::Controller,
        ToolExecutionOwner::ModSidecar,
        ToolExecutionOwner::ChannelGateway,
    ];
    let names: Vec<_> = owners
        .iter()
        .map(|owner| serde_json::to_value(owner).unwrap())
        .collect();
    assert_eq!(
        names,
        [
            "rust",
            "mcp",
            "controller",
            "mod_sidecar",
            "channel_gateway"
        ]
    );
}

fn round_trip(outcome: &ToolOutcome, expected: &str) {
    let json = serde_json::to_value(outcome).unwrap();
    assert_eq!(json["type"], expected);
    assert_eq!(
        serde_json::from_value::<ToolOutcome>(json).unwrap(),
        *outcome
    );
}
pub(crate) fn outcome_success() {
    round_trip(
        &ToolOutcome::Success {
            content: ToolResultText::new("ok".into(), output_limit()).unwrap(),
        },
        "success",
    );
}
pub(crate) fn outcome_user_denied() {
    round_trip(
        &ToolOutcome::UserDenied {
            message: text("no"),
        },
        "user_denied",
    );
}
pub(crate) fn outcome_interrupted() {
    round_trip(
        &ToolOutcome::Interruption {
            message: text("stop"),
        },
        "interruption",
    );
}
pub(crate) fn outcome_timeout() {
    round_trip(
        &ToolOutcome::Timeout {
            message: text("late"),
        },
        "timeout",
    );
}
pub(crate) fn outcome_validation_failure() {
    round_trip(
        &ToolOutcome::ValidationFailure {
            message: text("bad"),
        },
        "validation_failure",
    );
}
pub(crate) fn outcome_sandbox_denied() {
    round_trip(
        &ToolOutcome::SandboxDenied {
            message: text("deny"),
        },
        "sandbox_denied",
    );
}
pub(crate) fn outcome_spawn_failure() {
    round_trip(
        &ToolOutcome::SpawnFailure {
            message: text("spawn"),
        },
        "spawn_failure",
    );
}
pub(crate) fn outcome_tool_defined_error() {
    round_trip(
        &ToolOutcome::ToolDefinedError {
            code: ToolOutcomeCode::new("tool_error".into()).unwrap(),
            message: text("failed"),
        },
        "tool_defined_error",
    );
}

struct ExternalTool;
impl ToolPort for ExternalTool {
    fn execute(&self, request: ToolExecutionRequest) -> PortFuture<'_, ToolOutcome> {
        Box::pin(async move {
            assert_eq!(
                request.input.as_value(),
                &serde_json::json!({"secret":"value"})
            );
            Ok(ToolOutcome::Success {
                content: ToolResultText::new("ok".into(), request.definition.output_limit)?,
            })
        })
    }
}
pub(crate) fn object_safe_external_implementation() {
    fn object_safe(_: Arc<dyn ToolPort>) {}
    object_safe(Arc::new(ExternalTool));
}

pub(crate) fn input_output_deadline_boundaries() {
    let input = ValidatedToolInput::new(
        BoundedJsonValue::new(serde_json::json!({"secret":"value"})).unwrap(),
    )
    .unwrap();
    let request = ToolExecutionRequest {
        tool_call_id: ToolCallId::from_name(
            crate::boundary::ProviderName::new("call".into()).unwrap(),
        ),
        approval_grant: ToolApprovalGrant::None,
        definition: definition(),
        input,
        cancellation: CancellationToken::new(),
        deadline: ToolTimeout::new(Duration::from_millis(1)).unwrap(),
    };
    let debug = format!("{request:?}");
    assert!(debug.contains("[REDACTED]"));
    assert!(!debug.contains("value"));
    assert!(ToolTimeout::new(Duration::ZERO).is_err());
    assert!(
        ToolTimeout::new(Duration::from_millis(
            EXTERNAL_TOOL_CALL_TIMEOUT_MS as u64 + 1
        ))
        .is_err()
    );
    assert!(ToolOutputLimit::new(TOOL_RESULT_BYTES_MAX.value + 1, 1).is_err());
    assert!(ToolOutputLimit::new(1, TOOL_RESULT_MODEL_CHARS_MAX.value + 1).is_err());
    let unicode_limit = ToolOutputLimit::new(8, 2).unwrap();
    assert!(ToolResultText::new("😀😀".into(), unicode_limit).is_ok());
    assert!(ToolResultText::new("😀😀a".into(), unicode_limit).is_err());
}

pub(crate) fn schema_shape_and_malicious_serde() {
    let schema = |value| ToolInputSchema::new(BoundedJsonValue::new(value).unwrap());
    for valid in [
        serde_json::json!({}),
        serde_json::json!({
            "type": "object",
            "properties": {"name": {"type": "string"}},
            "required": ["name"]
        }),
        serde_json::json!({
            "properties": {"first": true, "second": false},
            "required": ["first", "second"]
        }),
        serde_json::json!({"properties": {"enabled": true}}),
        serde_json::json!({"required": ["canonical_without_properties"]}),
    ] {
        assert!(schema(valid.clone()).is_ok());
        assert!(serde_json::from_value::<ToolInputSchema>(valid).is_ok());
    }
    for invalid in [
        serde_json::json!(null),
        serde_json::json!(true),
        serde_json::json!(1),
        serde_json::json!("object"),
        serde_json::json!([]),
        serde_json::json!({"type":"string"}),
        serde_json::json!({"properties":[]}),
        serde_json::json!({"required":"name"}),
        serde_json::json!({"required":[1]}),
        serde_json::json!({"required":["name", "name"]}),
        serde_json::json!({"properties":{"present":true},"required":["missing"]}),
    ] {
        assert!(schema(invalid.clone()).is_err());
        assert!(serde_json::from_value::<ToolInputSchema>(invalid).is_err());
    }
}

pub(crate) fn secret_paths_and_serde() {
    let path = |value: &str| SecretFieldPath::new(value.into());
    assert!(path("/credentials/api~1key/~0token").is_ok());
    assert!(path(&format!("/{}", "a".repeat(254))).is_ok());
    assert!(path(&format!("/{}", "a".repeat(255))).is_ok());
    assert!(path(&format!("/{}", "a".repeat(256))).is_err());
    for invalid in ["", "relative", "/", "//x", "/bad~", "/bad~2", "/nul\0x"] {
        assert!(path(invalid).is_err());
        assert!(serde_json::from_value::<SecretFieldPath>(serde_json::json!(invalid)).is_err());
    }
    let one = path("/credentials/token").unwrap();
    let duplicate = BoundedVec::new(vec![one.clone(), one]).unwrap();
    assert!(SecretRedactionSpec::new(duplicate, SecretRedactionPolicy::Omit).is_err());
    let malicious = serde_json::json!({
        "fields": ["/credentials/token", "/credentials/token"],
        "policy": "redact"
    });
    assert!(serde_json::from_value::<SecretRedactionSpec>(malicious).is_err());
    let spec = SecretRedactionSpec::new(
        BoundedVec::new(vec![path("/credentials/token").unwrap()]).unwrap(),
        SecretRedactionPolicy::Omit,
    )
    .unwrap();
    let debug = format!("{spec:?}");
    assert!(debug.contains("/credentials/token") && debug.contains("Omit"));
    let wire = serde_json::to_value(&spec).unwrap();
    assert_eq!(
        serde_json::from_value::<SecretRedactionSpec>(wire).unwrap(),
        spec
    );
}

pub(crate) fn lower_limit_outcome_serde() {
    let limit = ToolOutputLimit::new(8, 4).unwrap();
    let outcome = ToolOutcome::Success {
        content: ToolResultText::new("four".into(), limit).unwrap(),
    };
    let wire = serde_json::to_value(&outcome).unwrap();
    assert_eq!(wire["content"]["value"], "four");
    assert_eq!(wire["content"]["applied_limit"]["bytes_max"], 8);
    assert_eq!(wire["content"]["applied_limit"]["model_chars_max"], 4);
    let decoded = serde_json::from_value::<ToolOutcome>(wire).unwrap();
    assert_eq!(decoded, outcome);
    let invalid = |value, bytes, chars| {
        serde_json::from_value::<ToolOutcome>(serde_json::json!({
            "type": "success",
            "content": {
                "value": value,
                "applied_limit": {"bytes_max": bytes, "model_chars_max": chars}
            }
        }))
    };
    assert!(invalid("abcde", 8, 4).is_err());
    assert!(invalid("😀😀a", 8, 4).is_err());
    assert!(invalid("ok", 0, 4).is_err());
    assert!(invalid("ok", TOOL_RESULT_BYTES_MAX.value + 1, 4).is_err());
    assert!(serde_json::from_value::<ToolOutcome>(serde_json::json!({"type":"unknown"})).is_err());
    assert!(
        serde_json::from_value::<ToolOutcome>(serde_json::json!({
            "type":"user_denied", "message":"x".repeat(TOOL_DESCRIPTION_BYTES_MAX.value + 1)
        }))
        .is_err()
    );
    assert!(
        serde_json::from_value::<ToolOutcome>(serde_json::json!({
            "type":"tool_defined_error", "code":"x".repeat(TOOL_NAME_BYTES_MAX.value + 1),
            "message":"bad"
        }))
        .is_err()
    );
}

pub(crate) fn owning_constructor_boundaries() {
    for size in [TOOL_NAME_BYTES_MAX.value - 1, TOOL_NAME_BYTES_MAX.value] {
        assert!(InternalToolName::new("n".repeat(size)).is_ok());
    }
    assert!(InternalToolName::new("n".repeat(TOOL_NAME_BYTES_MAX.value + 1)).is_err());
    assert!(ToolDescriptionAsset::new("d".repeat(TOOL_DESCRIPTION_BYTES_MAX.value)).is_ok());
    assert!(ToolDescriptionAsset::new("d".repeat(TOOL_DESCRIPTION_BYTES_MAX.value + 1)).is_err());
    assert!(ToolTimeout::new(Duration::from_millis(EXTERNAL_TOOL_CALL_TIMEOUT_MS as u64)).is_ok());
    let max = output_limit();
    assert!(ToolResultText::new("a".repeat(TOOL_RESULT_MODEL_CHARS_MAX.value), max).is_ok());
    assert!(ToolResultText::new("a".repeat(TOOL_RESULT_MODEL_CHARS_MAX.value + 1), max).is_err());
    let paths: Vec<_> = (0..TOOL_SECRET_FIELDS_ITEMS_MAX.value)
        .map(|index| SecretFieldPath::new(format!("/s{index}")).unwrap())
        .collect();
    assert!(
        SecretRedactionSpec::new(
            BoundedVec::new(paths).unwrap(),
            SecretRedactionPolicy::Redact
        )
        .is_ok()
    );
    let over: Vec<_> = (0..=TOOL_SECRET_FIELDS_ITEMS_MAX.value)
        .map(|index| SecretFieldPath::new(format!("/s{index}")).unwrap())
        .collect();
    assert!(BoundedVec::<_, { TOOL_SECRET_FIELDS_ITEMS_MAX.value }>::new(over).is_err());
}

pub(crate) fn broad_contract() {
    tool_definition_fields();
    parallel_safety_defaults_sequential();
    execution_owner();
    outcome_success();
    outcome_user_denied();
    outcome_interrupted();
    outcome_timeout();
    outcome_validation_failure();
    outcome_sandbox_denied();
    outcome_spawn_failure();
    outcome_tool_defined_error();
    object_safe_external_implementation();
    input_output_deadline_boundaries();
    schema_shape_and_malicious_serde();
    secret_paths_and_serde();
    lower_limit_outcome_serde();
    owning_constructor_boundaries();
}
