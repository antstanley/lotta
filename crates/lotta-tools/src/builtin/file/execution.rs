use super::test_support::{Fixture, assert_success, run, success_text};
use super::{FileToolBundle, IMAGE_BYTES_MAX, TEXT_FILE_BYTES_MAX};
use crate::ToolsetId;
use lotta_runtime::ports::{ParallelSafety, ToolOutcome};
use serde_json::json;
use std::{fs, fs::File};

async fn success(fixture: &Fixture, name: &str, input: serde_json::Value) -> ToolOutcome {
    let (result, records) = run(fixture, ToolsetId::None, name, input).await;
    let outcome = result.unwrap();
    assert_success(&records, &outcome);
    let bundle = FileToolBundle::new(&fixture.workspace, &fixture.artifacts).unwrap();
    let definition = &bundle
        .registrations()
        .iter()
        .find(|item| item.definition.internal_name.as_str() == name)
        .unwrap()
        .definition;
    assert_eq!(definition.parallel_safety, ParallelSafety::Sequential);
    outcome
}

#[tokio::test]
async fn read_line_numbers_tab_offset() {
    let fixture = Fixture::new("read");
    fixture.write("a.txt", "zero\none\ntwo\n");
    let out = success(
        &fixture,
        "Read",
        json!({"file_path":"a.txt","offset":1,"limit":2}),
    )
    .await;
    assert_eq!(success_text(&out), "2\tone\n3\ttwo");
}

#[tokio::test]
async fn write_atomic() {
    let fixture = Fixture::new("write");
    let out = success(
        &fixture,
        "Write",
        json!({"file_path":"a.txt","content":"atomic"}),
    )
    .await;
    assert_eq!(success_text(&out), "File written successfully.");
    assert_eq!(
        fs::read_to_string(fixture.workspace.join("a.txt")).unwrap(),
        "atomic"
    );
}

#[tokio::test]
async fn edit_exact() {
    let fixture = Fixture::new("edit");
    fixture.write("a.txt", "before unique after");
    success(
        &fixture,
        "Edit",
        json!({"file_path":"a.txt","old_string":"unique","new_string":"changed"}),
    )
    .await;
    assert_eq!(
        fs::read_to_string(fixture.workspace.join("a.txt")).unwrap(),
        "before changed after"
    );
}

#[tokio::test]
async fn multi_edit_ordered_one_commit() {
    let fixture = Fixture::new("multi");
    fixture.write("a.txt", "a b");
    success(
        &fixture,
        "MultiEdit",
        json!({"file_path":"a.txt","edits":[
            {"old_string":"a","new_string":"x"},
            {"old_string":"x b","new_string":"done"}
        ]}),
    )
    .await;
    assert_eq!(
        fs::read_to_string(fixture.workspace.join("a.txt")).unwrap(),
        "done"
    );
}

#[tokio::test]
async fn apply_patch_add_update_delete_move() {
    let fixture = Fixture::new("patch");
    fixture.write("update.txt", "old\n");
    fixture.write("delete.txt", "gone\n");
    fixture.write("move.txt", "move\n");
    let patch = concat!(
        "*** Begin Patch\n",
        "*** Add File: add.txt\n+added\n",
        "*** Update File: update.txt\n@@\n-old\n+new\n",
        "*** Delete File: delete.txt\n",
        "*** Update File: move.txt\n*** Move to: moved.txt\n",
        "@@\n-move\n+moved\n*** End Patch",
    );
    success(&fixture, "apply_patch", json!({"input":patch})).await;
    assert_eq!(
        fs::read_to_string(fixture.workspace.join("add.txt")).unwrap(),
        "added\n"
    );
    assert_eq!(
        fs::read_to_string(fixture.workspace.join("update.txt")).unwrap(),
        "new\n"
    );
    assert!(!fixture.workspace.join("delete.txt").exists());
    assert_eq!(
        fs::read_to_string(fixture.workspace.join("moved.txt")).unwrap(),
        "moved\n"
    );
}

