//! `output_within` kills and reaps its child before it fails on a system call,
//! a signal that interrupts its wait does not end the wait, and any limit up
//! to `Duration::MAX` is accepted.
#![allow(clippy::disallowed_methods)]

mod common;

use common::output_within;
use std::io::Read;
use std::os::fd::{AsRawFd, FromRawFd, OwnedFd};
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

const LIMIT: Duration = Duration::from_secs(20);

fn pipe() -> (OwnedFd, OwnedFd) {
    let mut fds = [0; 2];
    // SAFETY: `fds` has room for the two descriptors the call writes, and
    // each is owned by one `OwnedFd` from here on.
    unsafe {
        assert_eq!(libc::pipe2(fds.as_mut_ptr(), libc::O_CLOEXEC), 0);
        (OwnedFd::from_raw_fd(fds[0]), OwnedFd::from_raw_fd(fds[1]))
    }
}

/// Make `pidfd_open` fail with ENOSYS in this thread and the children it
/// starts, as it does in a sandbox that does not know the call.
fn refuse_pidfd_open() {
    let stmt = |code: u32, k: u32| libc::sock_filter {
        code: u16::try_from(code).unwrap(),
        jt: 0,
        jf: 0,
        k,
    };
    let mut filter = [
        stmt(libc::BPF_LD | libc::BPF_W | libc::BPF_ABS, 0),
        libc::sock_filter {
            jf: 1,
            ..stmt(
                libc::BPF_JMP | libc::BPF_JEQ | libc::BPF_K,
                u32::try_from(libc::SYS_pidfd_open).unwrap(),
            )
        },
        stmt(
            libc::BPF_RET | libc::BPF_K,
            libc::SECCOMP_RET_ERRNO | u32::try_from(libc::ENOSYS).unwrap(),
        ),
        stmt(libc::BPF_RET | libc::BPF_K, libc::SECCOMP_RET_ALLOW),
    ];
    let prog = libc::sock_fprog {
        len: u16::try_from(filter.len()).unwrap(),
        filter: filter.as_mut_ptr(),
    };
    // SAFETY: `prog` points at `filter`, which outlives the call. Both
    // settings apply to this thread only.
    unsafe {
        assert_eq!(libc::prctl(libc::PR_SET_NO_NEW_PRIVS, 1, 0, 0, 0), 0);
        assert_eq!(
            libc::syscall(
                libc::SYS_seccomp,
                libc::SECCOMP_SET_MODE_FILTER,
                0,
                &raw const prog
            ),
            0,
            "seccomp: {}",
            std::io::Error::last_os_error()
        );
    }
}

/// Read `fd` until its end or `limit`, whichever comes first, and say
/// whether the end came.
fn read_to_end_within(fd: OwnedFd, limit: Duration, text: &mut String) -> bool {
    let deadline = Instant::now() + limit;
    let mut pollfd = libc::pollfd {
        fd: fd.as_raw_fd(),
        events: libc::POLLIN,
        revents: 0,
    };
    let mut file = std::fs::File::from(fd);
    let mut buf = [0u8; 64];
    loop {
        let left = deadline.saturating_duration_since(Instant::now());
        let timeout = i32::try_from(left.as_millis()).unwrap();
        // SAFETY: `pollfd` is a valid one-element list and `file` keeps the
        // descriptor open.
        match unsafe { libc::poll(&mut pollfd, 1, timeout) } {
            0 => return false,
            n if n < 0 => panic!("poll: {}", std::io::Error::last_os_error()),
            _ => {}
        }
        match file.read(&mut buf).unwrap() {
            0 => return true,
            n => text.push_str(std::str::from_utf8(&buf[..n]).unwrap()),
        }
    }
}

