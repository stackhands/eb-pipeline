//! The integration smoke mounts EESSI and builds one package. It is weekly
//! and on demand, not a check on every pull request.

use std::path::PathBuf;

fn root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
}

#[test]
fn the_integration_smoke_is_scheduled_and_not_a_pull_request_check() {
    let workflow = root().join(".github/workflows/integration-smoke.yml");
    let text = std::fs::read_to_string(&workflow).unwrap();
    for needed in [
        "schedule:",
        "workflow_dispatch:",
        "eessi/github-action-eessi@834df09f7d60de47f10a7cbf59b470d69b9e2852 # v3.4.0",
        "eessi_stack_version: \"2026.06\"",
        "examples/integration-smoke.sh",
        "contents: read",
    ] {
        assert!(
            text.contains(needed),
            "{} lacks `{needed}`",
            workflow.display()
        );
    }
    assert!(
        !text.contains("pull_request"),
        "{} runs on a pull request",
        workflow.display()
    );
    let script = std::fs::read_to_string(root().join("examples/integration-smoke.sh")).unwrap();
    for name in [
        "eessi-arch-guard",
        "eb-local",
        "eb-keep-logs",
        "eb-staged-rebuild",
        "eb-pipeline",
        "eb-cvmfs",
        "eb-stack",
        "stack shard",
        "cvmfs_server",
    ] {
        assert!(script.contains(name), "the smoke script lacks {name}");
    }
}
