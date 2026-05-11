use super::*;

#[test]
fn ollama_provider_entry_enables_llama_cpp_cache_compat() {
    let entry = build_provider_entry(&ProviderKind::Ollama, None);

    assert!(entry.compat.supports_llama_cpp_cache);
    assert_eq!(entry.auth.as_deref(), Some("none"));
    assert_eq!(entry.api.as_deref(), Some("openai-completions"));
}

#[test]
fn ollama_provider_entry_uses_supplied_model_id() {
    let entry = build_provider_entry(&ProviderKind::Ollama, Some("qwen2.5-coder:14b"));
    assert_eq!(entry.models.len(), 1);
    assert_eq!(entry.models[0].id, "qwen2.5-coder:14b");
}

#[test]
fn ollama_provider_entry_default_model_is_a_recent_small_model() {
    // The default is a starting point — the user is expected to edit it
    // post-wizard. But it should at least not point at a 70B model that
    // a fresh Ollama install almost certainly does not have pulled.
    let entry = build_provider_entry(&ProviderKind::Ollama, None);
    assert_eq!(entry.models.len(), 1);
    assert!(
        !entry.models[0].id.contains("70b"),
        "default Ollama model should not require 70B weights",
    );
}