#[tokio::test]
async fn ls_deterministic() {
    let fixture = Fixture::new("ls");
    fixture.write("listing/z.txt", "z");
    fixture.write("listing/dir/a.txt", "a");
    fixture.write("listing/a.txt", "a");
    let out = success(&fixture, "LS", json!({"path":"listing"})).await;
    assert_eq!(success_text(&out), "dir/\na.txt\nz.txt");
}

#[tokio::test]
async fn glob_nested_hidden() {
    let fixture = Fixture::new("glob");
    fixture.write("nested/a.rs", "a");
    fixture.write("nested/.hidden.rs", "h");
    let out = success(
        &fixture,
        "Glob",
        json!({"pattern":"**/*.rs","path":"nested"}),
    )
    .await;
    let text = success_text(&out);
    assert!(text.contains("nested/a.rs"));
    assert!(text.contains("nested/.hidden.rs"));
}

#[tokio::test]
async fn grep_exact_modes_multiline_and_alias() {
    let fixture = Fixture::new("grep-parity");
    fixture.write("grep/a.txt", "before\nalpha\nbeta\nafter\nalpha beta\n");
    fixture.write("grep/b.txt", "alpha beta\n");
    let files = success(
        &fixture,
        "Grep",
        json!({"pattern":"alpha beta","path":"grep","offset":1,"head_limit":1}),
    )
    .await;
    assert_eq!(
        success_text(&files),
        "Found 2 files (showing 1)\ngrep/b.txt"
    );
    let count = success(
        &fixture,
        "Grep",
        json!({"pattern":"alpha beta","path":"grep","output_mode":"count"}),
    )
    .await;
    assert_eq!(
        success_text(&count),
        "grep/a.txt:1\ngrep/b.txt:1\n\nFound 2 total occurrences across 2 files."
    );
    let content = success(
        &fixture,
        "Grep",
        json!({
            "pattern":"alpha\\nbeta",
            "path":"grep",
            "output_mode":"content",
            "multiline":true,
            "-C":1
        }),
    )
    .await;
    assert_eq!(
        success_text(&content),
        "grep/a.txt-1-before\ngrep/a.txt:2:alpha\ngrep/a.txt:3:beta\ngrep/a.txt-4-after"
    );
    let alias = gemini_success(
        &fixture,
        "SearchFileContent",
        json!({"pattern":"alpha beta","dir_path":"grep"}),
    )
    .await;
    assert_eq!(
        success_text(&alias),
        "Found 2 files\ngrep/a.txt\ngrep/b.txt"
    );
}

#[tokio::test]
async fn read_notices_and_read_many_exact() {
    let fixture = Fixture::new("read-parity");
    fixture.write("empty.txt", " \n\t");
    fixture.write("long.txt", format!("{}\n", "x".repeat(2_001)));
    let empty = success(&fixture, "Read", json!({"file_path":"empty.txt"})).await;
    assert_eq!(
        success_text(&empty),
        format!(
            "<system-reminder>\nThe file {} exists but has empty contents.\n</system-reminder>",
            fixture.workspace.join("empty.txt").display()
        )
    );
    let long = success(&fixture, "Read", json!({"file_path":"long.txt"})).await;
    assert_eq!(
        success_text(&long),
        format!(
            concat!(
                "1\t{}... [line truncated]\n2\t\n\n",
                "[Some lines exceeded 2,000 characters and were truncated.]"
            ),
            "x".repeat(2_000)
        )
    );
    let many = gemini_success(
        &fixture,
        "ReadManyFiles",
        json!({
            "include":["empty.txt"],
            "recursive":false,
            "file_filtering_options":{"respect_git_ignore":false},
            "useDefaultExcludes":false
        }),
    )
    .await;
    assert_eq!(
        success_text(&many),
        format!(
            concat!(
                "--- {} ---\n\n<system-reminder>\n",
                "The file {} exists but has empty contents.\n",
                "</system-reminder>\n\n--- End of content ---"
            ),
            fixture.workspace.join("empty.txt").display(),
            fixture.workspace.join("empty.txt").display()
        )
    );
}

