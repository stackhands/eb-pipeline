//! XML-safe testcase name handling for `eb-junit`.
#![allow(clippy::disallowed_methods)]

use std::process::Command;

fn bin() -> Command {
    Command::new(env!("CARGO_BIN_EXE_eb-junit"))
}

#[test]
fn xml_character_removal_does_not_collapse_testcase_names() {
    let dir = tempfile::tempdir().unwrap();
    let result = dir.path().join("result.json");
    std::fs::write(
        &result,
        concat!(
            "{\"schema_version\":1,\"kind\":\"result\",",
            "\"site\":\"example\",\"target\":\"cpu\",",
            "\"build_type\":\"production\",\"run_id\":\"nightly\",\"exit_code\":1,",
            "\"lines\":[",
            "{\"easyconfig\":\"A\\u0001.eb\",\"unit\":\"all\",\"phase\":\"build\",\"status\":\"failed\"},",
            "{\"easyconfig\":\"A\\u0002.eb\",\"unit\":\"all\",\"phase\":\"build\",\"status\":\"failed\"}",
            "]}\n",
        ),
    )
    .unwrap();

    let output = bin().arg("--result").arg(&result).output().unwrap();

    assert_eq!(
        output.status.code(),
        Some(0),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(output.stderr.is_empty());
    let xml = String::from_utf8(output.stdout).unwrap();
    assert!(xml.contains("name=\"cpu/A.eb\""), "{xml}");
    assert!(xml.contains("name=\"cpu/A.eb #2\""), "{xml}");
}
