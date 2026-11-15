use eb_pipeline::lint::{deny_from_env, lint_example, lint_example_with};
use std::path::PathBuf;
use std::process::Command;

fn examples() -> Vec<PathBuf> {
    let dir = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("examples");
    let mut v: Vec<PathBuf> = std::fs::read_dir(dir)
        .unwrap()
        .map(|e| e.unwrap().path())
        .filter(|p| p.extension().is_some_and(|e| e == "toml"))
        .collect();
    v.sort();
    assert!(v.len() >= 2);
    v
}

#[test]
fn examples_pass_check() {
    for f in examples() {
        let o = Command::new(env!("CARGO_BIN_EXE_eb-pipeline"))
            .args(["check", "--site"])
            .arg(&f)
            .output()
            .unwrap();
        assert_eq!(
            o.status.code(),
            Some(0),
            "{}: {}",
            f.display(),
            String::from_utf8_lossy(&o.stderr)
        );
    }
}

#[test]
fn examples_hold_placeholders_only() {
    for f in examples() {
        let text = std::fs::read_to_string(&f).unwrap();
        // EB_PIPELINE_LINT_DENY adds an operator's own site names to refuse.
        let hits = lint_example_with(&text, &deny_from_env());
        assert!(hits.is_empty(), "{}: {hits:#?}", f.display());
    }
}

#[test]
fn lint_catches_a_site_value() {
    assert!(!lint_example("p = \"/home/someone/eb\"").is_empty());
    assert!(!lint_example("h = \"login.cluster.net\"").is_empty());
}