#[tokio::test]
async fn grep_regex_content() {
    let fixture = Fixture::new("grep");
    fixture.write("grep/a.txt", "alpha\nbeta42\n");
    let out = success(
        &fixture,
        "Grep",
        json!({"pattern":"beta\\d+","path":"grep","output_mode":"content","head_limit":20}),
    )
    .await;
    assert!(success_text(&out).contains("beta42"));
}

#[tokio::test]
async fn view_image_tiny_json_base64() {
    let fixture = Fixture::new("image");
    fixture.write("tiny.png", [137, 80, 78, 71]);
    let out = success(&fixture, "view_image", json!({"path":"tiny.png"})).await;
    let value: serde_json::Value = serde_json::from_str(success_text(&out)).unwrap();
    assert_eq!(value["mime_type"], "image/png");
    assert_eq!(value["data"], "iVBORw==");
}

#[tokio::test]
async fn artifact_write_then_read_utf8_and_base64() {
    let fixture = Fixture::new("artifact");
    success(
        &fixture,
        "write_artifact_file",
        json!({"path":"a.bin","content":"evidence","encoding":"utf8"}),
    )
    .await;
    let utf8 = success(
        &fixture,
        "read_artifact_file",
        json!({"path":"a.bin","encoding":"utf8"}),
    )
    .await;
    let base64 = success(
        &fixture,
        "read_artifact_file",
        json!({"path":"a.bin","encoding":"base64"}),
    )
    .await;
    assert_eq!(success_text(&utf8), "evidence");
    assert_eq!(success_text(&base64), "ZXZpZGVuY2U=");
}

#[tokio::test]
async fn gemini_read_offset_mapping() {
    let fixture = Fixture::new("gemini-read-offset");
    fixture.write("lines.txt", "first\nsecond\nthird\n");
    let omitted = success(
        &fixture,
        "read_file_gemini",
        json!({"file_path":"lines.txt","limit":1}),
    )
    .await;
    assert_eq!(success_text(&omitted), "1\tfirst");
    let explicit = success(
        &fixture,
        "read_file_gemini",
        json!({"file_path":"lines.txt","offset":0,"limit":1}),
    )
    .await;
    assert_eq!(success_text(&explicit), "2\tsecond");
}

#[tokio::test]
async fn gemini_adapters_real_mapped_names() {
    let fixture = Fixture::new("gemini");
    fixture.write("a.txt", "old needle\n");
    fixture.write("nested/c.txt", "nested needle\n");
    fixture.write("empty/keep.md", "keep\n");
    assert_gemini_reads(&fixture).await;
    assert_gemini_mutations(&fixture).await;
    assert_gemini_searches(&fixture).await;
}

async fn assert_gemini_reads(fixture: &Fixture) {
    let read = gemini_success(fixture, "ReadFileGemini", json!({"file_path":"a.txt"})).await;
    assert_eq!(success_text(&read), "1\told needle\n2\t");
    let many = gemini_success(
        fixture,
        "ReadManyFiles",
        json!({"include":["*.txt","**/*.txt"],"useDefaultExcludes":false}),
    )
    .await;
    for expected in ["a.txt", "old needle", "nested/c.txt", "nested needle"] {
        assert!(success_text(&many).contains(expected));
    }
}

async fn assert_gemini_mutations(fixture: &Fixture) {
    let write = gemini_success(
        fixture,
        "WriteFileGemini",
        json!({"file_path":"b.txt","content":"written"}),
    )
    .await;
    assert_eq!(success_text(&write), "File written successfully.");
    assert_eq!(
        fs::read_to_string(fixture.workspace.join("b.txt")).unwrap(),
        "written"
    );
    let replace = gemini_success(
        fixture,
        "Replace",
        json!({"file_path":"a.txt","old_string":"old","new_string":"new"}),
    )
    .await;
    assert_eq!(success_text(&replace), "File edited successfully.");
    assert_eq!(
        fs::read_to_string(fixture.workspace.join("a.txt")).unwrap(),
        "new needle\n"
    );
}

