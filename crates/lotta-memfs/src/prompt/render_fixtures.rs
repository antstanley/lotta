use super::render::{MemoryFile, inject_core_with_limit, parse_frontmatter, render_memfs};
use crate::tests::{content, path};
use lotta_runtime::RuntimeError;

fn memory_file(name: &str, value: &str) -> MemoryFile {
    MemoryFile::parse(&path(name), &content(value)).expect("valid memory fixture")
}

#[test]
fn mixed_external_tree_is_exact() {
    let files = [
        memory_file("z.md", "---\ndescription: z\n---\nz\n"),
        memory_file("a.md", "---\ndescription: a\n---\na\n"),
        memory_file("docs/z.md", "---\ndescription: z\n---\nz\n"),
        memory_file("docs/a.md", "---\ndescription: a\n---\na\n"),
        memory_file("docs/nested/b.md", "---\ndescription: b\n---\nb\n"),
        memory_file("alpha/x.md", "---\ndescription: x\n---\nx\n"),
    ];
    let rendered = render_memfs(&files).expect("bounded render");
    let external = rendered
        .split_once("<external_projection>")
        .expect("external start")
        .1
        .split_once("</external_projection>")
        .expect("external end")
        .0;
    assert_eq!(
        external,
        concat!(
            "\n${MEMORY_DIR}/\n",
            "├── alpha/\n│   └── x.md\n",
            "├── docs/\n│   ├── nested/\n│   │   └── b.md\n",
            "│   ├── a.md\n│   └── z.md\n",
            "├── a.md\n└── z.md\n",
        )
    );
    assert!(!external.contains("$MEMORY_DIR/"));
}

#[test]
fn frontmatter_description_and_body_are_exact() {
    let source = "---\ndescription: \"quoted \\\"value\\\"\"\n---\nbody\n---\nstill body\n";
    let (description, body) = parse_frontmatter(source).expect("frontmatter");
    assert_eq!(description, "quoted \"value\"");
    assert_eq!(body, "body\n---\nstill body\n");
}

#[test]
fn core_placeholder_absent_one_and_repeated_are_exact() {
    assert_eq!(
        inject_core_with_limit("raw", "core", 10).expect("absent"),
        "raw\n\ncore"
    );
    assert_eq!(
        inject_core_with_limit("a{CORE_MEMORY}b", "core", 6).expect("one"),
        "acoreb"
    );
    assert_eq!(
        inject_core_with_limit("{CORE_MEMORY}-{CORE_MEMORY}", "xy", 5).expect("repeated"),
        "xy-xy"
    );
}

#[test]
fn repeated_core_expansion_obeys_same_small_builder_cap() {
    assert_eq!(
        inject_core_with_limit("{CORE_MEMORY}{CORE_MEMORY}", "abcd", 8).expect("at cap"),
        "abcdabcd"
    );
    assert!(matches!(
        inject_core_with_limit("{CORE_MEMORY}{CORE_MEMORY}", "abcd", 7),
        Err(RuntimeError::LimitExceeded { .. })
    ));
    assert!(matches!(
        inject_core_with_limit("{CORE_MEMORY}{CORE_MEMORY}", "abcde", 8),
        Err(RuntimeError::LimitExceeded { .. })
    ));
}
