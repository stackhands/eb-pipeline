//! Non-regular metrics files must not block `eb-cvmfs`.

use std::ffi::CString;
use std::os::unix::ffi::OsStrExt;
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

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

    let mut child = bin()
        .args(["--dry-run", "--metrics-file"])
        .arg(&metrics)
        .arg("repo.example.org")
        .arg("--")
        .arg("true")
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();

    let deadline = Instant::now() + Duration::from_secs(5);
    loop {
        if child.try_wait().unwrap().is_some() {
            break;
        }
        if Instant::now() >= deadline {
            let _ = child.kill();
            let _ = child.wait();
            panic!("eb-cvmfs blocked while reading a FIFO metrics file");
        }
        std::thread::sleep(Duration::from_millis(10));
    }

    let output = child.wait_with_output().unwrap();
    assert_eq!(output.status.code(), Some(0), "{output:?}");
    assert!(
        String::from_utf8_lossy(&output.stdout).contains("cvmfs_server list"),
        "{}",
        String::from_utf8_lossy(&output.stdout)
    );
}
