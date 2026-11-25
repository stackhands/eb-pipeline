//! Non-UTF-8 path-like arguments should be accepted as byte paths and fail
//! normally, not panic.
#![allow(clippy::disallowed_methods)]

use std::ffi::OsString;
use std::os::unix::ffi::OsStringExt;
use std::path::Path;
use std::process::Command;

fn non_utf8_path() -> OsString {
    OsString::from_vec(vec![
        b'b', b'a', b'd', b'-', 0x80, b'.', b'j', b's', b'o', b'n',
    ])
}

fn assert_non_utf8_path_is_handled(output: std::process::Output, command: &str) {
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert_eq!(output.status.code(), Some(2), "{command}: {stderr}");
    assert!(
        stderr.contains(&format!("{command}:")),
        "{command}: {stderr}"
    );
    assert!(!stderr.contains("panicked at"), "{command}: {stderr}");
    assert_eq!(output.stdout, b"", "{command}: {stderr}");
}

#[test]
fn non_utf8_path_arguments_are_processed_without_panic() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(dir.path().join("list.txt"), "Foo-1.0.eb\n").unwrap();
    let path = non_utf8_path();

    let output = Command::new(env!("CARGO_BIN_EXE_eb-junit"))
        .arg("--result")
        .arg(path.clone())
        .output()
        .unwrap();
    assert_non_utf8_path_is_handled(output, "eb-junit");

    let output = Command::new(env!("CARGO_BIN_EXE_eb-retry-list"))
        .arg("--result")
        .arg(path.clone())
        .arg("--list")
        .arg(dir.path().join("list.txt"))
        .output()
        .unwrap();
    assert_non_utf8_path_is_handled(output, "eb-retry-list");

    let output = Command::new(env!("CARGO_BIN_EXE_eb-shard-weights"))
        .arg("--result")
        .arg(path.clone())
        .output()
        .unwrap();
    assert_non_utf8_path_is_handled(output, "eb-shard-weights");

    let output = Command::new(env!("CARGO_BIN_EXE_eb-test-map"))
        .arg("--map")
        .arg(path.clone())
        .arg("--list")
        .arg(dir.path().join("list.txt"))
        .output()
        .unwrap();
    assert_non_utf8_path_is_handled(output, "eb-test-map");

    let output = Command::new(env!("CARGO_BIN_EXE_eb-cvmfs"))
        .arg("--archive-manifest")
        .arg(path)
        .args(["repo.example", "--"])
        .arg(Path::new("/bin/true"))
        .output()
        .unwrap();
    assert_non_utf8_path_is_handled(output, "eb-cvmfs");
}
