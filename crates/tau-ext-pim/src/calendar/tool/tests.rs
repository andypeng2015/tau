use super::*;

#[test]
fn calendar_schema_hides_timezone_and_has_command_conditionals() {
    // Weak local models need command-specific schema constraints rather
    // than prose-only guidance. Keep timezone out of model-visible args.
    let schema = calendar_tool_spec().parameters.expect("parameters");
    let args_properties = schema
        .pointer("/properties/args/properties")
        .and_then(serde_json::Value::as_object)
        .expect("args properties");

    assert!(!args_properties.contains_key("timezone"));
    assert!(
        schema
            .get("allOf")
            .and_then(serde_json::Value::as_array)
            .is_some_and(|rules| 7 < rules.len())
    );
}
