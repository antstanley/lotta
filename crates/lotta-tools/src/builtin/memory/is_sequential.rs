use super::definitions;
use crate::{names, toolset::ToolsetId};
use lotta_runtime::ports::ParallelSafety;

#[test]
fn exact_assets_names_and_sequential_classification() {
    assert_eq!(
        definitions::schema_asset("memory"),
        include_str!("assets/schemas/Memory.json")
    );
    assert_eq!(
        definitions::schema_asset("memory_apply_patch"),
        include_str!("assets/schemas/MemoryApplyPatch.json")
    );
    for name in ["memory", "memory_apply_patch"] {
        assert!(!definitions::description(name).is_empty());
    }
    for set in [
        ToolsetId::Default,
        ToolsetId::Codex,
        ToolsetId::CodexSnake,
        ToolsetId::Gemini,
        ToolsetId::GeminiSnake,
    ] {
        let memory = names::rows().iter().any(|row| {
            row.toolset == set && (row.internal == "memory" || row.internal == "memory_apply_patch")
        });
        assert!(memory, "missing memory family for {set:?}");
    }
    assert!(
        !names::rows()
            .iter()
            .any(|row| row.toolset == ToolsetId::None)
    );
    let definition = crate::registry::tests::support::registration("memory", "memory").definition;
    assert_eq!(definition.parallel_safety, ParallelSafety::Sequential);
}