#[test]
fn a_failed_pidfd_open_kills_and_reaps_the_child_and_names_the_error() {
    let (read, write) = pipe();
    let mut cmd = Command::new("sh");
    cmd.args(["-c", "echo $$; exec sleep 30"])
        .stdin(Stdio::null())
        .stdout(Stdio::from(write))
        .stderr(Stdio::null());
    let failed = std::thread::spawn(move || {
        refuse_pidfd_open();
        output_within(&mut cmd, LIMIT)
    })
    .join()
    .expect_err("output_within returned although pidfd_open failed");
    let mut pid = String::new();
    if !read_to_end_within(read, Duration::from_secs(5), &mut pid) {
        let pid: libc::pid_t = pid.trim().parse().unwrap();
        // SAFETY: `pid` is the child that output_within left behind; it is
        // still unreaped, so the pid cannot have been reused.
        unsafe {
            libc::kill(pid, libc::SIGKILL);
            libc::waitpid(pid, std::ptr::null_mut(), 0);
        }
        panic!("output_within left its child {pid} running");
    }
    let message = failed.downcast_ref::<String>().unwrap();
    let errno = std::io::Error::from_raw_os_error(libc::ENOSYS).to_string();
    assert!(
        message.contains("pidfd_open") && message.contains(&errno),
        "{message}"
    );
}

extern "C" fn ignore(_: libc::c_int) {}

/// A no-op `SIGUSR1` handler for this process, and the thread that sends the
/// signal. Dropping it joins that thread and then puts back the previous
/// action, so no signal can arrive after the restore. This is the only test
/// in this binary that changes a signal action.
struct Usr1Handler {
    old: libc::sigaction,
    signaller: Option<std::thread::JoinHandle<libc::c_int>>,
}

impl Usr1Handler {
    fn install() -> Self {
        // SAFETY: a zeroed `sigaction` is a valid value of that plain C
        // struct, and `ignore` is async-signal-safe. No SA_RESTART, so the
        // signal makes `poll` fail with EINTR.
        unsafe {
            let mut action: libc::sigaction = std::mem::zeroed();
            action.sa_sigaction = ignore as extern "C" fn(libc::c_int) as libc::sighandler_t;
            let mut old: libc::sigaction = std::mem::zeroed();
            assert_eq!(
                libc::sigaction(libc::SIGUSR1, &raw const action, &raw mut old),
                0
            );
            Self {
                old,
                signaller: None,
            }
        }
    }
}

impl Drop for Usr1Handler {
    fn drop(&mut self) {
        if let Some(signaller) = self.signaller.take() {
            let _ = signaller.join();
        }
        // SAFETY: `old` is the action that `install` read back.
        unsafe { libc::sigaction(libc::SIGUSR1, &raw const self.old, std::ptr::null_mut()) };
    }
}

#[test]
fn a_signal_during_the_wait_does_not_end_it() {
    let mut handler = Usr1Handler::install();
    // SAFETY: plain call with no arguments.
    let waiter = unsafe { libc::pthread_self() };
    handler.signaller = Some(std::thread::spawn(move || {
        std::thread::sleep(Duration::from_millis(500));
        // SAFETY: `waiter` is the test thread, which is alive until the
        // handler is dropped, and that joins this thread first.
        unsafe { libc::pthread_kill(waiter, libc::SIGUSR1) }
    }));
    let output = output_within(
        Command::new("sh")
            .args(["-c", "sleep 2; echo done"])
            .stdin(Stdio::null())
            .stdout(Stdio::piped()),
        LIMIT,
    );
    let sent = handler.signaller.take().unwrap().join().unwrap();
    drop(handler);
    assert_eq!(sent, 0, "pthread_kill");
    assert!(output.status.success(), "{output:?}");
    assert_eq!(output.stdout, b"done\n");
}

#[test]
fn the_longest_limit_waits_for_a_child_that_exits_at_once() {
    let output = output_within(
        Command::new("sh")
            .args(["-c", "echo done"])
            .stdin(Stdio::null())
            .stdout(Stdio::piped()),
        Duration::MAX,
    );
    assert!(output.status.success(), "{output:?}");
    assert_eq!(output.stdout, b"done\n");
}
