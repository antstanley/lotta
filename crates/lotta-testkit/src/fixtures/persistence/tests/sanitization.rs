use super::*;

fn normalized_text(text: &str) -> String {
    let decoded = serde_json::from_str::<Value>(text)
        .map_or_else(|_| text.to_owned(), |value| value.to_string());
    decoded
        .chars()
        .filter(|value| !value.is_ascii_whitespace() && *value != '\\')
        .collect::<String>()
        .to_ascii_lowercase()
}

fn sanitization_violation(bytes: &[u8]) -> bool {
    let Ok(text) = std::str::from_utf8(bytes) else {
        return true;
    };
    let lower = normalized_text(text);
    let letters: String = lower.chars().filter(char::is_ascii_alphabetic).collect();
    let fragments = [
        ["s", "k", "-"].concat(),
        ["bear", "er"].concat(),
        ["g", "hp_"].concat(),
        ["x", "oxb-"].concat(),
        ["a", "kia"].concat(),
    ];
    fragments.iter().any(|fragment| lower.contains(fragment))
        || letters.contains(&["beginprivate", "key"].concat())
        || jwt_shape(&lower)
        || secret_fields(text)
}

fn jwt_shape(text: &str) -> bool {
    text.split(|value: char| !(value.is_ascii_alphanumeric() || matches!(value, '-' | '_' | '.')))
        .any(|word| {
            let mut parts = word.split('.');
            matches!(
                (parts.next(), parts.next(), parts.next(), parts.next()),
                (Some(a), Some(b), Some(c), None) if a.len() >= 8 && b.len() >= 8 && c.len() >= 8
            )
        })
}

fn secret_fields(text: &str) -> bool {
    let mut values = Vec::new();
    for line in text.lines() {
        if let Ok(value) = serde_json::from_str::<Value>(line) {
            collect_secret_fields(&value, &mut values);
        } else {
            collect_raw_fields(line, &mut values);
        }
    }
    values
        .into_iter()
        .any(|value| value != "<redacted-fixture>" && value != "not-needed")
}

fn collect_secret_fields(value: &Value, found: &mut Vec<String>) {
    match value {
        Value::Object(map) => {
            for (key, value) in map {
                if is_secret_key(key)
                    && let Some(text) = value.as_str()
                {
                    found.push(text.to_owned());
                }
                collect_secret_fields(value, found);
            }
        }
        Value::Array(values) => {
            for value in values {
                collect_secret_fields(value, found);
            }
        }
        _ => {}
    }
}

fn is_secret_key(key: &str) -> bool {
    matches!(
        key.to_ascii_lowercase().as_str(),
        "token"
            | "api_key"
            | "api-key"
            | "password"
            | "secret"
            | "access_token"
            | "refresh_token"
            | "refresh"
            | "idtoken"
    )
}

fn collect_raw_fields(line: &str, found: &mut Vec<String>) {
    let lower = line.to_ascii_lowercase();
    for key in [
        "api_key",
        "api-key",
        "password",
        "secret",
        "token:",
        "\"token\"",
        "access_token",
        "refresh_token",
    ] {
        let mut rest = lower.as_str();
        while let Some(at) = rest.find(key) {
            let suffix = &line[line.len() - rest.len() + at + key.len()..];
            let value = suffix
                .trim_start_matches(['"', '\'', ' ', ':', '='])
                .split(['"', '\'', ',', '}', ' ', '\n'])
                .next()
                .unwrap_or_default();
            if !value.is_empty() {
                found.push(value.to_owned());
            }
            rest = &rest[at + key.len()..];
        }
    }
}

#[test]
fn is_sanitized_all_actual_files() {
    let loader = FixtureLoader::new();
    let paths = loader
        .list_tree("persistence")
        .expect("complete fixture tree");
    assert_eq!(paths.len(), INVENTORY_ENTRIES + 1);
    assert_eq!(paths.len(), 85);
    for path in paths {
        let bytes = loader
            .load_bytes(format!("persistence/{path}"))
            .expect("fixture bytes");
        assert!(!sanitization_violation(&bytes), "unsafe {path}");
    }
}

#[test]
fn is_sanitized_mutation_matrix() {
    let pieces = [
        [
            "{\"token\":\"<redacted-fixture>\",\"nested\":{\"token\":\"",
            "bad\"}}",
        ]
        .concat(),
        ["{\"note\":\"Bear", " er fixture\"}"].concat(),
        ["{\"note\":\"-----BEGIN ", "PRIVATE KEY-----\"}"].concat(),
        ["{\"note\":\"g", "hp_fixture\"}"].concat(),
        ["{\"note\":\"A", "KIAFIXTURE\"}"].concat(),
        ["{\"note\":\"abcdefgh", ".ijklmnop.qrstuvwx\"}"].concat(),
        ["{\"note\":\"Bear\\u0065r", " fixture\"}"].concat(),
        ["{\"note\":\"-----BEGIN P R I V A T E ", " K E Y-----\"}"].concat(),
        [
            "{\"access_token\":\"",
            "bad\"}\n{\"refresh_token\":\"bad\"}",
        ]
        .concat(),
        ["token: ", "bad"].concat(),
        ["{\"token\":\"", "bad"].concat(),
    ];
    for (position, mutation) in pieces.iter().enumerate() {
        assert!(
            sanitization_violation(mutation.as_bytes()),
            "mutation {position}"
        );
    }
    assert!(sanitization_violation(&[0xff, 0xfe]));
    assert!(!sanitization_violation(br#"{"key":"<redacted-fixture>"}"#));
}
