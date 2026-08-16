use super::test_support::*;
use crate::{
    PipelineError, PipelineStage, PreflightEvent, ToolsetId, TraceEvent, WorkspacePolicy,
    WorkspaceSandboxGate,
};
use lotta_runtime::{WorkspaceSandbox, ports::ToolOutcome};
use serde_json::{Value, json};
use std::fs;

const PATH_TOOLS: [&str; 18] = [
    "Read",
    "Write",
    "Edit",
    "MultiEdit",
    "apply_patch",
    "LS",
    "Glob",
    "Grep",
    "view_image",
    "read_artifact_file",
    "write_artifact_file",
    "read_file_gemini",
    "read_many_files",
    "write_file_gemini",
    "replace",
    "list_directory",
    "glob_gemini",
    "search_file_content",
];
const ASSERTIONS_PER_TOOL: usize = 3;
const PATCH_VARIANTS: usize = 5;

#[tokio::test]
async fn traversal_matrix() {
    let fixture = live_fixture("traversal");
    run_matrix(&fixture, Attack::Traversal).await;
}

#[tokio::test]
async fn absolute_outside_matrix() {
    let fixture = live_fixture("absolute");
    run_matrix(&fixture, Attack::Absolute).await;
}

#[cfg(unix)]
#[tokio::test]
async fn symlink_escape_matrix() {
    let fixture = live_fixture("symlink");
    std::os::unix::fs::symlink(
        fixture.peer.join("marker.png"),
        fixture.workspace.join("escape-file.png"),
    )
    .unwrap();
    std::os::unix::fs::symlink(&fixture.peer, fixture.workspace.join("escape-dir")).unwrap();
    run_matrix(&fixture, Attack::Symlink).await;
}

#[derive(Clone, Copy)]
enum Attack {
    Traversal,
    Absolute,
    Symlink,
}

fn live_fixture(label: &str) -> Fixture {
    let fixture = Fixture::new(label);
    fs::write(fixture.peer.join("marker.png"), "OUTSIDE_SENTINEL").unwrap();
    fs::write(fixture.artifacts.join("marker.png"), "ARTIFACT_SENTINEL").unwrap();
    fixture.write("inside.txt", "inside needle\n");
    fixture.write("nested/inside.txt", "nested needle\n");
    fixture
}

async fn run_matrix(fixture: &Fixture, attack: Attack) {
    let sandbox = WorkspaceSandbox::new(fixture.workspace.clone(), fixture.root.clone());
    let policy = WorkspacePolicy::new(&sandbox).unwrap();
    let gate = WorkspaceSandboxGate::new(&policy, &fixture.workspace);
    let mut assertions = 0usize;

    for name in PATH_TOOLS {
        let input = attack_input(fixture, name, attack);
        let (result, records) = run_with_gate(fixture, ToolsetId::None, name, input, &gate).await;
        if is_artifact(name) {
            assert_artifact_failure(name, &result, &records);
        } else {
            assert_workspace_denial(name, result, &records);
        }
        assert_markers(fixture);
        assert!(!has_residue(&fixture.workspace));
        assertions += ASSERTIONS_PER_TOOL;
    }

    assertions += patch_embedded_attacks(fixture, attack, &gate).await;
    assertions += pathless_allowed(fixture, &gate).await;
    let expected = PATH_TOOLS.len() * ASSERTIONS_PER_TOOL
        + PATCH_VARIANTS * ASSERTIONS_PER_TOOL
        + 2 * ASSERTIONS_PER_TOOL;
    assert_eq!(assertions, expected);
}

fn assert_workspace_denial(
    name: &str,
    result: Result<ToolOutcome, PipelineError>,
    records: &Records,
) {
    match result {
        Ok(ToolOutcome::SandboxDenied { .. }) => {}
        Err(PipelineError::Sandbox) if name == "read_many_files" => {}
        other => panic!("{name} did not fail at sandbox confinement: {other:?}"),
    }
    let trace = records.trace.lock().unwrap();
    assert!(trace.contains(&TraceEvent::Preflight(PreflightEvent::SchemaValidation)));
    assert!(trace.contains(&TraceEvent::Stage(PipelineStage::Permission)));
    assert!(trace.contains(&TraceEvent::Stage(PipelineStage::Sandbox)));
    assert!(!trace.contains(&TraceEvent::Stage(PipelineStage::Executor)));
}

fn assert_artifact_failure(
    name: &str,
    result: &Result<ToolOutcome, PipelineError>,
    records: &Records,
) {
    assert!(matches!(result, Ok(ToolOutcome::ToolDefinedError { .. })));
    let trace = records.trace.lock().unwrap();
    assert!(trace.contains(&TraceEvent::Stage(PipelineStage::Executor)));
    assert!(!matches!(result, Ok(ToolOutcome::Success { .. })));
    assert!(is_artifact(name));
}

fn is_artifact(name: &str) -> bool {
    matches!(name, "read_artifact_file" | "write_artifact_file")
}

fn assert_markers(fixture: &Fixture) {
    assert_eq!(
        fs::read_to_string(fixture.peer.join("marker.png")).unwrap(),
        "OUTSIDE_SENTINEL"
    );
    assert_eq!(
        fs::read_to_string(fixture.artifacts.join("marker.png")).unwrap(),
        "ARTIFACT_SENTINEL"
    );
}

