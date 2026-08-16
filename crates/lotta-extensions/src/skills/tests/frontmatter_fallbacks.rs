use super::*;

#[test]
fn path_derived_id_and_name() {
    let (_, id, name, description) = parse_skill_document(b"Body", "web/data-tools").unwrap();
    assert_eq!(id, "web/data-tools");
    assert_eq!(name, "Data Tools");
    assert_eq!(description, "Body");
}
#[test]
fn first_paragraph_including_crlf_and_bom() {
    let input = "\u{feff}---\r\nunknown: yes\r\n---\r\nFirst line\r\nsecond line\r\n\r\nLater";
    let (_, _, _, description) = parse_skill_document(input.as_bytes(), "skill").unwrap();
    assert_eq!(description, "First line\nsecond line");
}
#[test]
fn empty_body_uses_exact_literal() {
    let (_, _, _, description) = parse_skill_document(b"", "skill").unwrap();
    assert_eq!(description, "No description available");
}
#[test]
fn explicit_fields_are_trimmed_and_unquoted() {
    let input = b"---\nid: 'explicit/id'\nname: \"Explicit Name\"\ndescription: 'Exact description'\n---\nbody";
    let (_, id, name, description) = parse_skill_document(input, "fallback").unwrap();
    assert_eq!(
        (id.as_str(), name.as_str(), description.as_str()),
        ("explicit/id", "Explicit Name", "Exact description")
    );
}
#[test]
fn frontmatter_limits_below_at_above() {
    for (length, accepted) in [
        (limits::SKILL_FRONTMATTER_LINE_BYTES_MAX - 1, true),
        (limits::SKILL_FRONTMATTER_LINE_BYTES_MAX, true),
        (limits::SKILL_FRONTMATTER_LINE_BYTES_MAX + 1, false),
    ] {
        let input = format!("---\n{}\n---\n", "x".repeat(length));
        let result = parse_skill_document(input.as_bytes(), "x");
        assert_eq!(result.is_ok(), accepted, "frontmatter line length {length}");
        if !accepted {
            assert_eq!(result, Err(SkillError::LimitExceeded));
        }
    }
}

#[test]
fn unknown_list_and_block_continuations_are_ignored() {
    let input = b"---\ntags:\n - one\nmetadata: |\n  retained only as unknown text\n---\nFirst line\n  internally indented\n\nLater";
    let (_, _, _, description) = parse_skill_document(input, "skill").unwrap();
    assert_eq!(description, "First line\n  internally indented");
}

#[test]
fn invalid_utf8_and_nul_are_rejected() {
    assert_eq!(
        parse_skill_document(&[0xff], "skill"),
        Err(SkillError::Malformed)
    );
    assert_eq!(
        parse_skill_document(b"body\0text", "skill"),
        Err(SkillError::Malformed)
    );
}
