//! `eb-test-map` turns a build list into the tests that cover it.

use std::process::Command;

fn bin() -> Command {
    Command::new(env!("CARGO_BIN_EXE_eb-test-map"))
}

fn files(dir: &std::path::Path, map: &str, list: &str) {
    std::fs::write(dir.join("map.tsv"), map).unwrap();
    std::fs::write(dir.join("build.list"), list).unwrap();
}

fn result_file(dir: &std::path::Path, name: &str, body: &str) -> std::path::PathBuf {
    let path = dir.join(name);
    std::fs::write(&path, body).unwrap();
    path
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
    assert!(page.contains("--format reframe"));
    assert!(page.contains("--format eessi-modules"));
    assert!(page.contains("--setvar modules="));
    let reference = std::fs::read_to_string(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/docs/orgmode/reference/test-map.org"
    ))
    .unwrap();
    assert!(reference.contains("--format eessi-modules"));
    assert!(reference.contains("--setvar modules="));
    assert!(reference.contains("set_modules"));
}

#[test]
fn the_default_format_still_requires_a_map() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(
        dir.path().join("build.list"),
        "GROMACS-2024.1-foss-2023b.eb\n",
    )
    .unwrap();
    let o = bin()
        .arg("--list")
        .arg(dir.path().join("build.list"))
        .output()
        .unwrap();
    assert_eq!(o.status.code(), Some(2));
    assert!(o.stdout.is_empty());
    let err = String::from_utf8(o.stderr).unwrap();
    assert!(err.contains("--map is required"), "{err}");
    assert!(!err.contains("--list is required"), "{err}");
}

#[test]
fn eessi_modules_prints_one_setvar_argument_without_a_map() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(
        dir.path().join("build.list"),
        "\
# comment
GCCcore-13.2.0.eb
GROMACS-2024.1-foss-2023b.eb
/lists/Python-3.11.5-GCCcore-12.3.0.eb
zlib-1.3.1.eb
libfoo-bar-1.2.eb
GROMACS-2024.1-foss-2023b.eb
GROMACS-2024.1-foss-2023b-CUDA-12.1.0.eb
foss-2023b.eb
MyStack-2024a.eb
",
    )
    .unwrap();
    let o = bin()
        .args(["--format", "eessi-modules", "--toolchain", "MyStack"])
        .arg("--list")
        .arg(dir.path().join("build.list"))
        .arg("--map")
        .arg(dir.path().join("missing.tsv"))
        .output()
        .unwrap();
    assert_eq!(
        o.status.code(),
        Some(0),
        "{}",
        String::from_utf8_lossy(&o.stderr)
    );
    assert!(
        o.stderr.is_empty(),
        "{}",
        String::from_utf8_lossy(&o.stderr)
    );
    assert_eq!(
        String::from_utf8(o.stdout).unwrap(),
        "--setvar modules=GROMACS/2024.1-foss-2023b,GROMACS/2024.1-foss-2023b-CUDA-12.1.0,Python/3.11.5-GCCcore-12.3.0,libfoo-bar/1.2,zlib/1.3.1\n"
    );
}

#[test]
fn eessi_modules_prints_nothing_when_the_list_has_no_application() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(
        dir.path().join("build.list"),
        "# only a toolchain\n\nGCC-13.2.0.eb\n",
    )
    .unwrap();
    let o = bin()
        .args(["--format=eessi-modules"])
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
    assert!(o.stdout.is_empty());
    assert!(o.stderr.is_empty());
}

#[test]
fn eessi_modules_refuses_a_name_that_cannot_be_one_list_entry() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(dir.path().join("build.list"), "Foo-bar.eb\n").unwrap();
    let o = bin()
        .args(["--format", "eessi-modules", "--list"])
        .arg(dir.path().join("build.list"))
        .output()
        .unwrap();
    assert_eq!(o.status.code(), Some(2));
    assert!(o.stdout.is_empty());
    let err = String::from_utf8(o.stderr).unwrap();
    assert!(err.contains("has no version"), "{err}");

    std::fs::write(dir.path().join("build.list"), "Foo-1,2.eb\n").unwrap();
    let o = bin()
        .args(["--format", "eessi-modules", "--list"])
        .arg(dir.path().join("build.list"))
        .output()
        .unwrap();
    assert_eq!(o.status.code(), Some(2));
    let err = String::from_utf8(o.stderr).unwrap();
    assert!(err.contains("comma or a space"), "{err}");
}

