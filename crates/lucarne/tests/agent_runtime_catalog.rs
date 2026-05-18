#[test]
fn agent_runtime_does_not_expose_public_default_provider_catalog() {
    let manifest = std::path::Path::new(env!("CARGO_MANIFEST_DIR"));
    let module =
        std::fs::read_to_string(manifest.join("src/agent_runtime/mod.rs")).expect("runtime module");
    let catalog = std::fs::read_to_string(manifest.join("src/agent_runtime/catalog.rs"))
        .expect("runtime catalog");

    for forbidden in [
        "DEFAULT_AGENT_PROVIDERS",
        "default_provider_ids",
        "known_provider",
        "provider_label",
    ] {
        assert!(
            !module.contains(forbidden),
            "agent_runtime module must not publicly expose {forbidden}"
        );
        assert!(
            !catalog.contains(forbidden),
            "agent_runtime catalog must not own {forbidden}"
        );
    }
}
