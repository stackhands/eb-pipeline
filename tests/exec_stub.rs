//! `write_exec` must not leave a writable descriptor in this process.
//!
//! A forked child inherits every descriptor open at the fork and keeps it
//! until it execs. If one of them is the writable end of a stub, any process
//! that execs the stub in that window fails with ETXTBSY. These tests fork
//! without any lock while stubs are being installed, which is what the
//! integration suites do when a test reaches a plain `.output()`.

#![allow(dead_code)]

#[path = "common/exec_stub.rs"]
mod exec_stub;

use exec_stub::write_exec;
use std::os::unix::fs::PermissionsExt;
use std::os::unix::process::CommandExt;
use std::process::Command;
use std::sync::atomic::{AtomicBool, Ordering};

const WRITERS: usize = 4;
const FORKERS: usize = 4;
const ROUNDS: usize = 60;
const HOLD_SPINS: u32 = 400_000;

fn hold_descriptors_across_fork() -> Command {
    let mut command = Command::new("/bin/true");
    // SAFETY: the closure only spins on the stack, so it is async-signal-safe.
    unsafe {
        command.pre_exec(|| {
            for _ in 0..HOLD_SPINS {
                std::hint::spin_loop();
            }
            Ok(())
        });
    }
    command
}

#[test]
fn a_stub_runs_at_once_while_other_threads_fork_without_a_lock() {
    let dir = tempfile::tempdir().unwrap();
    let stop = AtomicBool::new(false);
    let failures: Vec<String> = std::thread::scope(|scope| {
        for _ in 0..FORKERS {
            scope.spawn(|| {
                while !stop.load(Ordering::Acquire) {
                    hold_descriptors_across_fork().status().unwrap();
                }
            });
        }
        let writers: Vec<_> = (0..WRITERS)
            .map(|writer| {
                let dir = dir.path();
                scope.spawn(move || {
                    let stub = dir.join(format!("stub-{writer}"));
                    let mut failures = Vec::new();
                    for round in 0..ROUNDS {
                        write_exec(&stub, &format!("#!/bin/sh\nexit {}\n", round % 7));
                        match Command::new(&stub).status() {
                            Ok(status) if status.code() == Some((round % 7) as i32) => {}
                            Ok(status) => failures.push(format!("round {round}: {status}")),
                            Err(error) => failures.push(format!("round {round}: {error}")),
                        }
                    }
                    failures
                })
            })
            .collect();
        let failures = writers
            .into_iter()
            .flat_map(|writer| writer.join().unwrap())
            .collect();
        stop.store(true, Ordering::Release);
        failures
    });
    assert!(failures.is_empty(), "{failures:#?}");
}

#[test]
fn a_stub_keeps_its_exact_bytes_across_chunks() {
    let dir = tempfile::tempdir().unwrap();
    let stub = dir.path().join("stub");
    let mut text = String::from("#!/bin/sh\n");
    while text.len() < 100_000 {
        text.push_str("printf '%s\\n' \"-n %s \\\\ $x `y` é€😀\"  \n\n");
    }
    text.push_str("exit 0");
    write_exec(&stub, &text);
    assert_eq!(std::fs::read_to_string(&stub).unwrap(), text);
    assert_eq!(
        std::fs::metadata(&stub).unwrap().permissions().mode() & 0o777,
        0o755
    );
}

#[test]
fn a_later_write_replaces_the_stub_and_leaves_no_temporary_file() {
    let dir = tempfile::tempdir().unwrap();
    let stub = dir.path().join("stub");
    write_exec(&stub, "#!/bin/sh\nexit 3\n");
    write_exec(&stub, "#!/bin/sh\nexit 4\n");
    assert_eq!(Command::new(&stub).status().unwrap().code(), Some(4));
    let names: Vec<_> = std::fs::read_dir(dir.path())
        .unwrap()
        .map(|entry| entry.unwrap().file_name())
        .collect();
    assert_eq!(names, ["stub"]);
}

#[test]
fn an_empty_stub_is_written_empty() {
    let dir = tempfile::tempdir().unwrap();
    let stub = dir.path().join("empty");
    write_exec(&stub, "");
    assert_eq!(std::fs::metadata(&stub).unwrap().len(), 0);
}

#[test]
#[should_panic(expected = "NUL")]
fn a_nul_byte_is_refused_rather_than_truncated() {
    let dir = tempfile::tempdir().unwrap();
    write_exec(&dir.path().join("stub"), "#!/bin/sh\0exit 0\n");
}
