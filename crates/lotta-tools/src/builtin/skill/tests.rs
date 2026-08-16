use super::*;
use std::collections::BTreeMap;
use std::sync::atomic::{AtomicUsize, Ordering};

struct FakePort {
    values: BTreeMap<String, RegisteredSkillContent>,
    reads: AtomicUsize,
}
impl RegisteredSkillPort for FakePort {
    fn load(&self, id: &str) -> SkillLoadFuture<'_> {
        let value = self.values.get(id).cloned();
        if value.is_some() {
            self.reads.fetch_add(1, Ordering::Relaxed);
        }
        Box::pin(async move { value.ok_or(SkillPortError::Unknown) })
    }
}

#[tokio::test]
async fn loads_registered_skill() {
    let content = RegisteredSkillContent {
        instructions: "exact instructions".into(),
        companions: vec![
            RegisteredSkillCompanion {
                relative_path: "a.txt".into(),
                bytes: b"A".to_vec(),
            },
            RegisteredSkillCompanion {
                relative_path: "nested/b.txt".into(),
                bytes: b"B".to_vec(),
            },
        ],
    };
    let port = Arc::new(FakePort {
        values: BTreeMap::from([("demo".into(), content.clone())]),
        reads: AtomicUsize::new(0),
    });
    let loaded = port.load("demo").await.unwrap();
    let rendered = render("demo", loaded).unwrap();
    assert!(rendered.contains("exact instructions"));
    assert!(rendered.find("a.txt").unwrap() < rendered.find("nested/b.txt").unwrap());
    assert_eq!(port.reads.load(Ordering::Relaxed), 1);
}

#[tokio::test]
async fn unknown_skill_is_tool_error() {
    let port = FakePort {
        values: BTreeMap::new(),
        reads: AtomicUsize::new(0),
    };
    assert_eq!(port.load("unknown").await, Err(SkillPortError::Unknown));
    assert_eq!(port.reads.load(Ordering::Relaxed), 0);
    assert!(!valid_id("../escape"));
    assert!(!valid_relative("../escape"));
    assert!(!valid_relative("nested/../escape"));
}

#[test]
fn output_bounds_below_at_above() {
    let base = "x".repeat(SKILL_OUTPUT_BYTES_MAX - 32);
    assert!(
        render(
            "x",
            RegisteredSkillContent {
                instructions: base,
                companions: Vec::new()
            }
        )
        .is_ok()
    );
    let above = "x".repeat(SKILL_OUTPUT_BYTES_MAX + 1);
    assert!(
        render(
            "x",
            RegisteredSkillContent {
                instructions: above,
                companions: Vec::new()
            }
        )
        .is_err()
    );
}
