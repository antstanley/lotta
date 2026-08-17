use super::test_support::*;

fn model(index: usize) -> ListedModel {
    ListedModel {
        handle: handle(&format!("m{index}")),
        readiness: ConnectionReadiness::Ready,
        model_settings: settings(&json!({})),
    }
}
#[test]
fn reports_readiness() {
    let catalog = ModelCatalog::new([("openai".into(), vec![model(0)])])
        .unwrap_or_else(|error| panic!("catalog: {error}"));
    let listed = catalog.list_models(&BTreeMap::from([(
        "openai".into(),
        ConnectionReadiness::Disconnected,
    )]));
    assert_eq!(listed[0].readiness, ConnectionReadiness::Disconnected);
    let degraded = catalog.list_models(&BTreeMap::from([(
        "openai".into(),
        ConnectionReadiness::Degraded,
    )]));
    assert_eq!(degraded[0].readiness, ConnectionReadiness::Degraded);
    let ready = catalog.list_models(&BTreeMap::from([(
        "openai".into(),
        ConnectionReadiness::Ready,
    )]));
    assert_eq!(ready[0].readiness, ConnectionReadiness::Ready);
    assert_eq!(
        catalog.list_models(&BTreeMap::new())[0].readiness,
        ConnectionReadiness::Disconnected
    );
}
#[test]
fn contains_no_credentials() {
    let listed = model(0);
    let debug = format!("{listed:?}");
    let wire = serde_json::to_string(&listed).unwrap_or_else(|error| panic!("wire: {error}"));
    for forbidden in ["auth", "token", "api_key", "secret"] {
        assert!(!debug.contains(forbidden));
        assert!(!wire.contains(forbidden));
    }
}
#[test]
fn below_limit_is_accepted() {
    assert!(ModelCatalog::new([("openai".into(), (0..9_999).map(model).collect())]).is_ok());
}
#[test]
fn at_limit_is_accepted() {
    assert!(
        ModelCatalog::new([(
            "openai".into(),
            (0..MODELS_PER_PROVIDER_MAX).map(model).collect()
        )])
        .is_ok()
    );
}
#[test]
fn above_limit_is_rejected_atomically() {
    assert!(matches!(
        ModelCatalog::new([(
            "openai".into(),
            (0..=MODELS_PER_PROVIDER_MAX).map(model).collect()
        )]),
        Err(ModelCatalogError::TooManyModels)
    ));
}
#[test]
fn duplicates_are_rejected() {
    assert!(matches!(
        ModelCatalog::new([("openai".into(), vec![model(0), model(0)])]),
        Err(ModelCatalogError::DuplicateModel)
    ));
}
#[test]
fn provider_mismatch_is_rejected() {
    assert!(matches!(
        ModelCatalog::new([("other".into(), vec![model(0)])]),
        Err(ModelCatalogError::ProviderMismatch)
    ));
}
