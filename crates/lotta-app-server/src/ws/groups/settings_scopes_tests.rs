//! `ws::settings::scopes` — each persisted value lands at exactly the path
//! §State outside the backend root assigns its scope, written through the
//! Task 27 side store (never a direct filesystem write).

use lotta_store::{ProjectFile, StoreErrorKind};
use serde_json::{Value, json};

use super::support::{bridge, runtime};
use super::{
    ReflectionMergeMode, ReflectionScope, ReflectionTriggerMode, SetReflectionSettingsCommand,
    SettingsCommand,
};

/// Fixture agent identifier shared by every scope case.
const AGENT_ID: &str = "agent-local-scopes-test";

fn set_reflection_command(request_id: &str, scope: Option<ReflectionScope>) -> SettingsCommand {
    SettingsCommand::SetReflectionSettings(SetReflectionSettingsCommand {
        request_id: request_id.to_owned(),
        runtime: runtime(AGENT_ID, "default"),
        settings: super::ReflectionSettingsBody {
            trigger: ReflectionTriggerMode::StepCount,
            step_count: 25,
            merge: Some(ReflectionMergeMode::Explicit),
            merge_instructions: Some("Preserve exact wording.".to_owned()),
        },
        scope,
    })
}

/// Reads one document through the Task 27 side store; null when absent.
fn scoped_document(
    fixture: &super::support::TestSettings,
    workspace_file: Option<ProjectFile>,
) -> Value {
    let read = match workspace_file {
        Some(file) => lotta_store::side::project::read(&fixture.paths, &fixture.workspace, file),
        None => lotta_store::side::settings::read(&fixture.paths),
    };
    match read {
        Ok(source) => serde_json::from_slice(source.bytes()).expect("scoped document"),
        Err(error) if error.kind() == StoreErrorKind::NotFound => json!(null),
        Err(error) => panic!("scoped read failed: {error:?}"),
    }
}

/// Whether one scoped settings file exists on disk.
fn exists(fixture: &super::support::TestSettings, workspace_file: Option<ProjectFile>) -> bool {
    match workspace_file {
        Some(file) => fixture
            .paths
            .project_file(&fixture.workspace, file)
            .is_ok_and(|path| path.exists()),
        None => fixture.paths.settings().is_ok_and(|path| path.exists()),
    }
}

/// The persisted per-agent reflection entry of one scoped document.
fn agent_entry(document: &Value) -> &Value {
    &document["reflection_settings_by_agent"][AGENT_ID]
}

#[test]
fn local_project_scope_writes_the_workspace_local_settings_path() {
    let fixture = bridge();
    fixture
        .send(
            &serde_json::to_value(set_reflection_command(
                "sc-1",
                Some(ReflectionScope::LocalProject),
            ))
            .expect("command encodes"),
        )
        .expect("wellformed command");
    let local_path = fixture
        .paths
        .project_file(&fixture.workspace, ProjectFile::LocalSettings)
        .expect("authorized workspace");
    assert_eq!(
        local_path,
        fixture.workspace.join(".letta").join("settings.local.json"),
        "project-local writes land in <workspace>/.letta/settings.local.json"
    );
    let document = scoped_document(&fixture, Some(ProjectFile::LocalSettings));
    assert_eq!(agent_entry(&document)["trigger"], "step-count");
    assert_eq!(agent_entry(&document)["step_count"], 25);
    assert_eq!(agent_entry(&document)["merge"], "explicit");
    assert_eq!(
        scoped_document(&fixture, None),
        json!(null),
        "a local_project write never touches the global document"
    );
}

#[test]
fn global_scope_writes_the_global_settings_path() {
    let fixture = bridge();
    fixture
        .send(
            &serde_json::to_value(set_reflection_command(
                "sc-2",
                Some(ReflectionScope::Global),
            ))
            .expect("command encodes"),
        )
        .expect("wellformed command");
    let global_path = fixture.paths.settings().expect("global settings path");
    assert_eq!(
        global_path,
        fixture.home.join(".letta").join("settings.json"),
        "global writes land in <home>/.letta/settings.json"
    );
    let document = scoped_document(&fixture, None);
    assert_eq!(agent_entry(&document)["trigger"], "step-count");
    assert_eq!(
        agent_entry(&document)["merge_instructions"],
        "Preserve exact wording."
    );
    assert!(
        !exists(&fixture, Some(ProjectFile::LocalSettings)),
        "a global write never touches the project-local document"
    );
}

#[test]
fn both_scope_writes_both_scoped_paths() {
    let fixture = bridge();
    fixture
        .send(&serde_json::to_value(set_reflection_command("sc-3", None)).expect("command encodes"))
        .expect("wellformed command");
    let local = scoped_document(&fixture, Some(ProjectFile::LocalSettings));
    let global = scoped_document(&fixture, None);
    assert_eq!(
        agent_entry(&local)["merge"],
        "explicit",
        "workspace-local copy persisted"
    );
    assert_eq!(
        agent_entry(&global)["step_count"],
        25,
        "global copy persisted"
    );
    assert!(exists(&fixture, Some(ProjectFile::LocalSettings)));
    assert!(exists(&fixture, None));
}
