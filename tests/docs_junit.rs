use std::path::PathBuf;
use std::process::Command;

#[test]
fn junit_reference_documents_env_result_fallback() {
    let help = Command::new(env!("CARGO_BIN_EXE_eb-junit"))
        .arg("--help")
        .output()
        .unwrap();
    assert_eq!(help.status.code(), Some(0));
    let help_text = String::from_utf8_lossy(&help.stdout);
    assert!(help_text.contains("EB_PIPELINE_RESULT"));

    let junit_path =
        PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("docs/orgmode/reference/junit.org");
    let junit = std::fs::read_to_string(&junit_path).unwrap();
    assert!(junit.contains("=EB_PIPELINE_RESULT="));
}
