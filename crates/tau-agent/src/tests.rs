use tau_config::settings::ModelRegistry;

#[test]
fn no_config_resolves_none() {
    let models = ModelRegistry::default();
    assert!(tau_provider::resolve("fake/model", &models).is_none());
}
