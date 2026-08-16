use super::PromptSkill;
use super::skills::render_skills;

fn skill(name: &str, description: &str, location: Option<&str>) -> PromptSkill {
    PromptSkill::new(
        name.to_owned(),
        description.to_owned(),
        location.map(str::to_owned),
    )
    .expect("valid skill")
}

#[test]
fn pinned_skill_locations_and_tree_are_exact() {
    let skills = [
        skill(
            "pdf",
            "PDF tools\nignored",
            Some("/repo/skills/pdf/SKILL.md"),
        ),
        skill(
            "linear-cli",
            "Linear CLI",
            Some("/home/user/.letta/skills/linear-cli/SKILL.md"),
        ),
        skill("org/pdf", "Namespaced", Some("/other/pdf/SKILL.md")),
        skill("default", "Default", None),
        skill("fallback", "Fallback", Some("/opt/custom/readme.md")),
        skill("win", "Windows", Some(r"C:\repo\skills\win\SKILL.md")),
        skill("pdf", "duplicate loses", Some("/ignored/pdf/SKILL.md")),
        skill("nested/leaf", "Leaf", None),
        skill("nested/file", "File", None),
    ];
    assert_eq!(
        render_skills(&skills).expect("bounded skills"),
        concat!(
            "<available_skills>\n${MEMORY_DIR}/skills\n└── default/\n    └── SKILL.md (Default)\n",
            "\n${MEMORY_DIR}/skills/nested\n├── file/\n│   └── SKILL.md (File)\n└── leaf/\n    └──",
            " SKILL.md (Leaf)\n\n/home/user/.letta/skills\n└── linear-cli/\n    └── SKILL.md (Lin",
            "ear CLI)\n\n/opt/custom\n└── readme.md (Fallback)\n\n/other\n",
            "└── pdf/\n    └── SKILL.md (Namespaced)\n\n/repo/skills\n└── pdf/\n",
            "    └── SKILL.md (PDF tools)\n\nC:/repo/skill",
            "s\n└── win/\n    └── SKILL.md (Windows)\n</available_skills>",
        )
    );
}

#[test]
fn component_aware_display_validation_accepts_benign_dots() {
    assert!(PromptSkill::new("safe".into(), "x".into(), Some("/a/file..md".into())).is_ok());
    for location in ["/a/./x", "/a/../x", "/a/control\nname"] {
        assert!(PromptSkill::new("safe".into(), "x".into(), Some(location.into())).is_err());
    }
    for name in [".", "..", "bad\nname", "bad├──name", "bad/name/.."] {
        assert!(PromptSkill::new(name.into(), "x".into(), None).is_err());
    }
}
