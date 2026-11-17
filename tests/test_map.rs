//! `eb-test-map` turns a build list into the tests that cover it.

use std::process::Command;

fn bin() -> Command {
    Command::new(env!("CARGO_BIN_EXE_eb-test-map"))
}

fn files(dir: &std::path::Path, map: &str, list: &str) {
    std::fs::write(dir.join("map.tsv"), map).unwrap();
    std::fs::write(dir.join("build.list"), list).unwrap();
}

#[test]
fn an_application_with_no_test_is_refused_and_a_toolchain_is_not() {
    let dir = tempfile::tempdir().unwrap();
    files(
        dir.path(),
        "GROMACS gromacs\nFFTW fftw\nGROMACS gromacs-perf\n",
        "GCCcore-13.2.0.eb\nGROMACS-2024.1-foss-2023b.eb\nFFTW-3.3.10-GCC-13.2.0.eb\n",
    );
    let o = bin()
        .args(["--map"])
        .arg(dir.path().join("map.tsv"))
        .arg("--list")
        .arg(dir.path().join("build.list"))
        .output()
        .unwrap();
    assert_eq!(
        o.status.code(),
        Some(0),
        "{}",
        String::from_utf8_lossy(&o.stderr)
    );
    assert_eq!(
        String::from_utf8(o.stdout).unwrap(),
        "fftw\ngromacs\ngromacs-perf\n"
    );
    assert!(o.stderr.is_empty());

    let list = "GCCcore-13.2.0.eb\nPython-3.11.5-GCCcore-12.3.0.eb\n";
    std::fs::write(dir.path().join("build.list"), list).unwrap();
    let o = bin()
        .args(["--map"])
        .arg(dir.path().join("map.tsv"))
        .arg("--list")
        .arg(dir.path().join("build.list"))
        .output()
        .unwrap();
    assert_eq!(
        o.status.code(),
        Some(2),
        "{}",
        String::from_utf8_lossy(&o.stderr)
    );
    assert!(
        o.stdout.is_empty(),
        "{}",
        String::from_utf8_lossy(&o.stdout)
    );
    let err = String::from_utf8(o.stderr).unwrap();
    assert!(err.contains("Python has no test"), "{err}");
}

#[test]
fn an_extra_toolchain_and_an_allowed_gap_are_named() {
    let dir = tempfile::tempdir().unwrap();
    files(
        dir.path(),
        "# site map\nGROMACS gromacs\n",
        "MyStack-2024a.eb\nGROMACS-2024.1-foss-2023b.eb\nMissing-1.0.eb\n",
    );
    let o = bin()
        .args(["--map"])
        .arg(dir.path().join("map.tsv"))
        .arg("--list")
        .arg(dir.path().join("build.list"))
        .args(["--toolchain", "MyStack", "--allow-unmapped"])
        .output()
        .unwrap();
    assert_eq!(
        o.status.code(),
        Some(0),
        "{}",
        String::from_utf8_lossy(&o.stderr)
    );
    assert_eq!(String::from_utf8(o.stdout).unwrap(), "gromacs\n");
    let err = String::from_utf8(o.stderr).unwrap();
    assert!(!err.contains("MyStack"), "{err}");
    assert!(err.contains("Missing has no test"), "{err}");
    assert!(err.contains("allowed"), "{err}");
}

#[test]
fn the_how_to_shows_the_command_the_checks_phase_can_run() {
    let page = std::fs::read_to_string(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/docs/orgmode/howto/run-tests-for-what-you-built.org"
    ))
    .unwrap();
    assert!(page.contains("eb-test-map --map"));
    assert!(page.contains("[checks]"));
    assert!(page.contains("has no test"));
}
