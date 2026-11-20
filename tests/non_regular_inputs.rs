//! Non-regular input paths are refused promptly across sibling binaries.

use std::ffi::CString;
use std::os::unix::ffi::OsStrExt;
use std::path::Path;
use std::process::{Child, Command, Output, Stdio};
use std::time::{Duration, Instant};

fn wait_output_with_timeout(mut child: Child, label: &str) -> Output {
    let deadline = Instant::now() + Duration::from_secs(5);
    loop {
        if child.try_wait().unwrap().is_some() {
            return child.wait_with_output().unwrap();
        }
        if Instant::now() >= deadline {
            let _ = child.kill();
            let _ = child.wait();
            panic!("{label} blocked on a non-regular input path");
        }
        std::thread::sleep(Duration::from_millis(10));
    }
}

fn make_fifo(path: &Path) {
    let raw = CString::new(path.as_os_str().as_bytes()).unwrap();
    assert_eq!(unsafe { libc::mkfifo(raw.as_ptr(), 0o600) }, 0);
}

fn write_file(path: &Path, text: &str) {
    std::fs::write(path, text).unwrap();
}

#[test]
fn binaries_refuse_or_finish_promptly_for_fifo_and_directory_inputs() {
    let dir = tempfile::tempdir().unwrap();
    let map = dir.path().join("map.tsv");
    let result = dir.path().join("result.json");
    let existing_ok = dir.path().join("existing.tsv");
    let regular_list = dir.path().join("build.list");
    write_file(&map, "G g\n");
    write_file(
        &result,
        r#"{"schema_version":1,"kind":"result","target":"cpu","exit_code":0,"lines":[]}"#,
    );
    write_file(&existing_ok, "A/1.0\t2\n");
    write_file(&regular_list, "G-1.0.eb\n");

    for bad_kind in ["fifo", "dir"] {
        let plain = dir.path().join(format!("bad-{bad_kind}.list"));
        let yaml = dir.path().join(format!("bad-{bad_kind}.yaml"));
        let existing = dir.path().join(format!("bad-{bad_kind}.tsv"));
        let metrics = dir.path().join(format!("bad-{bad_kind}.prom"));
        match bad_kind {
            "fifo" => {
                make_fifo(&plain);
                make_fifo(&yaml);
                make_fifo(&existing);
                make_fifo(&metrics);
            }
            "dir" => {
                std::fs::create_dir(&plain).unwrap();
                std::fs::create_dir(&yaml).unwrap();
                std::fs::create_dir(&existing).unwrap();
                std::fs::create_dir(&metrics).unwrap();
            }
            _ => unreachable!(),
        }

        // eb-test-map --list (plain path)
        let output = wait_output_with_timeout(
            Command::new(env!("CARGO_BIN_EXE_eb-test-map"))
                .args(["--map"])
                .arg(&map)
                .args(["--list"])
                .arg(&plain)
                .stdout(Stdio::piped())
                .stderr(Stdio::piped())
                .spawn()
                .unwrap(),
            &format!("eb-test-map plain {bad_kind}"),
        );
        assert_eq!(output.status.code(), Some(2));
        assert_eq!(output.stdout, b"");
        assert!(
            String::from_utf8_lossy(&output.stderr).contains("cannot read"),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );

        // eb-test-map --list (easystack path)
        let output = wait_output_with_timeout(
            Command::new(env!("CARGO_BIN_EXE_eb-test-map"))
                .args(["--map"])
                .arg(&map)
                .args(["--list"])
                .arg(&yaml)
                .stdout(Stdio::piped())
                .stderr(Stdio::piped())
                .spawn()
                .unwrap(),
            &format!("eb-test-map yaml {bad_kind}"),
        );
        assert_eq!(output.status.code(), Some(2));
        assert_eq!(output.stdout, b"");
        assert!(
            String::from_utf8_lossy(&output.stderr).contains("cannot read"),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );

        // eb-retry-list --list (plain path)
        let output = wait_output_with_timeout(
            Command::new(env!("CARGO_BIN_EXE_eb-retry-list"))
                .args(["--result"])
                .arg(&result)
                .args(["--list"])
                .arg(&plain)
                .stdout(Stdio::piped())
                .stderr(Stdio::piped())
                .spawn()
                .unwrap(),
            &format!("eb-retry-list plain {bad_kind}"),
        );
        assert_eq!(output.status.code(), Some(2));
        assert_eq!(output.stdout, b"");
        assert!(
            String::from_utf8_lossy(&output.stderr).contains("cannot read list"),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );

        // eb-retry-list --list (easystack path)
        let output = wait_output_with_timeout(
            Command::new(env!("CARGO_BIN_EXE_eb-retry-list"))
                .args(["--result"])
                .arg(&result)
                .args(["--list"])
                .arg(&yaml)
                .stdout(Stdio::piped())
                .stderr(Stdio::piped())
                .spawn()
                .unwrap(),
            &format!("eb-retry-list yaml {bad_kind}"),
        );
        assert_eq!(output.status.code(), Some(2));
        assert_eq!(output.stdout, b"");
        assert!(
            String::from_utf8_lossy(&output.stderr).contains("cannot read list"),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );

        // eb-shard-weights --existing
        let output = wait_output_with_timeout(
            Command::new(env!("CARGO_BIN_EXE_eb-shard-weights"))
                .args(["--result"])
                .arg(&result)
                .args(["--existing"])
                .arg(&existing)
                .stdout(Stdio::piped())
                .stderr(Stdio::piped())
                .spawn()
                .unwrap(),
            &format!("eb-shard-weights {bad_kind}"),
        );
        assert_eq!(output.status.code(), Some(2));
        assert_eq!(output.stdout, b"");
        assert!(
            String::from_utf8_lossy(&output.stderr).contains("cannot read"),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );

        // eb-cvmfs --metrics-file
        let output = wait_output_with_timeout(
            Command::new(env!("CARGO_BIN_EXE_eb-cvmfs"))
                .args(["--dry-run", "--metrics-file"])
                .arg(&metrics)
                .arg("repo.example.org")
                .arg("--")
                .arg("true")
                .stdout(Stdio::piped())
                .stderr(Stdio::piped())
                .spawn()
                .unwrap(),
            &format!("eb-cvmfs {bad_kind}"),
        );
        // Directory metrics path cannot be replaced atomically and fails.
        // FIFO metrics path may be replaced by the atomic writer, but it
        // must never block while reading previous metrics content.
        if bad_kind == "dir" {
            assert_eq!(output.status.code(), Some(1));
            assert!(
                String::from_utf8_lossy(&output.stderr).contains("cannot write metrics"),
                "{}",
                String::from_utf8_lossy(&output.stderr)
            );
        } else {
            assert!(output.status.code().is_some(), "{output:?}");
        }
    }
}
