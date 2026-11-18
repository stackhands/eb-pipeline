//! Standalone command metadata output follows the CLI broken-pipe contract.

use std::os::fd::FromRawFd;
use std::process::{Command, Stdio};

fn closed_stdout() -> Stdio {
    let mut fds = [0; 2];
    // SAFETY: both descriptors are checked, and ownership of the write
    // descriptor moves into `Stdio`.
    unsafe {
        assert_eq!(libc::pipe(fds.as_mut_ptr()), 0);
        assert_eq!(libc::close(fds[0]), 0);
        Stdio::from_raw_fd(fds[1])
    }
}

#[test]
fn standalone_help_and_version_tolerate_a_closed_consumer() {
    for binary in [
        env!("CARGO_BIN_EXE_eb-cvmfs"),
        env!("CARGO_BIN_EXE_eb-junit"),
        env!("CARGO_BIN_EXE_eb-retry-list"),
        env!("CARGO_BIN_EXE_eb-shard-weights"),
        env!("CARGO_BIN_EXE_eb-test-map"),
    ] {
        for option in ["--help", "--version"] {
            let output = Command::new(binary)
                .arg(option)
                .stdout(closed_stdout())
                .stderr(Stdio::piped())
                .output()
                .unwrap();
            assert_eq!(
                output.status.code(),
                Some(0),
                "{binary} {option}: {}",
                String::from_utf8_lossy(&output.stderr)
            );
        }
    }
}
