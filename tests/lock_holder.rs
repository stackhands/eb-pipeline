//! A lock held by `LockHolder` is free again once `release` returns, even if
//! another thread forked while it was held.
//!
//! A forked child keeps every descriptor open at the fork until it execs, and
//! a `flock(2)` lock lives as long as any copy of its descriptor. The tests
//! stand in for that fork with a child that is held between fork and exec.

mod common;

use common::LockHolder;
use std::fs::File;
use std::os::fd::RawFd;
use std::os::unix::process::CommandExt;
use std::path::Path;
use std::process::Command;
use std::sync::{Mutex, MutexGuard};
use std::thread::JoinHandle;

static ONE_AT_A_TIME: Mutex<()> = Mutex::new(());

/// The tests fork and hold descriptors on purpose, so they must not run beside
/// each other: one test's fork would copy another test's descriptor.
fn alone() -> MutexGuard<'static, ()> {
    ONE_AT_A_TIME.lock().unwrap_or_else(|e| e.into_inner())
}

fn pipe() -> [RawFd; 2] {
    let mut fds = [0; 2];
    // SAFETY: `fds` has room for the two descriptors the call writes.
    assert_eq!(unsafe { libc::pipe2(fds.as_mut_ptr(), libc::O_CLOEXEC) }, 0);
    fds
}

fn read_byte(fd: RawFd) {
    let mut byte = 0u8;
    // SAFETY: the buffer is one live byte.
    assert_eq!(unsafe { libc::read(fd, (&raw mut byte).cast(), 1) }, 1);
}

fn write_byte(fd: RawFd) {
    let byte = 1u8;
    // SAFETY: the buffer is one live byte.
    assert_eq!(unsafe { libc::write(fd, (&raw const byte).cast(), 1) }, 1);
}

/// Another thread's fork, stopped between fork and exec until released.
struct HeldFork {
    go: RawFd,
    spawn: JoinHandle<()>,
}

impl HeldFork {
    /// Returns once the child exists and holds a copy of every open descriptor.
    fn start() -> Self {
        let [forked_read, forked_write] = pipe();
        let [go_read, go_write] = pipe();
        let spawn = std::thread::spawn(move || {
            let mut command = Command::new("/bin/true");
            // SAFETY: the closure only calls read and write.
            unsafe {
                command.pre_exec(move || {
                    let byte = 1u8;
                    libc::write(forked_write, (&raw const byte).cast(), 1);
                    let mut go = 0u8;
                    libc::read(go_read, (&raw mut go).cast(), 1);
                    Ok(())
                });
            }
            assert!(command.status().unwrap().success());
        });
        read_byte(forked_read);
        Self {
            go: go_write,
            spawn,
        }
    }

    fn finish(self) {
        write_byte(self.go);
        self.spawn.join().unwrap();
    }
}

fn is_free(path: &Path) -> bool {
    File::open(path).unwrap().try_lock().is_ok()
}

#[test]
fn an_in_process_descriptor_copied_by_a_fork_keeps_the_lock_after_release() {
    let _alone = alone();
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("lock");
    let held = File::create(&path).unwrap();
    held.lock().unwrap();
    let fork = HeldFork::start();
    drop(held);
    assert!(!is_free(&path));
    fork.finish();
    assert!(is_free(&path));
}

#[test]
fn a_released_lock_is_free_although_a_fork_ran_while_it_was_held() {
    let _alone = alone();
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("lock");
    let holder = LockHolder::exclusive(&path);
    let fork = HeldFork::start();
    holder.release();
    assert!(is_free(&path));
    fork.finish();
}

#[test]
fn a_shared_lock_keeps_an_exclusive_one_out_until_it_is_released() {
    let _alone = alone();
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("lock");
    let holder = LockHolder::shared(&path);
    assert!(File::open(&path).unwrap().try_lock_shared().is_ok());
    assert!(!is_free(&path));
    holder.release();
    assert!(is_free(&path));
}

#[test]
fn an_exclusive_lock_keeps_a_shared_one_out_until_it_is_dropped() {
    let _alone = alone();
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("lock");
    let holder = LockHolder::exclusive(&path);
    assert!(File::open(&path).unwrap().try_lock_shared().is_err());
    drop(holder);
    assert!(File::open(&path).unwrap().try_lock_shared().is_ok());
}
