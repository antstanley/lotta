use super::test_support::{Fixture, assert_success, run, success_text};
use crate::ToolsetId;
use serde_json::json;
use std::fmt::Write as _;

async fn assert_clamped(
    fixture: &Fixture,
    name: &str,
    input: serde_json::Value,
    shown: Option<&str>,
    expected_raw: &str,
) -> (usize, usize) {
    let (result, records) = run(fixture, ToolsetId::None, name, input).await;
    let outcome = result.unwrap();
    assert_success(&records, &outcome);
    let final_text = success_text(&outcome);
    let overflow = records.overflow.lock().unwrap();
    assert_eq!(overflow.len(), 1);
    assert_eq!(overflow[0].0, name);
    assert_eq!(overflow[0].1, expected_raw);
    assert_ne!(overflow[0].1, final_text);
    assert!(final_text.chars().count() <= 32_000);
    if let Some(marker) = shown {
        assert!(final_text.contains(marker));
    }
    assert_eq!(*records.persisted.lock().unwrap(), vec![outcome.clone()]);
    assert_eq!(*records.emitted.lock().unwrap(), vec![outcome.clone()]);
    eprintln!(
        "{name} clamp raw_chars={} final_chars={}",
        overflow[0].1.chars().count(),
        final_text.chars().count()
    );
    (overflow[0].1.chars().count(), final_text.chars().count())
}

#[tokio::test]
async fn read_raw_over_30k_pipeline_clamps_once() {
    let fixture = Fixture::new("clamp-read");
    let line = format!("{}\n", "r".repeat(1_000));
    fixture.write("large.txt", line.repeat(80));
    let expected = (1..=81).fold(String::new(), |mut output, index| {
        if index != 1 {
            output.push('\n');
        }
        write!(
            output,
            "{index}\t{}",
            if index == 81 {
                String::new()
            } else {
                "r".repeat(1_000)
            }
        )
        .unwrap();
        output
    });
    let (raw, final_chars) = assert_clamped(
        &fixture,
        "Read",
        json!({"file_path":"large.txt","limit":2000}),
        Some("showing 30,000"),
        &expected,
    )
    .await;
    assert!(raw > 30_000 && raw <= 1024 * 1024);
    assert!(final_chars <= 32_000);
}

#[tokio::test]
async fn grep_raw_over_10k_pipeline_clamps_once() {
    let fixture = Fixture::new("clamp-grep");
    let text = (0..2_000).fold(String::new(), |mut output, index| {
        writeln!(output, "needle-{index:04}-{}", "g".repeat(30)).unwrap();
        output
    });
    fixture.write("grep/large.txt", text);
    let expected = (0..2_000).fold(String::new(), |mut output, index| {
        if index != 0 {
            output.push('\n');
        }
        write!(
            output,
            "grep/large.txt:{}:needle-{index:04}-{}",
            index + 1,
            "g".repeat(30)
        )
        .unwrap();
        output
    });
    let (raw, final_chars) = assert_clamped(
        &fixture,
        "Grep",
        json!({
            "pattern":"needle",
            "path":"grep",
            "output_mode":"content",
            "head_limit":3000
        }),
        Some("showing 10,000"),
        &expected,
    )
    .await;
    assert!(raw > 10_000 && raw <= 1024 * 1024);
    assert!(final_chars <= 32_000);
}

#[tokio::test]
async fn unclassified_ls_raw_over_32k_backstop() {
    let fixture = Fixture::new("clamp-ls");
    let names: Vec<_> = (0..1_400)
        .map(|index| format!("entry-{index:04}-{}.txt", "x".repeat(20)))
        .collect();
    for name in &names {
        fixture.write(&format!("listing/{name}"), "x");
    }
    let expected = names.join("\n");
    let (raw, final_chars) =
        assert_clamped(&fixture, "LS", json!({"path":"listing"}), None, &expected).await;
    assert!(raw > 32_000 && raw <= 1024 * 1024);
    assert!(final_chars <= 32_000);
}
