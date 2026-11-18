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
    assert!(page.contains("--include-resumed"));
    assert!(page.contains("--setvar modules="));
    let reference = std::fs::read_to_string(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/docs/orgmode/reference/test-map.org"
    ))
    .unwrap();
    assert!(reference.contains("--format eessi-modules"));
    assert!(reference.contains("--include-resumed"));
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
fn result_formats_keep_default_bytes_and_can_include_resumed_lines() {
    let dir = tempfile::tempdir().unwrap();
    let map = dir.path().join("map.tsv");
    let result = dir.path().join("result.json");
    std::fs::write(&map, "Done done-test\nResumed resumed-test\n").unwrap();
    std::fs::write(
        &result,
        r#"{
  "schema_version": 1,
  "kind": "result",
  "lines": [
    {"easyconfig": "Done-1.0.eb", "status": "success", "phase": "build"},
    {"easyconfig": "Resumed-2.0.eb", "status": "skipped", "phase": "build", "note": "resumed: the unit succeeded and its outputs are still present"},
    {"easyconfig": "Blocked-3.0.eb", "status": "skipped", "phase": "build", "note": "an earlier unit was refused"},
    {"easyconfig": "Failed-4.0.eb", "status": "failed", "phase": "build"}
  ]
}"#,
    )
    .unwrap();

    let run = |format: &str, include_resumed: bool| {
        let mut command = bin();
        command
            .args(["--result"])
            .arg(&result)
            .args(["--format", format]);
        if format != "eessi-modules" {
            command.args(["--map"]).arg(&map);
        }
        if include_resumed {
            command.arg("--include-resumed");
        }
        command.output().unwrap()
    };

    for (format, expected) in [
        ("text", "done-test\n"),
        ("reframe", "-n ^done-test$\n"),
        ("eessi-modules", "--setvar modules=Done/1.0\n"),
    ] {
        let o = run(format, false);
        assert_eq!(o.status.code(), Some(0));
        assert_eq!(o.stdout, expected.as_bytes(), "format={format}");
        assert_eq!(o.stderr, b"", "format={format}");
    }

    let o = run("text", true);
    assert_eq!(o.status.code(), Some(0));
    assert_eq!(o.stdout, b"done-test\nresumed-test\n");
    assert_eq!(o.stderr, b"");

    let o = run("eessi-modules", true);
    assert_eq!(o.status.code(), Some(0));
    assert_eq!(o.stdout, b"--setvar modules=Done/1.0,Resumed/2.0\n");
    assert_eq!(o.stderr, b"");
}

#[test]
fn include_resumed_requires_result_input() {
    let dir = tempfile::tempdir().unwrap();
    let list = dir.path().join("build.list");
    std::fs::write(&list, "Done-1.0.eb\n").unwrap();
    let o = bin()
        .args(["--format", "eessi-modules", "--include-resumed", "--list"])
        .arg(&list)
        .output()
        .unwrap();
    assert_eq!(o.status.code(), Some(2));
    assert_eq!(o.stdout, b"");
    assert_eq!(
        o.stderr,
        b"eb-test-map: --include-resumed requires --result\n"
    );
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

#[test]
fn version_flags_write_to_stdout_and_exit_zero() {
    let expected = format!("eb-test-map {}\n", env!("CARGO_PKG_VERSION")).into_bytes();
    for flag in ["-V", "--version"] {
        let o = bin().arg(flag).output().unwrap();
        assert_eq!(o.status.code(), Some(0), "{flag}: {:?}", o.stderr);
        assert_eq!(o.stdout, expected, "{flag}");
        assert!(o.stderr.is_empty(), "{flag}: {:?}", o.stderr);
    }
}

#[test]
fn duplicate_single_value_flags_are_refused() {
    let dir = tempfile::tempdir().unwrap();
    let map = dir.path().join("map.tsv");
    std::fs::write(&map, "G g\n").unwrap();
    let list1 = dir.path().join("l1.txt");
    std::fs::write(&list1, "G-1.0.eb\n").unwrap();
    let list2 = dir.path().join("l2.txt");
    std::fs::write(&list2, "Missing-1.0.eb\n").unwrap();
    let res = dir.path().join("result.json");
    std::fs::write(
        &res,
        r#"{"kind":"result","exit_code":0,"lines":[{"easyconfig":"G-1.0.eb","status":"success"}]}"#,
    )
    .unwrap();

    for (flag, args) in [
        (
            "--map",
            vec![
                "--map".to_string(),
                map.display().to_string(),
                format!("--map={}", map.display()),
                "--list".to_string(),
                list1.display().to_string(),
            ],
        ),
        (
            "--list",
            vec![
                "--map".to_string(),
                map.display().to_string(),
                "--list".to_string(),
                list1.display().to_string(),
                "--list".to_string(),
                list2.display().to_string(),
            ],
        ),
        (
            "--result",
            vec![
                "--map".to_string(),
                map.display().to_string(),
                "--result".to_string(),
                res.display().to_string(),
                format!("--result={}", res.display()),
            ],
        ),
        (
            "--format",
            vec![
                "--map".to_string(),
                map.display().to_string(),
                "--list".to_string(),
                list1.display().to_string(),
                "--format".to_string(),
                "text".to_string(),
                "--format=reframe".to_string(),
            ],
        ),
    ] {
        let o = bin().args(&args).output().unwrap();
        assert_eq!(
            o.status.code(),
            Some(2),
            "{flag}: stderr={}",
            String::from_utf8_lossy(&o.stderr)
        );
        assert!(
            String::from_utf8_lossy(&o.stderr).contains("was given twice"),
            "{flag}: stderr={}",
            String::from_utf8_lossy(&o.stderr)
        );
    }
}

#[test]
fn test_map_succeeds_when_stdout_consumer_closes() {
    let dir = tempfile::tempdir().unwrap();
    let map = dir.path().join("map.tsv");
    std::fs::write(&map, "G g\n").unwrap();
    let list = dir.path().join("l.txt");
    std::fs::write(&list, "G-1.0.eb\n").unwrap();

    for format in ["text", "eessi-modules"] {
        let mut fds = [0; 2];
        unsafe {
            assert_eq!(libc::pipe(fds.as_mut_ptr()), 0);
            assert_eq!(libc::close(fds[0]), 0);
        }
        let closed =
            unsafe { <std::process::Stdio as std::os::fd::FromRawFd>::from_raw_fd(fds[1]) };
        let mut cmd = bin();
        cmd.args(["--list"]).arg(&list).args(["--format", format]);
        if format != "eessi-modules" {
            cmd.args(["--map"]).arg(&map);
        }
        let o = cmd.stdout(closed).output().unwrap();
        assert_eq!(
            o.status.code(),
            Some(0),
            "{format}: {}",
            String::from_utf8_lossy(&o.stderr)
        );
    }
}
