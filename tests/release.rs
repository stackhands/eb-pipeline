use std::path::PathBuf;

#[test]
fn release_provenance_covers_binary_and_source_archives() {
    let workflow = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join(".github/workflows/release.yml");
    let text = std::fs::read_to_string(&workflow).unwrap();
    let attest = text
        .split("uses: actions/attest-build-provenance@")
        .nth(1)
        .unwrap()
        .split("- name: draft release")
        .next()
        .unwrap();

    assert!(attest.contains("artifacts/*.tar.xz"), "{attest}");
    assert!(attest.contains("artifacts/source.tar.gz"), "{attest}");
}