async fn assert_gemini_searches(fixture: &Fixture) {
    let list = gemini_success(fixture, "ListDirectory", json!({"dir_path":"."})).await;
    for expected in ["a.txt", "b.txt", "nested/"] {
        assert!(success_text(&list).contains(expected));
    }
    let glob = gemini_success(
        fixture,
        "GlobGemini",
        json!({"pattern":"**/*.txt","dir_path":"."}),
    )
    .await;
    for expected in ["a.txt", "b.txt", "nested/c.txt"] {
        assert!(success_text(&glob).contains(expected));
    }
    let search = gemini_success(
        fixture,
        "SearchFileContent",
        json!({"pattern":"needle","dir_path":"."}),
    )
    .await;
    assert!(success_text(&search).contains("a.txt"));
    assert!(success_text(&search).contains("nested/c.txt"));
}

async fn gemini_success(fixture: &Fixture, model: &str, input: serde_json::Value) -> ToolOutcome {
    let (result, records) = run(fixture, ToolsetId::Gemini, model, input).await;
    let outcome = result.unwrap();
    assert_success(&records, &outcome);
    assert!(
        matches!(outcome, ToolOutcome::Success { .. }),
        "{model} returned {outcome:?}"
    );
    outcome
}

#[tokio::test]
async fn read_text_file_size_boundaries() {
    let fixture = Fixture::new("read-text-boundaries");
    let path = fixture.workspace.join("boundary.txt");
    for bytes in [TEXT_FILE_BYTES_MAX - 1, TEXT_FILE_BYTES_MAX] {
        fs::write(&path, "x".repeat(bytes)).unwrap();
        let outcome = success(
            &fixture,
            "Read",
            json!({"file_path":"boundary.txt","limit":1}),
        )
        .await;
        assert!(matches!(outcome, ToolOutcome::Success { .. }));
        fs::remove_file(&path).unwrap();
    }
    sparse(&path, TEXT_FILE_BYTES_MAX + 1);
    assert_fixed_non_success(&fixture, "Read", json!({"file_path":"boundary.txt"})).await;
    fs::remove_file(path).unwrap();
}

#[tokio::test]
async fn named_boundaries_reject_oversize() {
    let fixture = Fixture::new("bounds");
    fixture.write("marker.txt", "unchanged");
    let pattern = "x".repeat(4_097);
    let edits: Vec<_> = (0..1_025)
        .map(|_| json!({"old_string":"a","new_string":"b"}))
        .collect();
    for (name, input) in [
        ("Grep", json!({"pattern":pattern,"path":"."})),
        ("MultiEdit", json!({"file_path":"marker.txt","edits":edits})),
    ] {
        assert_fixed_non_success(&fixture, name, input).await;
    }

    sparse(
        &fixture.workspace.join("oversize.txt"),
        TEXT_FILE_BYTES_MAX + 1,
    );
    sparse(&fixture.workspace.join("oversize.png"), IMAGE_BYTES_MAX + 1);
    assert_fixed_non_success(&fixture, "Read", json!({"file_path":"oversize.txt"})).await;
    assert_fixed_non_success(&fixture, "view_image", json!({"path":"oversize.png"})).await;
    assert_eq!(
        fs::read_to_string(fixture.workspace.join("marker.txt")).unwrap(),
        "unchanged"
    );
}

fn sparse(path: &std::path::Path, bytes: usize) {
    File::create(path)
        .unwrap()
        .set_len(u64::try_from(bytes).unwrap())
        .unwrap();
}

async fn assert_fixed_non_success(fixture: &Fixture, name: &str, input: serde_json::Value) {
    let (result, _) = run(fixture, ToolsetId::None, name, input).await;
    match result {
        Ok(ToolOutcome::ToolDefinedError { .. }) | Err(_) => {}
        other => panic!("{name} unexpectedly succeeded or returned an unstable outcome: {other:?}"),
    }
}
