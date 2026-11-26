//! Non-regular metrics files must not block `eb-cvmfs`.

use std::ffi::CString;
use std::os::unix::ffi::OsStrExt;
use std::process::Command;

fn bin() -> Command {
    Command::new(env!("CARGO_BIN_EXE_eb-cvmfs"))
}

fn make_fifo(path: &std::path::Path) {
    let raw = CString::new(path.as_os_str().as_bytes()).unwrap();
    assert_eq!(unsafe { libc::mkfifo(raw.as_ptr(), 0o600) }, 0);
}

#[test]
fn dry_run_metrics_update_does_not_block_on_fifo() {
    let dir = tempfile::tempdir().unwrap();
    let metrics = dir.path().join("metrics.prom");
    make_fifo(&metrics);

    let output = bin()
        .args(["--dry-run", "--metrics-file"])
        .arg(&metrics)
        .arg("repo.example.org")
        .arg("--")
        .arg("true")
        .output()
        .unwrap();
    assert_eq!(output.status.code(), Some(0), "{output:?}");
    assert!(
        String::from_utf8_lossy(&output.stdout).contains("cvmfs_server list"),
        "{}",
        String::from_utf8_lossy(&output.stdout)
    );
}
