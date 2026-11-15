use eb_pipeline::lint::lint_example;
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
        let hits = lint_example(&text);
        assert!(hits.is_empty(), "{}: {hits:#?}", f.display());
    }
}

#[test]
fn lint_catches_a_site_value() {
    assert!(!lint_example("p = \"/home/someone/eb\"").is_empty());
    assert!(!lint_example("h = \"login.cluster.net\"").is_empty());
}