fn attack_input(fixture: &Fixture, name: &str, attack: Attack) -> Value {
    if is_artifact(name) {
        return artifact_input(fixture, name, attack);
    }
    let file = file_attack(fixture, attack);
    let directory = directory_attack(fixture, attack);
    match name {
        "Read" | "read_file_gemini" => json!({"file_path":file}),
        "Write" | "write_file_gemini" => {
            json!({"file_path":file,"content":"attack"})
        }
        "Edit" | "replace" => json!({
            "file_path":file,
            "old_string":"OUTSIDE_SENTINEL",
            "new_string":"attack"
        }),
        "MultiEdit" => json!({"file_path":file,"edits":[{
            "old_string":"OUTSIDE_SENTINEL",
            "new_string":"attack"
        }]}),
        "apply_patch" => json!({"input":update_patch(&file)}),
        "LS" => json!({"path":directory}),
        "list_directory" => json!({"dir_path":directory}),
        "Glob" => json!({"pattern":"**/*","path":directory}),
        "glob_gemini" => json!({"pattern":"**/*","dir_path":directory}),
        "Grep" => json!({"pattern":"OUTSIDE","path":directory}),
        "search_file_content" => {
            json!({"pattern":"OUTSIDE","dir_path":directory})
        }
        "view_image" => json!({"path":file}),
        "read_many_files" => match attack {
            Attack::Traversal | Attack::Symlink => {
                json!({"include":["../peer/**/*"]})
            }
            Attack::Absolute => json!({"include":[format!("/{directory}/**/*")]}),
        },
        _ => unreachable!(),
    }
}

fn artifact_input(fixture: &Fixture, name: &str, attack: Attack) -> Value {
    let path = match attack {
        Attack::Traversal => "../peer/marker.png".to_owned(),
        Attack::Absolute => fixture
            .peer
            .join("marker.png")
            .to_string_lossy()
            .into_owned(),
        Attack::Symlink => "../workspace/escape-file.png".to_owned(),
    };
    if name == "read_artifact_file" {
        json!({"path":path,"encoding":"utf8"})
    } else {
        json!({"path":path,"content":"attack","encoding":"utf8"})
    }
}

fn file_attack(fixture: &Fixture, attack: Attack) -> String {
    match attack {
        Attack::Traversal => "../peer/marker.png".to_owned(),
        Attack::Absolute => fixture
            .peer
            .join("marker.png")
            .to_string_lossy()
            .into_owned(),
        Attack::Symlink => "escape-file.png".to_owned(),
    }
}

fn directory_attack(fixture: &Fixture, attack: Attack) -> String {
    match attack {
        Attack::Traversal => "../peer".to_owned(),
        Attack::Absolute => fixture.peer.to_string_lossy().into_owned(),
        Attack::Symlink => "escape-dir".to_owned(),
    }
}

fn update_patch(path: &str) -> String {
    format!(
        "*** Begin Patch\n*** Update File: {path}\n@@\n\
         -OUTSIDE_SENTINEL\n+attack\n*** End Patch"
    )
}

async fn patch_embedded_attacks(
    fixture: &Fixture,
    attack: Attack,
    gate: &WorkspaceSandboxGate<'_>,
) -> usize {
    let source = file_attack(fixture, attack);
    let destination = match attack {
        Attack::Traversal => "../peer/new.png".to_owned(),
        Attack::Absolute => fixture.peer.join("new.png").to_string_lossy().into_owned(),
        Attack::Symlink => "escape-dir/new.png".to_owned(),
    };
    let patches = [
        format!("*** Begin Patch\n*** Add File: {destination}\n+x\n*** End Patch"),
        format!("*** Begin Patch\n*** Delete File: {source}\n*** End Patch"),
        update_patch(&source),
        format!(
            "*** Begin Patch\n*** Update File: {source}\n\
             *** Move to: moved.png\n@@\n-OUTSIDE_SENTINEL\n+x\n*** End Patch"
        ),
        format!(
            "*** Begin Patch\n*** Update File: inside.txt\n\
             *** Move to: {destination}\n@@\n-inside needle\n+x\n*** End Patch"
        ),
    ];
    let mut assertions = 0;
    for patch in patches {
        let (result, records) = run_with_gate(
            fixture,
            ToolsetId::None,
            "apply_patch",
            json!({"input":patch}),
            gate,
        )
        .await;
        assert_workspace_denial("apply_patch", result, &records);
        assert_markers(fixture);
        assert!(!has_residue(&fixture.workspace));
        assertions += ASSERTIONS_PER_TOOL;
    }
    assertions
}

async fn pathless_allowed(fixture: &Fixture, gate: &WorkspaceSandboxGate<'_>) -> usize {
    #[cfg(unix)]
    {
        fs::remove_file(fixture.workspace.join("escape-file.png")).ok();
        fs::remove_file(fixture.workspace.join("escape-dir")).ok();
    }
    for (name, input) in [
        ("Glob", json!({"pattern":"**/*.txt"})),
        ("Grep", json!({"pattern":"needle","output_mode":"content"})),
    ] {
        let (result, records) = run_with_gate(fixture, ToolsetId::None, name, input, gate).await;
        let outcome = result.unwrap();
        assert!(
            matches!(outcome, ToolOutcome::Success { .. }),
            "pathless {name} returned {outcome:?}"
        );
        assert!(success_text(&outcome).contains("inside.txt"));
        assert!(
            records
                .trace
                .lock()
                .unwrap()
                .contains(&TraceEvent::Stage(PipelineStage::Executor))
        );
    }
    2 * ASSERTIONS_PER_TOOL
}