#[test]
fn an_unknown_format_is_refused_and_text_matches_the_default() {
    let dir = tempfile::tempdir().unwrap();
    files(
        dir.path(),
        "GROMACS gromacs\n",
        "GROMACS-2024.1-foss-2023b.eb\n",
    );
    let bad = bin()
        .arg("--map")
        .arg(dir.path().join("map.tsv"))
        .arg("--list")
        .arg(dir.path().join("build.list"))
        .args(["--format", "nope"])
        .output()
        .unwrap();
    assert_eq!(bad.status.code(), Some(2));
    let err = String::from_utf8(bad.stderr).unwrap();
    assert!(err.contains("unknown --format"), "{err}");
    assert!(err.contains("eessi-modules"), "{err}");

    let text = bin()
        .arg("--map")
        .arg(dir.path().join("map.tsv"))
        .arg("--list")
        .arg(dir.path().join("build.list"))
        .args(["--format", "text"])
        .output()
        .unwrap();
    assert_eq!(
        text.status.code(),
        Some(0),
        "{}",
        String::from_utf8_lossy(&text.stderr)
    );
    assert_eq!(String::from_utf8(text.stdout).unwrap(), "gromacs\n");
}

#[test]
fn a_yaml_list_is_read_as_an_easystack() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(
        dir.path().join("map.tsv"),
        "GROMACS gromacs\nFFTW fftw\n".as_bytes(),
    )
    .unwrap();
    std::fs::write(
        dir.path().join("build.yaml"),
        "easyconfigs:\n  - GROMACS-2024.1-foss-2023b.eb\n  - FFTW-3.3.10-GCC-13.2.0.eb\n",
    )
    .unwrap();
    let o = bin()
        .arg("--map")
        .arg(dir.path().join("map.tsv"))
        .arg("--list")
        .arg(dir.path().join("build.yaml"))
        .output()
        .unwrap();
    assert_eq!(
        o.status.code(),
        Some(0),
        "{}",
        String::from_utf8_lossy(&o.stderr)
    );
    assert_eq!(String::from_utf8(o.stdout).unwrap(), "fftw\ngromacs\n");
}

#[test]
fn only_success_lines_are_kept_from_result() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(dir.path().join("map.tsv"), "GROMACS gromacs\nFFTW fftw\n").unwrap();
    let result = result_file(
        dir.path(),
        "result.json",
        r#"{
  "schema_version": 1,
  "kind": "result",
  "site": "example",
  "target": "cpu",
  "build_type": "production",
  "run_id": "nightly",
  "exit_code": 1,
  "lines": [
    {"easyconfig": "GROMACS-2024.1-foss-2023b.eb", "status": "success", "phase": "build", "unit": "all"},
    {"easyconfig": "FFTW-3.3.10-GCC-13.2.0.eb", "status": "failed", "phase": "build", "unit": "all"},
    {"easyconfig": "Missing-1.0.eb", "status": "success", "phase": "build", "unit": "all"}
  ]
}"#,
    );
    let o = bin()
        .arg("--map")
        .arg(dir.path().join("map.tsv"))
        .arg("--result")
        .arg(result)
        .arg("--allow-unmapped")
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
    assert!(err.contains("Missing has no test"), "{err}");
    assert!(!err.contains("FFTW"), "{err}");
}

#[test]
fn reframe_output_escapes_regex_metacharacters() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(
        dir.path().join("map.tsv"),
        "GROMACS gromacs+gpu(v2)\n".as_bytes(),
    )
    .unwrap();
    std::fs::write(
        dir.path().join("build.list"),
        "GROMACS-2024.1-foss-2023b.eb\n".as_bytes(),
    )
    .unwrap();
    let o = bin()
        .arg("--map")
        .arg(dir.path().join("map.tsv"))
        .arg("--list")
        .arg(dir.path().join("build.list"))
        .arg("--format")
        .arg("reframe")
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
        "-n ^gromacs\\+gpu\\(v2\\)$\n"
    );
}

#[test]
fn reframe_output_escapes_dot_and_libstdcpp() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(
        dir.path().join("map.tsv"),
        "GROMACS gromacs.2024\nFFTW libstdc++\n".as_bytes(),
    )
    .unwrap();
    std::fs::write(
        dir.path().join("build.list"),
        "GROMACS-2024.1-foss-2023b.eb\nFFTW-3.3.10-GCC-13.2.0.eb\n".as_bytes(),
    )
    .unwrap();
    let o = bin()
        .arg("--map")
        .arg(dir.path().join("map.tsv"))
        .arg("--list")
        .arg(dir.path().join("build.list"))
        .arg("--format")
        .arg("reframe")
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
        "-n ^gromacs\\.2024$\n-n ^libstdc\\+\\+$\n"
    );
}
