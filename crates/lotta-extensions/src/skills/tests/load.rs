use super::*;
use std::fs;
use std::path::PathBuf;
use std::sync::atomic::{AtomicUsize, Ordering};
static NEXT: AtomicUsize = AtomicUsize::new(0);

struct Fixture {
    base: PathBuf,
    skill: Skill,
}
impl Fixture {
    fn new(instructions: &[u8]) -> Self {
        let base = std::env::temp_dir().join(format!(
            "lotta-load-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        let _ = fs::remove_dir_all(&base);
        let root = base.join("skills");
        let directory = root.join("demo");
        fs::create_dir_all(&directory).unwrap();
        let file = directory.join("SKILL.md");
        fs::write(&file, instructions).unwrap();
        let skill = Skill {
            id: "demo".into(),
            name: "Demo".into(),
            description: "demo".into(),
            source: SkillSource::Project,
            root,
            skill_file: file,
        };
        Self { base, skill }
    }
}
impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.base);
    }
}

#[test]
fn complete_multiline_instructions_and_companions_exact_sorted() {
    let fixture = Fixture::new(b"---\nid: demo\n---\nline one\nline two\nline three\n");
    let directory = fixture.skill.skill_file.parent().unwrap();
    fs::create_dir(directory.join("nested")).unwrap();
    fs::write(directory.join("z.sh"), b"#!/bin/sh\nexit 0\n").unwrap();
    fs::write(directory.join("nested/a.bin"), [0, 255, 1]).unwrap();
    fs::write(directory.join("a.txt"), b"text").unwrap();
    let loaded = SkillLoader::load(&fixture.skill).unwrap();
    assert!(
        loaded
            .document
            .instructions
            .ends_with("line one\nline two\nline three\n")
    );
    assert_eq!(
        loaded
            .companions
            .iter()
            .map(|item| item.relative_path.as_str())
            .collect::<Vec<_>>(),
        ["a.txt", "nested/a.bin", "z.sh"]
    );
    assert_eq!(loaded.companions[1].bytes, [0, 255, 1]);
}
#[test]
fn missing_and_changed_entry_fail_closed() {
    let fixture = Fixture::new(b"---\nid: demo\n---\nbody");
    fs::write(&fixture.skill.skill_file, b"---\nid: changed\n---\nbody").unwrap();
    assert_eq!(
        SkillLoader::load(&fixture.skill),
        Err(SkillError::ChangedEntry)
    );
    fs::remove_file(&fixture.skill.skill_file).unwrap();
    assert_eq!(
        SkillLoader::load(&fixture.skill),
        Err(SkillError::MissingEntry)
    );
}
#[cfg(unix)]
#[test]
fn symlink_escape_is_rejected() {
    use std::os::unix::fs::symlink;
    let fixture = Fixture::new(b"---\nid: demo\n---\nbody");
    let outside = fixture.base.join("outside");
    fs::write(&outside, b"secret").unwrap();
    symlink(
        outside,
        fixture.skill.skill_file.parent().unwrap().join("escape"),
    )
    .unwrap();
    assert_eq!(
        SkillLoader::load(&fixture.skill),
        Err(SkillError::SpecialFile)
    );
}
#[test]
fn companion_file_bound_below_at_above() {
    let fixture = Fixture::new(b"---\nid: demo\n---\nbody");
    let path = fixture.skill.skill_file.parent().unwrap().join("data.bin");
    for (length, accepted) in [
        (limits::SKILL_COMPANION_FILE_BYTES_MAX - 1, true),
        (limits::SKILL_COMPANION_FILE_BYTES_MAX, true),
        (limits::SKILL_COMPANION_FILE_BYTES_MAX + 1, false),
    ] {
        fs::write(&path, vec![0; length]).unwrap();
        let result = SkillLoader::load(&fixture.skill);
        assert_eq!(result.is_ok(), accepted, "companion bytes {length}");
        if !accepted {
            assert_eq!(result, Err(SkillError::LimitExceeded));
        }
    }
}
