//! Bidirectional protocol fixture manifest tests.

use lotta_protocol::{ALL_COMMAND_DISCRIMINANTS, ALL_MESSAGE_DISCRIMINANTS};
use serde::Deserialize;
use std::collections::HashSet;

const PIN: &str = "300f923f16cc8eee50656d7da732902c1dea2b65";

#[derive(Deserialize)]
struct Fixture {
    schema_version: u32,
    source_commit: String,
    commands: UnionRecord,
    messages: UnionRecord,
}

#[derive(Deserialize)]
struct UnionRecord {
    entrypoint: String,
    source: String,
    declaration: String,
    discriminants: Vec<String>,
}

fn fixture() -> Fixture {
    serde_json::from_str(include_str!(
        "../../../fixtures/protocol/discriminants.json"
    ))
    .unwrap_or_else(|error| panic!("bounded fixture must parse: {error}"))
}

fn compare(fixture: &[String], variants: &[&str]) -> Result<(), String> {
    let fixture_set: HashSet<_> = fixture.iter().map(String::as_str).collect();
    let variant_set: HashSet<_> = variants.iter().copied().collect();
    if fixture_set.len() != fixture.len() {
        return Err("duplicate fixture discriminant".into());
    }
    if variant_set.len() != variants.len() {
        return Err("duplicate variant discriminant".into());
    }
    if let Some(missing) = fixture_set.difference(&variant_set).next() {
        return Err(format!("fixture discriminant has no variant: {missing}"));
    }
    if let Some(extra) = variant_set.difference(&fixture_set).next() {
        return Err(format!("variant discriminant has no fixture: {extra}"));
    }
    Ok(())
}

fn assert_header(value: &Fixture) {
    assert_eq!(value.schema_version, 1);
    assert_eq!(value.source_commit, PIN);
    assert_eq!(value.commands.entrypoint, "src/types/protocol_v2.ts");
    assert_eq!(value.commands.source, "src/types/protocol_v2.ts");
    assert_eq!(value.commands.declaration, "WsProtocolCommand");
    assert_eq!(
        value.messages.entrypoint,
        "src/types/app-server-protocol.ts"
    );
    assert_eq!(value.messages.source, "src/types/protocol_v2.ts");
    assert_eq!(value.messages.declaration, "WsProtocolMessage");
}

mod manifest {
    use super::*;

    #[test]
    fn every_fixture_has_a_variant() {
        let value = fixture();
        assert_header(&value);
        assert!(compare(&value.commands.discriminants, ALL_COMMAND_DISCRIMINANTS).is_ok());
        assert!(compare(&value.messages.discriminants, ALL_MESSAGE_DISCRIMINANTS).is_ok());
    }

    #[test]
    fn every_variant_has_a_fixture() {
        let value = fixture();
        assert!(compare(&value.commands.discriminants, ALL_COMMAND_DISCRIMINANTS).is_ok());
        assert!(compare(&value.messages.discriminants, ALL_MESSAGE_DISCRIMINANTS).is_ok());
    }

    #[test]
    fn fixture_mutations_fail_same_comparator() {
        let value = fixture();
        let mut missing = value.commands.discriminants.clone();
        missing.pop();
        assert!(compare(&missing, ALL_COMMAND_DISCRIMINANTS).is_err());
        let mut extra = value.messages.discriminants.clone();
        extra.push("vendor_extra".into());
        assert!(compare(&extra, ALL_MESSAGE_DISCRIMINANTS).is_err());
        let mut duplicate = value.commands.discriminants;
        duplicate.push(duplicate[0].clone());
        assert!(compare(&duplicate, ALL_COMMAND_DISCRIMINANTS).is_err());
    }
}
