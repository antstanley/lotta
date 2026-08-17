use super::test_support::*;

#[test]
fn open_key_survives_and_known_keys_canonicalize() {
    let value = settings(&json!({
        "contextWindowLimit": 128_000,
        "reasoningEffort": "high",
        "reasoningTier": "premium",
        "endpoint": "http://localhost:1234",
        "providerType": "openai",
        "vendor_knob": {"x": 1},
    }));
    let serialized =
        serde_json::to_value(&value).unwrap_or_else(|error| panic!("serialize: {error}"));
    assert_eq!(serialized["context_window_limit"], 128_000);
    assert_eq!(serialized["base_url"], "http://localhost:1234");
    assert_eq!(serialized["vendor_knob"], json!({"x":1}));
}
#[test]
fn duplicate_aliases_are_rejected() {
    assert_eq!(
        ModelSettings::normalize(&json!({"endpoint":"a","base_url":"b"})),
        Err(ModelSettingsError::DuplicateAlias)
    );
}
#[test]
fn provider_type_conflict_is_rejected() {
    assert_eq!(
        ModelSettings::normalize(&json!({"provider_type":"openai","provider":"other"})),
        Err(ModelSettingsError::ProviderTypeConflict)
    );
}
#[test]
fn known_null_is_unset_while_unknown_values_are_lossless() {
    let nested = json!({"raw":null,"nested":{"x":null},"bytes":"A/B@:+._-"});
    let value = settings(&json!({
        "contextWindowLimit": null,
        "reasoningEffort": null,
        "vendor": nested,
    }));
    let serialized =
        serde_json::to_value(value).unwrap_or_else(|error| panic!("serialize: {error}"));
    assert!(serialized.get("context_window_limit").is_none());
    assert!(serialized.get("reasoning_effort").is_none());
    assert_eq!(serialized["vendor"], nested);
}

#[test]
fn deterministic_serialization() {
    let value = settings(&json!({"z":1,"a":2}));
    assert_eq!(
        serde_json::to_string(&value).unwrap_or_else(|error| panic!("serialize: {error}")),
        "{\"a\":2,\"z\":1}"
    );
}
