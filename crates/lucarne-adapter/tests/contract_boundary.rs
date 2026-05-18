use std::process::Command;

#[test]
fn adapter_contract_has_no_channel_or_platform_dependencies() {
    let output = Command::new("cargo")
        .args(["tree", "-p", "lucarne-adapter", "--no-dev"])
        .output()
        .expect("run cargo tree");
    let stdout = String::from_utf8(output.stdout).unwrap();
    assert!(
        !stdout.contains("lucarne-channel")
            && !stdout.contains("lucarne-telegram")
            && !stdout.contains("teloxide"),
        "adapter contract must stay platform-neutral:\n{stdout}"
    );
}

#[test]
fn adapter_registry_emits_structured_tracing() {
    let source = std::fs::read_to_string(
        std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("src/lib.rs"),
    )
    .expect("read adapter contract source");
    for needle in [
        "target: \"lucarne_adapter\"",
        "adapter plugin registered",
        "adapter plugin enabled",
        "enabled adapter plugins spawned",
    ] {
        assert!(
            source.contains(needle),
            "adapter registry tracing must include {needle}"
        );
    }
}
