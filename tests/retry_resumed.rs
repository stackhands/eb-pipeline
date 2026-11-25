//! Resume-aware defaults for `eb-retry-list`.
#![allow(clippy::disallowed_methods)]

use std::process::Command;

fn bin() -> Command {
    Command::new(env!("CARGO_BIN_EXE_eb-retry-list"))
}

#[test]
fn default_retry_omits_resume_skips_from_results_and_summaries() {
    let dir = tempfile::tempdir().unwrap();
    let list = dir.path().join("build.list");
    let result = dir.path().join("result.json");
    let summary = dir.path().join("summary.json");
    std::fs::write(&list, "Already-1.0.eb --robot\nFailed-1.0.eb --rebuild\n").unwrap();
    std::fs::write(
        &result,
        concat!(
            "{\"kind\":\"result\",\"exit_code\":1,\"lines\":[",
            "{\"easyconfig\":\"Already-1.0.eb\",\"status\":\"skipped\",",
            "\"note\":\"resumed: the unit succeeded and its outputs are still present\"},",
            "{\"easyconfig\":\"Failed-1.0.eb\",\"status\":\"failed\"}",
            "]}\n",
        ),
    )
    .unwrap();
    std::fs::write(
        &summary,
        concat!(
            "{\"kind\":\"summary\",\"targets\":[{\"name\":\"cpu\",\"exit_code\":1,",
            "\"packages\":[",
            "{\"easyconfig\":\"Already-1.0.eb\",\"status\":\"skipped\",",
            "\"reason\":\"resumed: the unit succeeded and its outputs are still present\"},",
            "{\"easyconfig\":\"Failed-1.0.eb\",\"status\":\"failed\"}",
            "]}]}\n",
        ),
    )
    .unwrap();

    for args in [
        vec![
            "--result",
            result.to_str().unwrap(),
            "--list",
            list.to_str().unwrap(),
        ],
        vec![
            "--summary",
            summary.to_str().unwrap(),
            "--target",
            "cpu",
            "--list",
            list.to_str().unwrap(),
        ],
    ] {
        let output = bin().args(args).output().unwrap();
        assert_eq!(
            output.status.code(),
            Some(0),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
        assert_eq!(output.stdout, b"Failed-1.0.eb --rebuild\n");
        assert!(output.stderr.is_empty());
    }
}

#[test]
fn explicit_skipped_status_includes_resume_skips() {
    let dir = tempfile::tempdir().unwrap();
    let list = dir.path().join("build.list");
    let result = dir.path().join("result.json");
    std::fs::write(&list, "Already-1.0.eb --robot\n").unwrap();
    std::fs::write(
        &result,
        concat!(
            "{\"kind\":\"result\",\"exit_code\":1,\"lines\":[",
            "{\"easyconfig\":\"Already-1.0.eb\",\"status\":\"skipped\",",
            "\"note\":\"resumed: the unit succeeded and its outputs are still present\"}",
            "]}\n",
        ),
    )
    .unwrap();

    let output = bin()
        .args(["--result"])
        .arg(&result)
        .args(["--list"])
        .arg(&list)
        .args(["--status", "skipped"])
        .output()
        .unwrap();

    assert_eq!(output.status.code(), Some(0));
    assert_eq!(output.stdout, b"Already-1.0.eb --robot\n");
    assert!(output.stderr.is_empty());
}
