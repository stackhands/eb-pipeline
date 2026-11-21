//! Small helpers.

use crate::exit::Fail;
use std::ffi::{OsStr, OsString};
use std::fs::OpenOptions;
use std::io::{Read, Write};
use std::os::unix::ffi::{OsStrExt, OsStringExt};
use std::os::unix::fs::{MetadataExt, OpenOptionsExt, PermissionsExt};
use std::path::{Path, PathBuf};
use std::sync::OnceLock;

/// Open a regular input file without waiting on a special file.
pub fn open_regular_file(path: &Path) -> std::io::Result<std::fs::File> {
    let file = OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_NONBLOCK)
        .open(path)?;
    let meta = file.metadata()?;
    if meta.is_dir() {
        return Err(std::io::Error::from_raw_os_error(libc::EISDIR));
    }
    if !meta.is_file() {
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidInput,
            "not a regular file",
        ));
    }
    Ok(file)
}

/// Read a regular input file as UTF-8 without waiting on a special file.
pub fn read_text_file(path: &Path) -> std::io::Result<String> {
    let mut file = open_regular_file(path)?;
    let mut text = String::new();
    file.read_to_string(&mut text)?;
    Ok(text)
}

/// Write all bytes to standard output, treating a closed pipeline consumer as
/// successful completion.
pub fn write_stdout(bytes: &[u8]) -> std::io::Result<()> {
    match std::io::stdout().lock().write_all(bytes) {
        Err(e) if e.kind() == std::io::ErrorKind::BrokenPipe => Ok(()),
        result => result,
    }
}

/// Write one line to standard error on a best-effort basis. A closed or full
/// stderr is ignored, so a diagnostic never turns into a panic or changes the
/// exit status the caller already chose.
pub fn write_stderr_line(message: &str) {
    let mut stderr = std::io::stderr().lock();
    let _ = writeln!(stderr, "{message}");
}

/// Write bytes to standard output or format a user-facing error message.
pub fn write_stdout_checked(bytes: &[u8]) -> Result<(), String> {
    write_stdout(bytes).map_err(|e| format!("cannot write stdout: {e}"))
}

/// A non-empty Unicode value from the process environment.
pub fn env_value(name: &str) -> Option<String> {
    std::env::var(name).ok().filter(|value| !value.is_empty())
}

/// Why a duration in seconds is invalid.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DurationError {
    /// Not finite or negative.
    Invalid,
    /// Larger than a consumer's explicit upper bound.
    TooLarge,
}

/// Validate a duration value in seconds, with an optional exclusive upper
/// bound.
pub fn checked_duration_seconds(
    seconds: f64,
    max_seconds: Option<f64>,
) -> Result<f64, DurationError> {
    if !seconds.is_finite() || seconds < 0.0 {
        return Err(DurationError::Invalid);
    }
    if max_seconds.is_some_and(|max| seconds >= max) {
        return Err(DurationError::TooLarge);
    }
    Ok(seconds)
}

/// Process arguments after the executable path, preserving byte paths.
pub fn os_args() -> Vec<OsString> {
    std::env::args_os().skip(1).collect()
}

/// Convert one argument that must be text to UTF-8.
pub fn os_to_string(arg: OsString) -> Result<String, String> {
    arg.into_string()
        .map_err(|arg| format!("argument is not valid UTF-8: `{}`", arg.to_string_lossy()))
}

/// Split `--flag=value` even when `value` is not UTF-8.
pub fn split_os_flag_value(arg: &OsStr) -> Option<(String, OsString)> {
    let bytes = arg.as_bytes();
    let at = bytes.iter().position(|byte| *byte == b'=')?;
    if at < 2 || !bytes.starts_with(b"--") {
        return None;
    }
    let flag = std::str::from_utf8(&bytes[..at]).ok()?.to_string();
    Some((flag, OsString::from_vec(bytes[at + 1..].to_vec())))
}

/// The next non-empty command-line value as bytes, without consuming another
/// long option as that value.
pub fn separate_os_value(
    words: &mut impl Iterator<Item = OsString>,
    missing: &str,
) -> Result<OsString, String> {
    words
        .next()
        .filter(|value| {
            !value.as_os_str().is_empty() && !value.as_os_str().as_bytes().starts_with(b"--")
        })
        .ok_or_else(|| missing.to_string())
}

/// The next non-empty command-line value, without consuming another long
/// option as that value.
pub fn separate_value(
    words: &mut impl Iterator<Item = String>,
    missing: &str,
) -> Result<String, String> {
    words
        .next()
        .filter(|value| !value.is_empty() && !value.starts_with("--"))
        .ok_or_else(|| missing.to_string())
}

/// Whether two paths name the same file, including through a symbolic or
/// hard link. A path that does not exist is resolved through its parent.
pub fn paths_alias(path_a: &Path, path_b: &Path) -> bool {
    if resolved_path(path_a) == resolved_path(path_b) {
        return true;
    }
    match (std::fs::metadata(path_a), std::fs::metadata(path_b)) {
        (Ok(a), Ok(b)) => a.dev() == b.dev() && a.ino() == b.ino(),
        _ => false,
    }
}

/// Directory holding `path`, treating a bare file name as living in `.`.
fn parent_dir(path: &Path) -> &Path {
    match path.parent() {
        Some(parent) if !parent.as_os_str().is_empty() => parent,
        _ => Path::new("."),
    }
}

/// Resolve a path as far as the filesystem permits.
pub fn resolved_path(path: &Path) -> PathBuf {
    if let Ok(path) = std::fs::canonicalize(path) {
        return path;
    }
    let parent = parent_dir(path);
    match (
        std::fs::canonicalize(parent),
        path.file_name().map(ToOwned::to_owned),
    ) {
        (Ok(parent), Some(name)) => parent.join(name),
        _ => std::path::absolute(path).unwrap_or_else(|_| path.to_path_buf()),
    }
}

/// Refuse an output path that aliases any input path.
///
/// `-` means standard output and is not a file destination.
pub fn refuse_output_alias(
    output_option: &str,
    output: Option<&Path>,
    inputs: &[(&str, Option<&Path>)],
) -> Result<(), String> {
    let Some(output) = output.filter(|path| *path != Path::new("-")) else {
        return Ok(());
    };
    for (input_option, input) in inputs {
        let Some(input) = input else {
            continue;
        };
        if paths_alias(input, output) {
            return Err(format!(
                "{input_option} and {output_option} must name different files, both resolve to {}",
                resolved_path(input).display()
            ));
        }
    }
    Ok(())
}

/// Atomically replace a non-symlink `path` through a fully written temporary
/// sibling.
///
/// An existing regular file's permission bits are applied to the temporary
/// file before the rename. Ownership follows the account creating the
/// temporary file.
pub fn write_atomic(path: &Path, bytes: &[u8]) -> std::io::Result<()> {
    publish_atomic_with(
        path,
        |file, _| write_atomic_bytes(file, bytes),
        std::convert::identity,
    )
}

/// Atomically copy regular file `source` to non-symlink `destination` through a
/// temporary sibling, running `verify` before syncing and renaming into place.
pub fn copy_atomic_checked<E>(
    source: &Path,
    destination: &Path,
    verify: impl FnOnce(&Path) -> Result<(), E>,
    map_io: impl Fn(std::io::Error) -> E,
) -> Result<(), E> {
    publish_atomic_with(
        destination,
        |file, temporary| {
            let mut input = open_regular_file(source).map_err(&map_io)?;
            std::io::copy(&mut input, file).map_err(&map_io)?;
            file.flush().map_err(&map_io)?;
            verify(temporary)
        },
        &map_io,
    )
}

fn publish_atomic_with<T, E>(
    path: &Path,
    populate: impl FnOnce(&mut std::fs::File, &Path) -> Result<T, E>,
    map_io: impl Fn(std::io::Error) -> E,
) -> Result<T, E> {
    if path.is_symlink() {
        return Err(map_io(std::io::Error::new(
            std::io::ErrorKind::InvalidInput,
            "output path is a symbolic link",
        )));
    }
    let destination = path.to_path_buf();
    let parent = parent_dir(&destination);
    let existing_mode = match std::fs::metadata(&destination) {
        Ok(metadata) if metadata.is_file() => Some(metadata.permissions().mode() & 0o7777),
        Ok(_) => None,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => None,
        Err(error) => return Err(map_io(error)),
    };
    let name = atomic_output_name(&destination);
    let local_identity = local_writer_identity();
    remove_stale_atomic_temporary_files(parent, name, &local_identity);
    for attempt in 0..1000u32 {
        let temporary = parent.join(atomic_temp_file_name(
            name,
            &local_identity,
            std::process::id() as libc::pid_t,
            attempt,
        ));
        let mut file = match OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&temporary)
        {
            Ok(file) => file,
            Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => continue,
            Err(error) => return Err(map_io(error)),
        };
        #[cfg(test)]
        maybe_kill_atomic_writer("after-temp-created");
        if let Some(mode) = existing_mode {
            if let Err(error) =
                std::fs::set_permissions(&temporary, std::fs::Permissions::from_mode(mode))
            {
                drop(file);
                let _ = std::fs::remove_file(&temporary);
                return Err(map_io(error));
            }
        }
        let outcome = populate(&mut file, &temporary).and_then(|value| {
            sync_temp_file(&file).map_err(&map_io)?;
            Ok(value)
        });
        drop(file);
        let value = match outcome {
            Ok(value) => value,
            Err(error) => {
                let _ = std::fs::remove_file(&temporary);
                return Err(error);
            }
        };
        #[cfg(test)]
        maybe_kill_atomic_writer("before-rename");
        if let Err(error) = rename_atomic_temp(&temporary, &destination) {
            let _ = std::fs::remove_file(&temporary);
            return Err(map_io(error));
        }
        #[cfg(test)]
        maybe_kill_atomic_writer("after-rename-before-dir-sync");
        sync_parent_dir(parent).map_err(&map_io)?;
        return Ok(value);
    }
    Err(map_io(std::io::Error::new(
        std::io::ErrorKind::AlreadyExists,
        "cannot allocate a temporary output file",
    )))
}

#[cfg(not(test))]
fn write_atomic_bytes(file: &mut std::fs::File, bytes: &[u8]) -> std::io::Result<()> {
    file.write_all(bytes)
}

#[cfg(test)]
fn write_atomic_bytes(file: &mut std::fs::File, bytes: &[u8]) -> std::io::Result<()> {
    if kill_stage_matches("mid-write") && !bytes.is_empty() {
        let split_at = bytes.len().max(2) / 2;
        file.write_all(&bytes[..split_at])?;
        maybe_kill_atomic_writer("mid-write");
        file.write_all(&bytes[split_at..])
    } else {
        file.write_all(bytes)
    }
}

#[cfg(test)]
fn maybe_kill_atomic_writer(stage: &str) {
    if kill_stage_matches(stage) {
        // SAFETY: test hook only; intentionally terminates the child process.
        unsafe {
            libc::raise(libc::SIGKILL);
        }
    }
}

#[cfg(test)]
fn kill_stage_matches(stage: &str) -> bool {
    std::env::var_os("EB_PIPELINE_TEST_KILL_WRITE_ATOMIC").as_deref() == Some(stage.as_ref())
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct WriterIdentity {
    host: String,
    host_reap_safe: bool,
    machine_id: Option<String>,
    boot_id: Option<String>,
    pid_ns: Option<u64>,
    start_time: Option<u64>,
}

const HOST_UNKNOWN_TAG: &str = "host-unknown";
const MACHINE_UNKNOWN_TAG: &str = "machine-unknown";
const BOOT_UNKNOWN_TAG: &str = "boot-unknown";
const PID_NS_UNKNOWN_TAG: &str = "pidns-unknown";
const START_TIME_UNKNOWN_TAG: &str = "start-unknown";

#[cfg(test)]
mod writer_identity_test_hook {
    use super::WriterIdentity;
    use std::cell::RefCell;

    thread_local! {
        static OVERRIDE: RefCell<Option<WriterIdentity>> = const { RefCell::new(None) };
    }

    pub(super) fn get() -> Option<WriterIdentity> {
        OVERRIDE.with(|slot| slot.borrow().clone())
    }

    pub(super) fn install(identity: WriterIdentity) {
        OVERRIDE.with(|slot| *slot.borrow_mut() = Some(identity));
    }

    pub(super) fn clear() {
        OVERRIDE.with(|slot| *slot.borrow_mut() = None);
    }
}

fn local_writer_identity() -> WriterIdentity {
    #[cfg(test)]
    if let Some(identity) = writer_identity_test_hook::get() {
        return identity;
    }
    static IDENTITY: OnceLock<WriterIdentity> = OnceLock::new();
    IDENTITY.get_or_init(detect_writer_identity).clone()
}

fn detect_writer_identity() -> WriterIdentity {
    let raw_host = select_kernel_host(
        read_trimmed_non_empty("/proc/sys/kernel/hostname"),
        uname_nodename(),
    );
    let (host, host_reap_safe) = host_tag_and_reap_safety(raw_host.as_deref());
    let machine_id = read_trimmed_non_empty("/etc/machine-id").and_then(|id| machine_tag(&id));
    let boot_id =
        read_trimmed_non_empty("/proc/sys/kernel/random/boot_id").and_then(|boot| boot_tag(&boot));
    let pid_ns = current_pid_namespace();
    let start_time = process_start_time(std::process::id() as libc::pid_t);
    WriterIdentity {
        host,
        host_reap_safe,
        machine_id,
        boot_id,
        pid_ns,
        start_time,
    }
}

fn read_trimmed_non_empty(path: &str) -> Option<String> {
    let text = std::fs::read_to_string(path).ok()?;
    let trimmed = text.trim();
    (!trimmed.is_empty()).then(|| trimmed.to_string())
}

fn uname_nodename() -> Option<String> {
    let mut info = std::mem::MaybeUninit::<libc::utsname>::zeroed();
    // SAFETY: `info` is a valid buffer for `uname(2)`.
    if unsafe { libc::uname(info.as_mut_ptr()) } != 0 {
        return None;
    }
    // SAFETY: `uname(2)` initialized `info` on success.
    let info = unsafe { info.assume_init() };
    let bytes: Vec<u8> = info
        .nodename
        .iter()
        .take_while(|&&ch| ch != 0)
        .map(|&ch| ch as u8)
        .collect();
    let text = std::str::from_utf8(&bytes).ok()?.trim();
    (!text.is_empty()).then(|| text.to_string())
}

fn select_kernel_host(proc_host: Option<String>, uname_host: Option<String>) -> Option<String> {
    [proc_host, uname_host]
        .into_iter()
        .flatten()
        .map(|host| host.trim().to_string())
        .find(|host| !host.is_empty())
}

fn current_pid_namespace() -> Option<u64> {
    std::fs::metadata("/proc/self/ns/pid")
        .ok()
        .map(|meta| meta.ino())
        .filter(|&ino| ino > 0)
}

/// Parse the process state (field 3) and start time in clock ticks (field 22)
/// from `/proc/<pid>/stat`.
pub fn parse_proc_stat(stat: &str) -> Option<(char, u64)> {
    let after_comm = stat.rsplit_once(')')?.1;
    let mut fields = after_comm.split_ascii_whitespace();
    let state = fields.next()?;
    if state.len() != 1 {
        return None;
    }
    let state = state.chars().next()?;
    let start_time = fields.nth(18)?.parse::<u64>().ok()?;
    Some((state, start_time))
}

fn parse_proc_stat_start_time(stat: &str) -> Option<u64> {
    parse_proc_stat(stat).map(|(_, start_time)| start_time)
}

fn process_start_time(pid: libc::pid_t) -> Option<u64> {
    if pid <= 0 {
        return None;
    }
    let stat = std::fs::read_to_string(format!("/proc/{pid}/stat")).ok()?;
    parse_proc_stat_start_time(&stat)
}

fn validated_tag_component(text: &str) -> Option<String> {
    if !text.is_empty()
        && text
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_'))
    {
        Some(text.to_string())
    } else {
        None
    }
}

fn machine_tag(raw: &str) -> Option<String> {
    validated_tag_component(raw)
        .filter(|tag| !tag.eq_ignore_ascii_case("uninitialized") && tag != MACHINE_UNKNOWN_TAG)
}

fn boot_tag(raw: &str) -> Option<String> {
    validated_tag_component(raw).filter(|tag| tag != BOOT_UNKNOWN_TAG)
}

fn host_tag_and_reap_safety(raw_host: Option<&str>) -> (String, bool) {
    let Some(raw_host) = raw_host else {
        return (HOST_UNKNOWN_TAG.into(), false);
    };
    let Some(tag) = validated_tag_component(raw_host) else {
        return (HOST_UNKNOWN_TAG.into(), false);
    };
    if tag.eq_ignore_ascii_case("localhost") || tag == HOST_UNKNOWN_TAG {
        return (tag, false);
    }
    (tag, true)
}

fn atomic_output_name(destination: &Path) -> &str {
    destination
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or("output")
}

fn atomic_temp_file_name(
    name: &str,
    identity: &WriterIdentity,
    pid: libc::pid_t,
    attempt: u32,
) -> String {
    let machine = identity
        .machine_id
        .as_deref()
        .unwrap_or(MACHINE_UNKNOWN_TAG);
    let boot = identity.boot_id.as_deref().unwrap_or(BOOT_UNKNOWN_TAG);
    let pid_ns = identity
        .pid_ns
        .map(|ino| ino.to_string())
        .unwrap_or_else(|| PID_NS_UNKNOWN_TAG.into());
    let start_time = identity
        .start_time
        .map(|ticks| ticks.to_string())
        .unwrap_or_else(|| START_TIME_UNKNOWN_TAG.into());
    format!(
        ".{name}.{}.{}.{}.{}.{pid}.{}.{attempt}.tmp",
        identity.host, machine, boot, pid_ns, start_time
    )
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct AtomicTempFile {
    host: String,
    machine: String,
    boot: String,
    pid_ns: u64,
    pid: libc::pid_t,
    start_time: u64,
    attempt: u32,
}

fn parse_atomic_temporary_name(file_name: &str, output_name: &str) -> Option<AtomicTempFile> {
    let prefix = format!(".{output_name}.");
    let middle = file_name
        .strip_prefix(prefix.as_str())?
        .strip_suffix(".tmp")?;
    let mut fields = middle.split('.');
    let host = fields.next()?.to_string();
    let machine = fields.next()?.to_string();
    let boot = fields.next()?.to_string();
    let pid_ns = fields.next()?.parse::<u64>().ok().filter(|ns| *ns > 0)?;
    let pid = fields
        .next()?
        .parse::<libc::pid_t>()
        .ok()
        .filter(|pid| *pid > 0)?;
    let start_time = fields.next()?.parse::<u64>().ok()?;
    let attempt = fields.next()?.parse::<u32>().ok()?;
    if host.is_empty() || machine.is_empty() || boot.is_empty() || fields.next().is_some() {
        return None;
    }
    Some(AtomicTempFile {
        host,
        machine,
        boot,
        pid_ns,
        pid,
        start_time,
        attempt,
    })
}

#[cfg(test)]
fn atomic_temporary_siblings(path: &Path) -> Vec<(PathBuf, AtomicTempFile)> {
    let parent = parent_dir(path);
    let name = atomic_output_name(path);
    scan_atomic_temporary_siblings(parent, name)
}

fn scan_atomic_temporary_siblings(parent: &Path, name: &str) -> Vec<(PathBuf, AtomicTempFile)> {
    let Ok(entries) = std::fs::read_dir(parent) else {
        return Vec::new();
    };
    let mut found = Vec::new();
    for entry in entries.filter_map(Result::ok) {
        let Ok(file_type) = entry.file_type() else {
            continue;
        };
        if !file_type.is_file() {
            continue;
        }
        let file_name = entry.file_name();
        let Some(shown) = file_name.to_str() else {
            continue;
        };
        let Some(parsed) = parse_atomic_temporary_name(shown, name) else {
            continue;
        };
        found.push((entry.path(), parsed));
    }
    found.sort_by(|a, b| a.0.cmp(&b.0));
    found
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum ProcessState {
    Dead,
    AliveSameOrUnknown,
    Reused,
}

fn inspect_process_with(
    pid: libc::pid_t,
    expected_start_time: u64,
    read_stat: impl FnOnce(libc::pid_t) -> std::io::Result<String>,
    is_alive: impl FnOnce(libc::pid_t) -> bool,
) -> ProcessState {
    if pid <= 0 {
        return ProcessState::AliveSameOrUnknown;
    }
    match read_stat(pid) {
        Ok(stat) => match parse_proc_stat_start_time(&stat) {
            Some(actual_start_time) if actual_start_time != expected_start_time => {
                ProcessState::Reused
            }
            _ => ProcessState::AliveSameOrUnknown,
        },
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            if is_alive(pid) {
                ProcessState::AliveSameOrUnknown
            } else {
                ProcessState::Dead
            }
        }
        Err(_) => ProcessState::AliveSameOrUnknown,
    }
}

fn inspect_process(pid: libc::pid_t, expected_start_time: u64) -> ProcessState {
    inspect_process_with(
        pid,
        expected_start_time,
        |pid| std::fs::read_to_string(format!("/proc/{pid}/stat")),
        process_is_alive,
    )
}

fn should_reap_atomic_temp(local: &WriterIdentity, temp: &AtomicTempFile) -> bool {
    if !local.host_reap_safe {
        return false;
    }
    let Some(local_boot) = local.boot_id.as_deref() else {
        return false;
    };
    let Some(local_pid_ns) = local.pid_ns else {
        return false;
    };
    if local.start_time.is_none() {
        return false;
    }
    if temp.host != local.host
        || temp.host == HOST_UNKNOWN_TAG
        || temp.host.eq_ignore_ascii_case("localhost")
        || temp.boot == BOOT_UNKNOWN_TAG
        || temp.pid <= 0
    {
        return false;
    }
    if temp.boot == local_boot {
        let local_machine = local.machine_id.as_deref().unwrap_or(MACHINE_UNKNOWN_TAG);
        if temp.machine != local_machine || temp.pid_ns != local_pid_ns {
            return false;
        }
        matches!(
            inspect_process(temp.pid, temp.start_time),
            ProcessState::Dead | ProcessState::Reused
        )
    } else {
        local.machine_id.as_deref().is_some_and(|local_machine| {
            local_machine != MACHINE_UNKNOWN_TAG && temp.machine == local_machine
        })
    }
}

fn remove_stale_atomic_temporary_files(parent: &Path, name: &str, local_identity: &WriterIdentity) {
    if !local_identity.host_reap_safe
        || local_identity.boot_id.is_none()
        || local_identity.pid_ns.is_none()
        || local_identity.start_time.is_none()
    {
        return;
    }
    for (candidate, parsed) in scan_atomic_temporary_siblings(parent, name) {
        if should_reap_atomic_temp(local_identity, &parsed) {
            let _ = std::fs::remove_file(candidate);
        }
    }
}

fn process_is_alive(pid: libc::pid_t) -> bool {
    if pid <= 0 {
        return false;
    }
    // SAFETY: `pid > 0`, so signal 0 checks whether that single process exists.
    let rc = unsafe { libc::kill(pid, 0) };
    if rc == 0 {
        return true;
    }
    std::io::Error::last_os_error().raw_os_error() == Some(libc::EPERM)
}

#[cfg(test)]
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum AtomicWriteOp {
    SyncTemp,
    Rename,
    SyncParentDir(PathBuf),
}

#[cfg(test)]
pub(crate) mod atomic_write_trace {
    use super::AtomicWriteOp;
    use std::cell::RefCell;

    thread_local! {
        static OPS: RefCell<Option<Vec<AtomicWriteOp>>> = const { RefCell::new(None) };
    }

    pub(super) fn record(op: AtomicWriteOp) {
        OPS.with(|cell| {
            if let Some(ops) = cell.borrow_mut().as_mut() {
                ops.push(op);
            }
        });
    }

    pub(crate) struct Guard;

    impl Guard {
        pub(crate) fn start() -> Self {
            OPS.with(|cell| *cell.borrow_mut() = Some(Vec::new()));
            Self
        }

        pub(crate) fn take(&self) -> Vec<AtomicWriteOp> {
            OPS.with(|cell| {
                cell.borrow_mut()
                    .as_mut()
                    .map(std::mem::take)
                    .unwrap_or_default()
            })
        }
    }

    impl Drop for Guard {
        fn drop(&mut self) {
            OPS.with(|cell| *cell.borrow_mut() = None);
        }
    }
}

#[cfg(test)]
pub(crate) mod dir_sync_fault_hook {
    use std::cell::RefCell;

    thread_local! {
        static FAULT: RefCell<Option<libc::c_int>> = const { RefCell::new(None) };
    }

    pub(super) fn take() -> Option<std::io::Error> {
        FAULT.with(|slot| {
            slot.borrow_mut()
                .take()
                .map(std::io::Error::from_raw_os_error)
        })
    }

    pub(crate) struct Guard;

    impl Guard {
        pub(crate) fn inject(errno: libc::c_int) -> Self {
            FAULT.with(|slot| *slot.borrow_mut() = Some(errno));
            Self
        }
    }

    impl Drop for Guard {
        fn drop(&mut self) {
            FAULT.with(|slot| *slot.borrow_mut() = None);
        }
    }
}

fn sync_temp_file(file: &std::fs::File) -> std::io::Result<()> {
    file.sync_all()?;
    #[cfg(test)]
    atomic_write_trace::record(AtomicWriteOp::SyncTemp);
    Ok(())
}

fn rename_atomic_temp(temporary: &Path, destination: &Path) -> std::io::Result<()> {
    std::fs::rename(temporary, destination)?;
    #[cfg(test)]
    atomic_write_trace::record(AtomicWriteOp::Rename);
    Ok(())
}

fn is_dir_sync_unsupported(error: &std::io::Error) -> bool {
    matches!(
        error.raw_os_error(),
        Some(libc::EINVAL | libc::ENOTSUP | libc::EBADF)
    )
}

fn sync_dir_handle(dir: &std::fs::File) -> std::io::Result<()> {
    #[cfg(test)]
    if let Some(error) = dir_sync_fault_hook::take() {
        return Err(error);
    }
    dir.sync_all()
}

fn sync_parent_dir(parent: &Path) -> std::io::Result<()> {
    let dir = std::fs::File::open(parent)?;
    match sync_dir_handle(&dir) {
        Ok(()) => {
            #[cfg(test)]
            atomic_write_trace::record(AtomicWriteOp::SyncParentDir(parent.to_path_buf()));
            Ok(())
        }
        Err(error) if is_dir_sync_unsupported(&error) => {
            #[cfg(test)]
            atomic_write_trace::record(AtomicWriteOp::SyncParentDir(parent.to_path_buf()));
            Ok(())
        }
        Err(error) => Err(error),
    }
}

/// Split a string on whitespace, honouring single and double quotes.
pub fn split_args(s: &str) -> Result<Vec<String>, Fail> {
    let mut out = Vec::new();
    let mut cur = String::new();
    let mut quote: Option<char> = None;
    let mut have = false;
    for c in s.chars() {
        match quote {
            Some(q) if c == q => quote = None,
            Some(_) => cur.push(c),
            None if c == '\'' || c == '"' => {
                quote = Some(c);
                have = true;
            }
            None if c.is_whitespace() => {
                if have || !cur.is_empty() {
                    out.push(std::mem::take(&mut cur));
                    have = false;
                }
            }
            None => cur.push(c),
        }
    }
    if quote.is_some() {
        return Err(Fail::usage(format!("unterminated quote in `{s}`")));
    }
    if have || !cur.is_empty() {
        out.push(cur);
    }
    Ok(out)
}

/// Quote `a` for a shell: as it is when it holds only characters a shell
/// leaves alone, else in single quotes. A single quote in `a` is written
/// `'"'"'`, which a shell and [`split_args`] both read back.
///
/// A build list line is not a shell command. [`list_word`] writes one word
/// of a build list, and leaves a parenthesis or a star bare. `stack shard`
/// splits on white space and keeps the quotes a shell would add.
pub fn quote(a: &str) -> String {
    if !a.is_empty()
        && a.chars()
            .all(|c| c.is_ascii_alphanumeric() || "-_./=:,@%+".contains(c))
    {
        a.to_string()
    } else {
        format!("'{}'", a.replace('\'', "'\"'\"'"))
    }
}

/// One word of a build list line, which [`parse_list`] reads back as `word`.
///
/// The word stays bare when the reader already keeps it as one word, including
/// a parenthesis or a star that a shell would quote. A word that holds a space
/// or a quote is quoted so the reader still returns it: double quotes when it
/// holds an apostrophe, single quotes otherwise, and the two side by side when
/// it holds both. A line break cannot be one word of one line.
pub fn list_word(word: &str) -> Result<String, Fail> {
    if word.contains(['\n', '\r']) {
        return Err(Fail::usage(
            "a build list word holds a line break, which one line cannot hold",
        ));
    }
    if word.is_empty() {
        return Ok("''".into());
    }
    if is_bare_list_word(word) {
        return Ok(word.to_string());
    }
    if !word.contains('\'') {
        return Ok(format!("'{word}'"));
    }
    if !word.contains('"') {
        return Ok(format!("\"{word}\""));
    }
    let mut out = String::new();
    let mut chars = word.chars().peekable();
    while chars.peek().is_some() {
        let quote = if chars.peek() == Some(&'\'') {
            '"'
        } else {
            '\''
        };
        let mut run = String::new();
        while let Some(&c) = chars.peek() {
            if c == quote {
                break;
            }
            run.push(c);
            chars.next();
        }
        out.push(quote);
        out.push_str(&run);
        out.push(quote);
    }
    Ok(out)
}

/// Whether `word` is already one word to [`parse_list`], and not a comment
/// when it is the first word of a line.
fn is_bare_list_word(word: &str) -> bool {
    !word.starts_with('#')
        && !word
            .chars()
            .any(|c| c.is_whitespace() || c == '\'' || c == '"')
}

/// Whether `s` is a log stamp: a UTC time as `YYYYMMDD_HHMMSS`, the name
/// `eb-keep-logs rotate` ages as a run directory.
pub fn is_log_stamp(s: &str) -> bool {
    let b = s.as_bytes();
    b.len() == 15
        && b[8] == b'_'
        && b[..8].iter().chain(&b[9..]).all(u8::is_ascii_digit)
        && chrono::NaiveDateTime::parse_from_str(s, "%Y%m%d_%H%M%S").is_ok()
}

/// Parse dotted numbers (`1.2.3`); a leading `v` is accepted.
pub fn parse_version(s: &str) -> Option<Vec<u64>> {
    let s = s.trim().trim_start_matches('v');
    if s.is_empty() {
        return None;
    }
    s.split('.').map(|p| p.parse::<u64>().ok()).collect()
}

/// Compare two dotted versions; missing components count as zero.
pub fn cmp_version(a: &[u64], b: &[u64]) -> std::cmp::Ordering {
    let n = a.len().max(b.len());
    for i in 0..n {
        let x = a.get(i).copied().unwrap_or(0);
        let y = b.get(i).copied().unwrap_or(0);
        if x != y {
            return x.cmp(&y);
        }
    }
    std::cmp::Ordering::Equal
}

/// First dotted-number token of a `--version` output.
pub fn find_version(output: &str) -> Option<Vec<u64>> {
    for tok in output.split(|c: char| c.is_whitespace() || c == ',' || c == '(' || c == ')') {
        let t = tok.trim_start_matches('v');
        if t.chars().next().is_some_and(|c| c.is_ascii_digit()) {
            let core: String = t
                .chars()
                .take_while(|c| c.is_ascii_digit() || *c == '.')
                .collect();
            if let Some(v) = parse_version(core.trim_end_matches('.')) {
                return Some(v);
            }
        }
    }
    None
}

/// One line of a build list.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ListLine {
    /// The line as written, trimmed.
    pub raw: String,
    /// Easyconfig file name or path.
    pub easyconfig: String,
    /// Remaining easybuild arguments.
    pub args: Vec<String>,
}

/// Read build list lines: blank lines and `#` comments are skipped.
pub fn parse_list(text: &str) -> Result<Vec<ListLine>, Fail> {
    let mut out = Vec::new();
    let text = text.strip_prefix('\u{feff}').unwrap_or(text);
    for l in text.lines() {
        let t = l.trim();
        if t.is_empty() || t.starts_with('#') {
            continue;
        }
        let mut parts = split_args(t)?;
        let easyconfig = parts.remove(0);
        out.push(ListLine {
            raw: t.to_string(),
            easyconfig,
            args: parts,
        });
    }
    Ok(out)
}

/// Package name and version of an easyconfig file name `Name-version[-toolchain].eb`.
pub fn package_of(easyconfig: &str) -> (String, Option<String>) {
    let base = easyconfig.rsplit('/').next().unwrap_or(easyconfig);
    let stem = base.strip_suffix(".eb").unwrap_or(base);
    let parts: Vec<&str> = stem.split('-').collect();
    let mut name = Vec::new();
    let mut i = 0;
    while i < parts.len() {
        if i > 0 && parts[i].chars().next().is_some_and(|c| c.is_ascii_digit()) {
            break;
        }
        name.push(parts[i]);
        i += 1;
    }
    (name.join("-"), parts.get(i).map(|s| s.to_string()))
}

#[cfg(test)]
pub(crate) mod atomic_test_support {
    use super::*;
    use std::process::Command;
    use std::sync::{Mutex, MutexGuard, OnceLock};

    pub(crate) fn guard() -> MutexGuard<'static, ()> {
        static GUARD: OnceLock<Mutex<()>> = OnceLock::new();
        GUARD
            .get_or_init(|| Mutex::new(()))
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
    }

    pub(crate) struct IdentityOverrideGuard;

    impl Drop for IdentityOverrideGuard {
        fn drop(&mut self) {
            writer_identity_test_hook::clear();
        }
    }

    pub(super) fn install_identity_override(identity: WriterIdentity) -> IdentityOverrideGuard {
        writer_identity_test_hook::install(identity);
        IdentityOverrideGuard
    }

    pub(super) const REAP_SAFE_TEST_HOST: &str = "atomic-test-host";
    pub(super) const REAP_SAFE_TEST_MACHINE: &str = "0123456789abcdef0123456789abcdef";
    pub(super) const REAP_SAFE_TEST_BOOT: &str = "boot-current-1";
    pub(super) const REAP_SAFE_TEST_PID_NS: u64 = 4_026_531_836;

    pub(super) fn reap_safe_test_identity() -> WriterIdentity {
        assert_eq!(
            host_tag_and_reap_safety(Some(REAP_SAFE_TEST_HOST)),
            (REAP_SAFE_TEST_HOST.to_string(), true)
        );
        let start_time = process_start_time(std::process::id() as libc::pid_t)
            .expect("current process start time should be readable");
        WriterIdentity {
            host: REAP_SAFE_TEST_HOST.into(),
            host_reap_safe: true,
            machine_id: Some(REAP_SAFE_TEST_MACHINE.into()),
            boot_id: Some(REAP_SAFE_TEST_BOOT.into()),
            pid_ns: Some(REAP_SAFE_TEST_PID_NS),
            start_time: Some(start_time),
        }
    }

    pub(crate) fn install_reap_safe_identity() -> IdentityOverrideGuard {
        install_identity_override(reap_safe_test_identity())
    }

    pub(crate) fn spawned_and_reaped_pid() -> libc::pid_t {
        let mut child = Command::new("sh").arg("-c").arg("exit 0").spawn().unwrap();
        let pid = child.id() as libc::pid_t;
        let _ = child.wait();
        pid
    }

    pub(crate) fn dead_local_temp_name(output_name: &str, attempt: u32) -> String {
        let identity = reap_safe_test_identity();
        let dead_pid = spawned_and_reaped_pid();
        atomic_temp_file_name(output_name, &identity, dead_pid, attempt)
    }

    pub(crate) fn foreign_host_temp_name(output_name: &str, attempt: u32) -> String {
        let mut identity = reap_safe_test_identity();
        identity.host = "other-host".into();
        let dead_pid = spawned_and_reaped_pid();
        atomic_temp_file_name(output_name, &identity, dead_pid, attempt)
    }
}

#[cfg(test)]
mod tests {
    use super::atomic_test_support::{
        guard as atomic_test_guard, install_identity_override, reap_safe_test_identity,
        spawned_and_reaped_pid, REAP_SAFE_TEST_MACHINE,
    };
    use super::*;
    use std::ffi::CString;
    use std::os::unix::ffi::OsStrExt;
    use std::os::unix::process::ExitStatusExt;
    use std::process::{Command, Stdio};

    struct EnvVarGuard {
        name: &'static str,
        previous: Option<OsString>,
    }

    impl EnvVarGuard {
        fn set(name: &'static str, value: Option<&str>) -> Self {
            let previous = std::env::var_os(name);
            match value {
                Some(value) => std::env::set_var(name, value),
                None => std::env::remove_var(name),
            }
            Self { name, previous }
        }
    }

    impl Drop for EnvVarGuard {
        fn drop(&mut self) {
            match &self.previous {
                Some(value) => std::env::set_var(self.name, value),
                None => std::env::remove_var(self.name),
            }
        }
    }

    fn crash_write_atomic(path: &Path, stage: &str, bytes: &[u8]) -> std::process::ExitStatus {
        let output = Command::new(std::env::current_exe().unwrap())
            .arg("--exact")
            .arg("util::tests::atomic_write_recovers_after_kills_at_write_steps")
            .env("EB_PIPELINE_TEST_KILL_PATH", path)
            .env(
                "EB_PIPELINE_TEST_KILL_BYTES",
                String::from_utf8_lossy(bytes).into_owned(),
            )
            .env("EB_PIPELINE_TEST_KILL_WRITE_ATOMIC", stage)
            .output()
            .unwrap();
        output.status
    }

    fn spawn_rlimit_fsize_crash(
        path: &Path,
        work_dir: Option<&Path>,
    ) -> (libc::pid_t, PathBuf, AtomicTempFile) {
        let mut cmd = Command::new(std::env::current_exe().unwrap());
        cmd.arg("--exact")
            .arg("util::tests::atomic_write_reaps_temp_left_by_rlimit_fsize_crash")
            .env("EB_PIPELINE_TEST_ATOMIC_RLIMIT_PATH", path)
            .stdout(Stdio::null())
            .stderr(Stdio::null());
        if let Some(dir) = work_dir {
            cmd.current_dir(dir);
        }
        let mut child = cmd.spawn().unwrap();
        let child_pid = child.id() as libc::pid_t;
        let status = child.wait().unwrap();
        assert_eq!(
            status.signal(),
            Some(libc::SIGXFSZ),
            "expected deterministic SIGXFSZ from RLIMIT_FSIZE=0, got {status:?}"
        );
        let scan_target = match work_dir {
            Some(dir) => dir.join(path),
            None => path.to_path_buf(),
        };
        let siblings = atomic_temporary_siblings(&scan_target);
        assert_eq!(
            siblings.len(),
            1,
            "expected one stale temp after SIGXFSZ, found {siblings:?}"
        );
        let (temp_path, parsed) = siblings.into_iter().next().unwrap();
        assert_eq!(parsed.pid, child_pid);
        (child_pid, temp_path, parsed)
    }

    #[test]
    fn regular_text_reader_rejects_hostile_inputs_promptly() {
        let dir = tempfile::tempdir().unwrap();

        let fifo = dir.path().join("input.fifo");
        let raw = CString::new(fifo.as_os_str().as_bytes()).unwrap();
        assert_eq!(unsafe { libc::mkfifo(raw.as_ptr(), 0o600) }, 0);
        let (send, receive) = std::sync::mpsc::channel();
        let reader = std::thread::spawn(move || {
            send.send(read_text_file(&fifo)).unwrap();
        });
        let error = receive
            .recv_timeout(std::time::Duration::from_secs(5))
            .expect("FIFO reader blocked")
            .unwrap_err();
        reader.join().unwrap();
        assert_eq!(error.kind(), std::io::ErrorKind::InvalidInput);

        let error = read_text_file(dir.path()).unwrap_err();
        assert_eq!(error.kind(), std::io::ErrorKind::IsADirectory);

        let dangling = dir.path().join("dangling");
        std::os::unix::fs::symlink("gone", &dangling).unwrap();
        let error = read_text_file(&dangling).unwrap_err();
        assert_eq!(error.kind(), std::io::ErrorKind::NotFound);

        let invalid = dir.path().join("invalid");
        std::fs::write(&invalid, [0xff]).unwrap();
        let error = read_text_file(&invalid).unwrap_err();
        assert_eq!(error.kind(), std::io::ErrorKind::InvalidData);

        let bom = dir.path().join("bom");
        std::fs::write(&bom, "\u{feff}text").unwrap();
        assert_eq!(read_text_file(&bom).unwrap(), "\u{feff}text");
    }

    #[test]
    fn atomic_write_replaces_only_after_the_new_file_is_complete() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("result.json");
        std::fs::write(&path, b"old bytes\n").unwrap();
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o640)).unwrap();
        let old_inode = std::fs::metadata(&path).unwrap().ino();

        write_atomic(&path, b"complete new bytes\n").unwrap();

        let metadata = std::fs::metadata(&path).unwrap();
        assert_ne!(metadata.ino(), old_inode);
        assert_eq!(metadata.permissions().mode() & 0o777, 0o640);
        assert_eq!(std::fs::read(&path).unwrap(), b"complete new bytes\n");
        assert_eq!(std::fs::read_dir(dir.path()).unwrap().count(), 1);
    }

    #[test]
    fn atomic_write_failure_preserves_the_previous_file() {
        let _guard = atomic_test_guard();
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("result.json");
        std::fs::write(&path, b"complete old bytes\n").unwrap();
        let identity = local_writer_identity();
        for attempt in 0..1000u32 {
            std::fs::write(
                dir.path().join(atomic_temp_file_name(
                    "result.json",
                    &identity,
                    std::process::id() as libc::pid_t,
                    attempt,
                )),
                b"occupied\n",
            )
            .unwrap();
        }

        let error = write_atomic(&path, b"new bytes\n").unwrap_err();

        assert_eq!(error.kind(), std::io::ErrorKind::AlreadyExists);
        assert_eq!(std::fs::read(&path).unwrap(), b"complete old bytes\n");
    }

    #[test]
    fn atomic_write_leaves_foreign_host_temps_alone() {
        let _guard = atomic_test_guard();
        let local = reap_safe_test_identity();
        let _override = install_identity_override(local.clone());
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("result.json");
        std::fs::write(&path, b"old bytes\n").unwrap();

        let foreign_pid = spawned_and_reaped_pid();
        let foreign = dir.path().join(format!(
            ".result.json.{}.{}.{}.{}.{}.{}.1.tmp",
            "other-host",
            local.machine_id.as_deref().unwrap(),
            local.boot_id.as_deref().unwrap(),
            local.pid_ns.unwrap(),
            foreign_pid,
            local.start_time.unwrap(),
        ));
        std::fs::write(&foreign, b"foreign temp\n").unwrap();
        let status = crash_write_atomic(&path, "after-temp-created", b"new bytes\n");
        assert_eq!(status.signal(), Some(libc::SIGKILL), "{status:?}");
        let locals_before = atomic_temporary_siblings(&path)
            .into_iter()
            .filter(|(candidate, _)| candidate != &foreign)
            .count();
        assert_eq!(
            locals_before, 1,
            "expected one local temp from killed writer"
        );

        write_atomic(&path, b"new bytes\n").unwrap();

        assert!(foreign.exists(), "foreign temp unexpectedly removed");
        let locals_after = atomic_temporary_siblings(&path)
            .into_iter()
            .filter(|(candidate, _)| candidate != &foreign)
            .count();
        assert_eq!(locals_after, 0, "local stale temp was not removed");
        assert_eq!(std::fs::read(&path).unwrap(), b"new bytes\n");
    }

    #[test]
    fn atomic_write_reaps_same_host_temps_from_earlier_boot() {
        let _guard = atomic_test_guard();
        let local = reap_safe_test_identity();
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("result.json");
        std::fs::write(&path, b"old bytes\n").unwrap();

        {
            let _override = install_identity_override(local.clone());
            let same_machine_stale = dir.path().join(format!(
                ".result.json.{}.{}.earlier-boot.{}.{}.{}.1.tmp",
                local.host,
                local.machine_id.as_deref().unwrap(),
                local.pid_ns.unwrap(),
                std::process::id(),
                local.start_time.unwrap(),
            ));
            std::fs::write(&same_machine_stale, b"earlier boot temp\n").unwrap();

            write_atomic(&path, b"new bytes\n").unwrap();

            assert!(
                !same_machine_stale.exists(),
                "earlier-boot temp from the same machine was not removed"
            );
            assert_eq!(std::fs::read(&path).unwrap(), b"new bytes\n");
        }

        {
            let _override = install_identity_override(WriterIdentity {
                machine_id: None,
                ..local.clone()
            });
            let unverified_stale = dir.path().join(format!(
                ".result.json.{}.{}.earlier-boot.{}.{}.{}.1.tmp",
                local.host,
                MACHINE_UNKNOWN_TAG,
                local.pid_ns.unwrap(),
                std::process::id(),
                local.start_time.unwrap(),
            ));
            std::fs::write(&unverified_stale, b"unverified earlier boot temp\n").unwrap();

            write_atomic(&path, b"later bytes\n").unwrap();

            assert!(
                unverified_stale.exists(),
                "earlier-boot temp without a machine id was unexpectedly removed"
            );
            assert_eq!(std::fs::read(&path).unwrap(), b"later bytes\n");
        }
    }

    #[test]
    fn atomic_write_keeps_shared_host_temps_from_different_machine() {
        let _guard = atomic_test_guard();
        let local = reap_safe_test_identity();
        let _override = install_identity_override(local.clone());
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("result.json");
        std::fs::write(&path, b"old bytes\n").unwrap();

        let live_pid = std::process::id() as libc::pid_t;
        let start_time = local.start_time.unwrap();
        let pid_ns = local.pid_ns.unwrap();
        let other_machine_temp = dir.path().join(format!(
            ".result.json.{}.{}.other-boot.{pid_ns}.{live_pid}.{start_time}.1.tmp",
            local.host, "fedcba9876543210fedcba9876543210"
        ));
        let unknown_machine_temp = dir.path().join(format!(
            ".result.json.{}.{}.other-boot.{pid_ns}.{live_pid}.{start_time}.2.tmp",
            local.host, MACHINE_UNKNOWN_TAG
        ));
        let same_machine_earlier_boot = dir.path().join(format!(
            ".result.json.{}.{}.earlier-boot.{pid_ns}.{live_pid}.{start_time}.3.tmp",
            local.host,
            local.machine_id.as_deref().unwrap()
        ));
        std::fs::write(&other_machine_temp, b"other machine live temp\n").unwrap();
        std::fs::write(&unknown_machine_temp, b"unknown machine live temp\n").unwrap();
        std::fs::write(&same_machine_earlier_boot, b"same machine old boot\n").unwrap();

        write_atomic(&path, b"new bytes\n").unwrap();

        assert!(
            other_machine_temp.exists(),
            "shared-host temp from a different machine was removed"
        );
        assert!(
            unknown_machine_temp.exists(),
            "shared-host temp without a machine id was removed"
        );
        assert!(
            !same_machine_earlier_boot.exists(),
            "same-machine earlier-boot temp was not removed"
        );
        assert_eq!(std::fs::read(&path).unwrap(), b"new bytes\n");
    }

    #[test]
    fn atomic_write_keeps_same_boot_temps_from_different_pid_namespace() {
        let _guard = atomic_test_guard();
        let local = reap_safe_test_identity();
        let _override = install_identity_override(local.clone());
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("result.json");
        std::fs::write(&path, b"old bytes\n").unwrap();

        let other_ns = local.pid_ns.unwrap() + 1;
        let dead_pid = spawned_and_reaped_pid();
        let start_time = local.start_time.unwrap();
        let other_ns_dead_pid = dir.path().join(format!(
            ".result.json.{}.{}.{}.{other_ns}.{dead_pid}.{start_time}.1.tmp",
            local.host,
            local.machine_id.as_deref().unwrap(),
            local.boot_id.as_deref().unwrap(),
        ));
        let other_ns_reused_start = dir.path().join(format!(
            ".result.json.{}.{}.{}.{other_ns}.{}.{}.2.tmp",
            local.host,
            local.machine_id.as_deref().unwrap(),
            local.boot_id.as_deref().unwrap(),
            std::process::id(),
            start_time.wrapping_add(1),
        ));
        let same_ns_dead_pid =
            dir.path()
                .join(atomic_temp_file_name("result.json", &local, dead_pid, 3));
        std::fs::write(&other_ns_dead_pid, b"other ns dead pid\n").unwrap();
        std::fs::write(&other_ns_reused_start, b"other ns reused start\n").unwrap();
        std::fs::write(&same_ns_dead_pid, b"same ns dead pid\n").unwrap();

        write_atomic(&path, b"new bytes\n").unwrap();

        assert!(
            other_ns_dead_pid.exists(),
            "temp from a different PID namespace was removed"
        );
        assert!(
            other_ns_reused_start.exists(),
            "temp from a different PID namespace was removed on start mismatch"
        );
        assert!(
            !same_ns_dead_pid.exists(),
            "stale temp in the same PID namespace was not removed"
        );
        assert_eq!(std::fs::read(&path).unwrap(), b"new bytes\n");
    }

    #[test]
    fn atomic_write_reaps_same_boot_temp_when_pid_was_reused() {
        let _guard = atomic_test_guard();
        let local = reap_safe_test_identity();
        let _override = install_identity_override(local.clone());
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("result.json");
        std::fs::write(&path, b"old bytes\n").unwrap();

        let live_pid = std::process::id() as libc::pid_t;
        let stale_start_time = local.start_time.unwrap().wrapping_add(1);
        let reused_pid_temp = dir.path().join(format!(
            ".result.json.{}.{}.{}.{}.{live_pid}.{stale_start_time}.1.tmp",
            local.host,
            local.machine_id.as_deref().unwrap(),
            local.boot_id.as_deref().unwrap(),
            local.pid_ns.unwrap(),
        ));
        std::fs::write(&reused_pid_temp, b"stale temp from reused pid\n").unwrap();

        write_atomic(&path, b"new bytes\n").unwrap();

        assert!(
            !reused_pid_temp.exists(),
            "stale temp with mismatched start time for reused pid was not removed"
        );
        assert_eq!(std::fs::read(&path).unwrap(), b"new bytes\n");
    }

    #[test]
    fn atomic_write_does_not_reap_when_boot_id_is_unavailable() {
        let _guard = atomic_test_guard();
        let local = reap_safe_test_identity();
        let _override = install_identity_override(WriterIdentity {
            boot_id: None,
            ..local.clone()
        });
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("result.json");
        std::fs::write(&path, b"old bytes\n").unwrap();
        let stale_pid = spawned_and_reaped_pid();
        let stale = dir.path().join(format!(
            ".result.json.{}.{}.earlier-boot.{}.{}.{}.1.tmp",
            local.host,
            local.machine_id.as_deref().unwrap(),
            local.pid_ns.unwrap(),
            stale_pid,
            local.start_time.unwrap(),
        ));
        std::fs::write(&stale, b"stale temp\n").unwrap();

        write_atomic(&path, b"new bytes\n").unwrap();

        assert!(stale.exists(), "temp was reaped without local boot id");
        assert_eq!(std::fs::read(&path).unwrap(), b"new bytes\n");
    }

    #[test]
    fn atomic_write_does_not_reap_when_pid_ns_or_start_time_is_unavailable() {
        let _guard = atomic_test_guard();
        let local = reap_safe_test_identity();
        let cases = [
            WriterIdentity {
                pid_ns: None,
                ..local.clone()
            },
            WriterIdentity {
                start_time: None,
                ..local.clone()
            },
        ];
        for incomplete in cases {
            let _override = install_identity_override(incomplete);
            let dir = tempfile::tempdir().unwrap();
            let path = dir.path().join("result.json");
            std::fs::write(&path, b"old bytes\n").unwrap();
            let dead_pid = spawned_and_reaped_pid();
            let stale = dir
                .path()
                .join(atomic_temp_file_name("result.json", &local, dead_pid, 1));
            std::fs::write(&stale, b"stale temp\n").unwrap();

            write_atomic(&path, b"new bytes\n").unwrap();

            assert!(
                stale.exists(),
                "temp was reaped when namespace or start time was unknown"
            );
            assert_eq!(std::fs::read(&path).unwrap(), b"new bytes\n");
        }
    }

    #[test]
    fn atomic_write_does_not_reap_when_host_tag_is_not_unique() {
        let _guard = atomic_test_guard();
        let local = reap_safe_test_identity();
        let raw_cases = [
            None,
            Some(""),
            Some("localhost"),
            Some("node.example"),
            Some(HOST_UNKNOWN_TAG),
        ];
        for raw in raw_cases {
            let (host, host_reap_safe) = host_tag_and_reap_safety(raw);
            assert!(!host_reap_safe, "{raw:?} unexpectedly marked reap-safe");
            let _override = install_identity_override(WriterIdentity {
                host: host.clone(),
                host_reap_safe,
                ..local.clone()
            });
            let dir = tempfile::tempdir().unwrap();
            let path = dir.path().join("result.json");
            std::fs::write(&path, b"old bytes\n").unwrap();
            let stale_pid = spawned_and_reaped_pid();
            let stale = dir.path().join(format!(
                ".result.json.{}.{}.earlier-boot.{}.{}.{}.1.tmp",
                host,
                REAP_SAFE_TEST_MACHINE,
                local.pid_ns.unwrap(),
                stale_pid,
                local.start_time.unwrap(),
            ));
            std::fs::write(&stale, b"stale temp\n").unwrap();
            write_atomic(&path, b"new bytes\n").unwrap();
            assert!(
                stale.exists(),
                "{raw:?}: stale temp was unexpectedly reaped"
            );
        }
    }

    #[test]
    fn atomic_write_keeps_same_boot_temps_for_live_pids() {
        let _guard = atomic_test_guard();
        let local = reap_safe_test_identity();
        let _override = install_identity_override(local.clone());
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("result.json");
        std::fs::write(&path, b"old bytes\n").unwrap();
        let live_temp = dir.path().join(atomic_temp_file_name(
            "result.json",
            &local,
            std::process::id() as libc::pid_t,
            1,
        ));
        std::fs::write(&live_temp, b"live temp\n").unwrap();

        write_atomic(&path, b"new bytes\n").unwrap();

        assert!(live_temp.exists(), "temp from a live pid was reaped");
        assert_eq!(std::fs::read(&path).unwrap(), b"new bytes\n");
    }

    #[test]
    fn atomic_write_recovers_after_kills_at_write_steps() {
        if let Some(child_path) = std::env::var_os("EB_PIPELINE_TEST_KILL_PATH") {
            let _override = install_identity_override(reap_safe_test_identity());
            let bytes = std::env::var("EB_PIPELINE_TEST_KILL_BYTES").unwrap();
            let _ = write_atomic(Path::new(&child_path), bytes.as_bytes());
            return;
        }
        let _guard = atomic_test_guard();
        let _override = install_identity_override(reap_safe_test_identity());
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("result.json");
        let new_bytes = b"complete new bytes\n";
        for stage in [
            "after-temp-created",
            "mid-write",
            "before-rename",
            "after-rename-before-dir-sync",
        ] {
            std::fs::write(&path, b"old bytes\n").unwrap();
            let status = crash_write_atomic(&path, stage, new_bytes);
            assert_eq!(status.signal(), Some(libc::SIGKILL), "{stage}: {status:?}");
            let seen = std::fs::read(&path).unwrap();
            let siblings = atomic_temporary_siblings(&path);
            if stage == "after-rename-before-dir-sync" {
                assert_eq!(seen, new_bytes, "{stage}: expected new bytes after rename");
                assert!(
                    siblings.is_empty(),
                    "{stage}: unexpected stale temp after rename: {siblings:?}"
                );
            } else {
                assert_eq!(
                    seen, b"old bytes\n",
                    "{stage}: pre-rename kill should keep old bytes"
                );
                assert_eq!(
                    siblings.len(),
                    1,
                    "{stage}: pre-rename kill should leave one stale temp until recovery: {siblings:?}"
                );
            }

            write_atomic(&path, new_bytes).unwrap();
            assert_eq!(std::fs::read(&path).unwrap(), new_bytes, "{stage}");
            assert!(
                atomic_temporary_siblings(&path).is_empty(),
                "{stage}: stale local temp files remained: {:?}",
                atomic_temporary_siblings(&path)
            );
        }
    }

    #[test]
    fn atomic_write_reaps_temp_left_by_rlimit_fsize_crash() {
        if let Some(child_path) = std::env::var_os("EB_PIPELINE_TEST_ATOMIC_RLIMIT_PATH") {
            // SAFETY: reset SIGXFSZ to default and set RLIMIT_FSIZE=0 in this
            // child process so write_atomic terminates synchronously on its
            // first write(2) while leaving the created empty temporary file.
            unsafe {
                libc::signal(libc::SIGXFSZ, libc::SIG_DFL);
                let limit = libc::rlimit {
                    rlim_cur: 0,
                    rlim_max: 0,
                };
                assert_eq!(libc::setrlimit(libc::RLIMIT_FSIZE, &limit), 0);
            }
            let _ = write_atomic(Path::new(&child_path), b"new bytes\n");
            std::process::exit(1);
        }

        let _guard = atomic_test_guard();
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("result.json");
        std::fs::write(&path, b"old bytes\n").unwrap();

        let (_child_pid, local_temp, parsed) = spawn_rlimit_fsize_crash(&path, None);
        assert_eq!(std::fs::read(&path).unwrap(), b"old bytes\n");

        let foreign_pid = spawned_and_reaped_pid();
        let foreign_host_name = if parsed.host == "other-host" {
            "foreign-host"
        } else {
            "other-host"
        };
        let foreign_host = dir.path().join(format!(
            ".result.json.{}.{}.{}.{}.{}.{}.1.tmp",
            foreign_host_name,
            parsed.machine,
            parsed.boot,
            parsed.pid_ns,
            foreign_pid,
            parsed.start_time,
        ));
        std::fs::write(&foreign_host, b"foreign host temp\n").unwrap();

        let other_machine_name = if parsed.machine == "other-machine-id" {
            "second-machine-id"
        } else {
            "other-machine-id"
        };
        let other_boot_name = if parsed.boot == "other-boot-id" {
            "second-boot-id"
        } else {
            "other-boot-id"
        };
        let other_machine_live = dir.path().join(format!(
            ".result.json.{}.{}.{}.{}.{}.{}.1.tmp",
            parsed.host,
            other_machine_name,
            other_boot_name,
            parsed.pid_ns,
            std::process::id(),
            parsed.start_time,
        ));
        std::fs::write(&other_machine_live, b"other machine live temp\n").unwrap();

        let other_ns_temp = dir.path().join(format!(
            ".result.json.{}.{}.{}.{}.{}.{}.1.tmp",
            parsed.host,
            parsed.machine,
            parsed.boot,
            parsed.pid_ns + 1,
            foreign_pid,
            parsed.start_time,
        ));
        std::fs::write(&other_ns_temp, b"other pid namespace temp\n").unwrap();

        write_atomic(&path, b"new bytes\n").unwrap();

        let local = local_writer_identity();
        if local.host_reap_safe
            && local.boot_id.is_some()
            && local.pid_ns.is_some()
            && local.start_time.is_some()
        {
            assert!(!local_temp.exists(), "local stale temp was not removed");
        } else {
            assert!(
                local_temp.exists(),
                "local temp should not be removed when kernel identity is incomplete"
            );
        }
        assert!(foreign_host.exists(), "foreign-host temp should stay");
        assert!(
            other_machine_live.exists(),
            "shared-host different-machine live temp should stay"
        );
        assert!(
            other_ns_temp.exists(),
            "different PID namespace temp should stay"
        );
        assert_eq!(std::fs::read(&path).unwrap(), b"new bytes\n");
    }

    #[test]
    fn atomic_temporary_name_parser_accepts_only_complete_tagged_names() {
        let parsed = parse_atomic_temporary_name(
            ".result.json.node-1.mach-1.boot-1.4026531836.42.9001.7.tmp",
            "result.json",
        );
        assert_eq!(
            parsed,
            Some(AtomicTempFile {
                host: "node-1".into(),
                machine: "mach-1".into(),
                boot: "boot-1".into(),
                pid_ns: 4_026_531_836,
                pid: 42,
                start_time: 9001,
                attempt: 7,
            })
        );

        for (file_name, output_name) in [
            (".cpu.json.tmp", "cpu.json"),
            (".result.json.tmp", "result.json"),
            (".cvmfs.prom.tmp", "cvmfs.prom"),
            (".result.json..tmp", "result.json"),
            (".result.json.", "result.json"),
            (".result.json", "result.json"),
            (".result.json.tmp.tmp", "result.json"),
            (".result.json.1.2.tmp", "result.json"),
            (".result.json.node-1.boot-1.42.7.tmp", "result.json"),
            (".result.json.node-1.mach-1.boot-1.42.7.tmp", "result.json"),
            (
                ".result.json.node-1.mach-1.boot-1.4026531836.42.7.tmp",
                "result.json",
            ),
            (
                ".result.json.node-1.mach-1.boot-1.4026531836.42.9001.7.8.tmp",
                "result.json",
            ),
            (
                ".result.json..mach-1.boot-1.4026531836.42.9001.7.tmp",
                "result.json",
            ),
            (
                ".result.json.node-1..boot-1.4026531836.42.9001.7.tmp",
                "result.json",
            ),
            (
                ".result.json.node-1.mach-1..4026531836.42.9001.7.tmp",
                "result.json",
            ),
            (
                ".result.json.node-1.mach-1.boot-1.0.42.9001.7.tmp",
                "result.json",
            ),
            (
                ".result.json.node-1.mach-1.boot-1.pidns-unknown.42.9001.7.tmp",
                "result.json",
            ),
            (
                ".result.json.node-1.mach-1.boot-1.4026531836.0.9001.7.tmp",
                "result.json",
            ),
            (
                ".result.json.node-1.mach-1.boot-1.4026531836.-1.9001.7.tmp",
                "result.json",
            ),
            (
                ".result.json.node-1.mach-1.boot-1.4026531836.42.start-unknown.7.tmp",
                "result.json",
            ),
            (
                ".result.json.node-1.mach-1.boot-1.4026531836.42.9001.7.tmp",
                "other.json",
            ),
            ("", "result.json"),
            ("tmp", ""),
            (".tmp", ""),
            ("..tmp", ""),
        ] {
            assert_eq!(
                parse_atomic_temporary_name(file_name, output_name),
                None,
                "{file_name:?} for {output_name:?}"
            );
        }
    }

    #[test]
    fn parse_proc_stat_start_time_handles_spaces_and_parentheses_in_comm() {
        let stat = "42 (my (weird) proc) S 1 2 3 4 5 6 7 8 9 10 11 12 13 14 15 16 17 18 987654 999";
        assert_eq!(parse_proc_stat_start_time(stat), Some(987_654));
        assert_eq!(parse_proc_stat_start_time("42 (broken"), None);
        assert_eq!(parse_proc_stat_start_time("42 (short) S 1 2"), None);
    }

    #[test]
    fn parse_proc_stat_extracts_state_and_start_time() {
        let live = "42 (my (weird) proc) S 1 2 3 4 5 6 7 8 9 10 11 12 13 14 15 16 17 18 987654 999";
        let zombie = "42 (sleep) Z 1 2 3 4 5 6 7 8 9 10 11 12 13 14 15 16 17 18 987654 999";
        let bad_state = "42 (sleep) ZZ 1 2 3 4 5 6 7 8 9 10 11 12 13 14 15 16 17 18 987654 999";
        assert_eq!(parse_proc_stat(live), Some(('S', 987_654)));
        assert_eq!(parse_proc_stat(zombie), Some(('Z', 987_654)));
        assert_eq!(parse_proc_stat(bad_state), None);
        assert_eq!(parse_proc_stat("42 (broken"), None);
        assert_eq!(parse_proc_stat("42 (short) S 1 2"), None);
    }

    #[test]
    fn inspect_process_fails_closed_when_stat_is_missing_for_live_pid() {
        assert_eq!(
            inspect_process_with(
                42,
                100,
                |_| Err(std::io::Error::from(std::io::ErrorKind::NotFound)),
                |_| true,
            ),
            ProcessState::AliveSameOrUnknown
        );
        assert_eq!(
            inspect_process_with(
                42,
                100,
                |_| Err(std::io::Error::from(std::io::ErrorKind::NotFound)),
                |_| false,
            ),
            ProcessState::Dead
        );
        assert_eq!(
            inspect_process_with(
                42,
                100,
                |_| Err(std::io::Error::from(std::io::ErrorKind::PermissionDenied)),
                |_| false,
            ),
            ProcessState::AliveSameOrUnknown
        );
        assert_eq!(
            inspect_process_with(
                42,
                100,
                |_| Ok("42 (p) S 1 2 3 4 5 6 7 8 9 10 11 12 13 14 15 16 17 18 200 0".into()),
                |_| true,
            ),
            ProcessState::Reused
        );
        assert_eq!(
            inspect_process_with(
                42,
                100,
                |_| Ok("42 (p) S 1 2 3 4 5 6 7 8 9 10 11 12 13 14 15 16 17 18 100 0".into()),
                |_| true,
            ),
            ProcessState::AliveSameOrUnknown
        );
        assert_eq!(
            inspect_process_with(0, 100, |_| panic!("should not read stat"), |_| false),
            ProcessState::AliveSameOrUnknown
        );
    }

    #[test]
    fn atomic_write_ignores_siblings_with_short_or_malformed_temp_names() {
        let _guard = atomic_test_guard();
        let _override = install_identity_override(reap_safe_test_identity());
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("result.json");
        std::fs::write(&path, b"old bytes\n").unwrap();
        let siblings = [
            ".result.json.tmp",
            ".result.json..tmp",
            ".result.json.tmp.tmp",
            ".result.json.1.2.tmp",
            ".result.json.atomic-test-host.boot.1.1.tmp",
            ".result.json.atomic-test-host.mach.boot.1.1.tmp",
            ".result.json.atomic-test-host.mach.boot.4026531836.0.10.1.tmp",
            ".result.json.atomic-test-host.mach.boot.4026531836.-1.10.1.tmp",
            ".result.json.atomic-test-host.mach.boot.pidns-unknown.10.10.1.tmp",
            ".result.json.atomic-test-host.mach.boot.4026531836.10.start-unknown.1.tmp",
        ];
        for sibling in siblings {
            std::fs::write(dir.path().join(sibling), b"unrelated\n").unwrap();
        }

        write_atomic(&path, b"new bytes\n").unwrap();

        assert_eq!(std::fs::read(&path).unwrap(), b"new bytes\n");
        for sibling in siblings {
            assert!(dir.path().join(sibling).exists(), "{sibling} was removed");
        }
    }

    #[test]
    fn atomic_write_preserves_non_utf8_siblings() {
        let _guard = atomic_test_guard();
        let local = reap_safe_test_identity();
        let _override = install_identity_override(local.clone());
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("result.json");
        std::fs::write(&path, b"old bytes\n").unwrap();

        let dead_pid = spawned_and_reaped_pid();
        let mut raw_name = format!(
            ".result.json.{}.{}.boot-",
            local.host,
            local.machine_id.as_deref().unwrap()
        )
        .into_bytes();
        raw_name.push(0xff);
        raw_name.extend_from_slice(
            format!(
                ".{}.{dead_pid}.{}.1.tmp",
                local.pid_ns.unwrap(),
                local.start_time.unwrap()
            )
            .as_bytes(),
        );
        let non_utf8 = dir.path().join(OsString::from_vec(raw_name));
        std::fs::write(&non_utf8, b"non-utf8 sibling\n").unwrap();

        let valid_stale =
            dir.path()
                .join(atomic_temp_file_name("result.json", &local, dead_pid, 2));
        std::fs::write(&valid_stale, b"stale temp\n").unwrap();

        write_atomic(&path, b"new bytes\n").unwrap();

        assert!(!valid_stale.exists(), "valid stale temp was not removed");
        assert!(non_utf8.exists(), "non-UTF-8 sibling was removed");
        assert_eq!(std::fs::read(&non_utf8).unwrap(), b"non-utf8 sibling\n");
        assert_eq!(std::fs::read(&path).unwrap(), b"new bytes\n");
    }

    #[test]
    fn atomic_write_removes_only_regular_stale_files() {
        let _guard = atomic_test_guard();
        let local = reap_safe_test_identity();
        let _override = install_identity_override(local.clone());
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("result.json");
        std::fs::write(&path, b"old bytes\n").unwrap();

        let dead_pid = spawned_and_reaped_pid();
        let target = dir.path().join("target.txt");
        std::fs::write(&target, b"target\n").unwrap();

        let stale_regular =
            dir.path()
                .join(atomic_temp_file_name("result.json", &local, dead_pid, 1));
        std::fs::write(&stale_regular, b"stale regular\n").unwrap();

        let stale_symlink =
            dir.path()
                .join(atomic_temp_file_name("result.json", &local, dead_pid, 2));
        std::os::unix::fs::symlink(&target, &stale_symlink).unwrap();

        let stale_dir = dir
            .path()
            .join(atomic_temp_file_name("result.json", &local, dead_pid, 3));
        std::fs::create_dir(&stale_dir).unwrap();

        let stale_fifo = dir
            .path()
            .join(atomic_temp_file_name("result.json", &local, dead_pid, 4));
        let raw_fifo = CString::new(stale_fifo.as_os_str().as_bytes()).unwrap();
        assert_eq!(unsafe { libc::mkfifo(raw_fifo.as_ptr(), 0o600) }, 0);

        write_atomic(&path, b"new bytes\n").unwrap();

        assert!(
            !stale_regular.exists(),
            "stale regular temp was not removed"
        );
        assert!(
            stale_symlink.is_symlink(),
            "stale-named symlink was removed"
        );
        assert!(stale_dir.is_dir(), "stale-named directory was removed");
        assert!(
            std::fs::symlink_metadata(&stale_fifo).is_ok(),
            "stale-named fifo was removed"
        );
        assert_eq!(std::fs::read(&path).unwrap(), b"new bytes\n");
    }

    #[test]
    fn atomic_write_reaps_stale_temp_for_bare_filename() {
        if std::env::var_os("EB_PIPELINE_TEST_BARE_FILENAME").is_some() {
            let _override = install_identity_override(reap_safe_test_identity());
            write_atomic(Path::new("result.json"), b"new bytes\n").unwrap();
            return;
        }
        let _guard = atomic_test_guard();
        let local = reap_safe_test_identity();
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("result.json");
        std::fs::write(&path, b"old bytes\n").unwrap();
        let dead_pid = spawned_and_reaped_pid();
        let stale = dir
            .path()
            .join(atomic_temp_file_name("result.json", &local, dead_pid, 1));
        std::fs::write(&stale, b"bare stale temp\n").unwrap();

        let output = Command::new(std::env::current_exe().unwrap())
            .arg("--exact")
            .arg("util::tests::atomic_write_reaps_stale_temp_for_bare_filename")
            .current_dir(dir.path())
            .env("EB_PIPELINE_TEST_BARE_FILENAME", "1")
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "{:?}: {}",
            output.status,
            String::from_utf8_lossy(&output.stderr)
        );
        assert!(!stale.exists(), "bare-filename stale temp was not removed");
        assert_eq!(std::fs::read(&path).unwrap(), b"new bytes\n");
    }

    #[test]
    fn kernel_host_selection_skips_blank_names() {
        let some = |text: &str| Some(text.to_string());
        assert_eq!(select_kernel_host(some("a"), some("b")), some("a"));
        assert_eq!(select_kernel_host(some(" a \n"), some("b")), some("a"));
        assert_eq!(select_kernel_host(some(""), some("b\n")), some("b"));
        assert_eq!(select_kernel_host(some(" \t"), some("b")), some("b"));
        assert_eq!(select_kernel_host(None, some("b")), some("b"));
        assert_eq!(select_kernel_host(some(""), some("\n")), None);
        assert_eq!(select_kernel_host(None, None), None);
    }

    #[test]
    fn detect_writer_identity_ignores_hostname_env_var() {
        let _guard = atomic_test_guard();
        let baseline = {
            let _env = EnvVarGuard::set("HOSTNAME", None);
            detect_writer_identity()
        };
        for value in ["", "localhost", "spoofed.example", "spoofed-host-tag"] {
            let _env = EnvVarGuard::set("HOSTNAME", Some(value));
            let observed = detect_writer_identity();
            assert_eq!(
                observed, baseline,
                "HOSTNAME={value:?} altered detected writer identity"
            );
            assert_ne!(observed.host, "spoofed-host-tag");
            let dir = tempfile::tempdir().unwrap();
            let path = dir.path().join("result.json");
            std::fs::write(&path, b"old bytes\n").unwrap();
            let (_pid, _temp, parsed) = spawn_rlimit_fsize_crash(&path, None);
            assert_eq!(parsed.host, baseline.host);
            assert_ne!(parsed.host, "spoofed-host-tag");
        }
    }

    #[test]
    fn boot_and_machine_tags_fail_closed_when_invalid() {
        assert_eq!(
            boot_tag("0a1b2c3d-4e5f-6071-8293-a4b5c6d7e8f9").as_deref(),
            Some("0a1b2c3d-4e5f-6071-8293-a4b5c6d7e8f9")
        );
        assert_eq!(boot_tag("boot.id"), None);
        assert_eq!(boot_tag("boot id"), None);
        assert_eq!(boot_tag(BOOT_UNKNOWN_TAG), None);
        assert_eq!(boot_tag(""), None);

        assert_eq!(
            machine_tag("0123456789abcdef0123456789abcdef").as_deref(),
            Some("0123456789abcdef0123456789abcdef")
        );
        assert_eq!(machine_tag("uninitialized"), None);
        assert_eq!(machine_tag("UNINITIALIZED"), None);
        assert_eq!(machine_tag(MACHINE_UNKNOWN_TAG), None);
        assert_eq!(machine_tag("machine.id"), None);
        assert_eq!(machine_tag(""), None);
    }

    #[test]
    fn process_is_alive_rejects_non_positive_pids() {
        assert!(!process_is_alive(0));
        assert!(!process_is_alive(-1));
        assert!(!process_is_alive(libc::pid_t::MIN));
        assert!(process_is_alive(std::process::id() as libc::pid_t));
        assert!(!process_is_alive(spawned_and_reaped_pid()));
    }

    #[test]
    fn atomic_write_refuses_output_symlinks() {
        let dir = tempfile::tempdir().unwrap();
        let target = dir.path().join("result.json");
        let link = dir.path().join("latest.json");
        std::fs::write(&target, b"old bytes\n").unwrap();
        std::os::unix::fs::symlink("result.json", &link).unwrap();

        let error = write_atomic(&link, b"new bytes\n").unwrap_err();
        assert_eq!(error.kind(), std::io::ErrorKind::InvalidInput);
        assert_eq!(std::fs::read(&target).unwrap(), b"old bytes\n");
        assert!(link.is_symlink());

        let missing = dir.path().join("missing.json");
        let dangling = dir.path().join("dangling.json");
        std::os::unix::fs::symlink("missing.json", &dangling).unwrap();
        let error = write_atomic(&dangling, b"new bytes\n").unwrap_err();
        assert_eq!(error.kind(), std::io::ErrorKind::InvalidInput);
        assert!(!missing.exists());
        assert!(dangling.is_symlink());
    }

    #[test]
    fn parent_dir_of_a_bare_file_name_is_the_current_directory() {
        assert_eq!(parent_dir(Path::new("result.json")), Path::new("."));
        assert_eq!(parent_dir(Path::new("./result.json")), Path::new("."));
        assert_eq!(parent_dir(Path::new("out/result.json")), Path::new("out"));
        assert_eq!(parent_dir(Path::new("/result.json")), Path::new("/"));
    }

    #[test]
    fn atomic_write_syncs_temp_then_renames_then_syncs_parent_dir() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("result.json");
        let trace = atomic_write_trace::Guard::start();

        write_atomic(&path, b"durable bytes\n").unwrap();

        assert_eq!(
            trace.take(),
            vec![
                AtomicWriteOp::SyncTemp,
                AtomicWriteOp::Rename,
                AtomicWriteOp::SyncParentDir(dir.path().to_path_buf()),
            ]
        );
        assert_eq!(std::fs::read(&path).unwrap(), b"durable bytes\n");
    }

    #[test]
    fn atomic_write_propagates_parent_dir_sync_errors_except_unsupported() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("result.json");

        for errno in [libc::EIO, libc::ENOSPC] {
            let _fault = dir_sync_fault_hook::Guard::inject(errno);
            let error = write_atomic(&path, b"bytes\n").unwrap_err();
            assert_eq!(error.raw_os_error(), Some(errno), "errno {errno}");
        }

        for errno in [libc::EINVAL, libc::ENOTSUP, libc::EBADF] {
            let _fault = dir_sync_fault_hook::Guard::inject(errno);
            write_atomic(&path, b"accepted\n").unwrap();
            assert_eq!(std::fs::read(&path).unwrap(), b"accepted\n");
        }
    }

    #[test]
    fn copy_atomic_checked_cleans_up_temp_when_verification_fails() {
        let _guard = atomic_test_guard();
        let dir = tempfile::tempdir().unwrap();
        let src = dir.path().join("source.bin");
        let dst = dir.path().join("dest.bin");
        std::fs::write(&src, b"payload\n").unwrap();

        let err = copy_atomic_checked(
            &src,
            &dst,
            |tmp| {
                assert_eq!(std::fs::read(tmp).unwrap(), b"payload\n");
                Err("checksum mismatch")
            },
            |_| "io error",
        )
        .unwrap_err();

        assert_eq!(err, "checksum mismatch");
        assert!(!dst.exists());
        assert!(atomic_temporary_siblings(&dst).is_empty());
    }

    #[test]
    fn split_quotes() {
        assert_eq!(
            split_args("-p batch --comment 'a b' \"\"").unwrap(),
            vec!["-p", "batch", "--comment", "a b", ""]
        );
        assert!(split_args("'open").is_err());
    }

    #[test]
    fn checked_duration_seconds_rejects_invalid_and_too_large_values() {
        assert_eq!(
            checked_duration_seconds(f64::NAN, None),
            Err(DurationError::Invalid)
        );
        assert_eq!(
            checked_duration_seconds(-0.1, Some(10.0)),
            Err(DurationError::Invalid)
        );
        assert_eq!(
            checked_duration_seconds(11.0, Some(10.0)),
            Err(DurationError::TooLarge)
        );
        assert_eq!(
            checked_duration_seconds(10.0, Some(10.0)),
            Err(DurationError::TooLarge)
        );
        assert_eq!(checked_duration_seconds(1.25, None), Ok(1.25));
        assert_eq!(
            checked_duration_seconds(18446744073709551616.0, Some(18446744073709551616.0)),
            Err(DurationError::TooLarge)
        );
    }

    #[test]
    fn a_quoted_word_reads_back_as_itself() {
        for a in [
            "--try-amend=configopts=-DA=OFF -DB=OFF",
            "it's",
            "a\"b'c",
            "",
            "--rebuild",
        ] {
            assert_eq!(split_args(&quote(a)).unwrap(), [a], "{}", quote(a));
            let o = std::process::Command::new("sh")
                .arg("-c")
                .arg(format!("printf %s {}", quote(a)))
                .output()
                .unwrap();
            assert_eq!(String::from_utf8_lossy(&o.stdout), a, "{}", quote(a));
        }
        assert_eq!(quote("--rebuild"), "--rebuild");
    }

    #[test]
    fn a_shell_quote_is_not_a_build_list_word() {
        // quote() is the shell quoter. An apostrophe is '"'"', which a shell
        // and split_args both read back, not the backslash form '\''.
        let shell = quote("it's");
        assert!(shell.contains("'\"'\"'"), "{shell}");
        assert!(!shell.contains("'\\''"), "{shell}");
        assert_eq!(split_args(&shell).unwrap(), ["it's"]);

        // A parenthesis is one bare word of a build list. The shell form
        // quotes it, so a reader that splits on white space and keeps the
        // quotes does not see the option.
        let hooks = "--hooks=/tmp/a(b).py";
        let quoted = quote(hooks);
        assert_ne!(quoted, hooks, "{quoted}");
        assert_ne!(quoted.split_whitespace().collect::<Vec<_>>(), [hooks]);
        assert_eq!(list_word(hooks).unwrap(), hooks);
        assert_eq!(
            list_word("--filter-deps=foo(bar)").unwrap(),
            "--filter-deps=foo(bar)"
        );
        assert_eq!(list_word("--opt=a*b").unwrap(), "--opt=a*b");

        for word in [
            "it's",
            "a b",
            "say \"hi\"",
            "it's \"a\"",
            "",
            "#hash",
            "--hooks=/tmp/a(b).py",
            "--opt=a*b",
            "--filter-deps=foo(bar)",
            "  ",
        ] {
            let listed = list_word(word).unwrap();
            let lines = parse_list(&format!("x.eb {listed}")).unwrap();
            assert_eq!(
                lines[0].args,
                vec![word.to_string()],
                "{word:?} encoded as {listed}"
            );
        }
        assert_eq!(list_word("it's").unwrap(), "\"it's\"");
        assert_eq!(list_word("a b").unwrap(), "'a b'");
        assert!(list_word("a\nb").is_err());
        assert!(list_word("a\rb").is_err());
    }

    #[test]
    fn versions() {
        assert_eq!(find_version("eb-stack 0.7.4 (abc)"), Some(vec![0, 7, 4]));
        assert_eq!(find_version("tool v1.2"), Some(vec![1, 2]));
        assert_eq!(cmp_version(&[1, 2], &[1, 2, 0]), std::cmp::Ordering::Equal);
        assert_eq!(cmp_version(&[0, 7, 4], &[0, 10]), std::cmp::Ordering::Less);
    }

    #[test]
    fn package_names() {
        assert_eq!(
            package_of("GROMACS-2025.2-foss-2025a.eb"),
            ("GROMACS".into(), Some("2025.2".into()))
        );
        assert_eq!(
            package_of("/x/y/R-bundle-Bioconductor-3.20-foss-2025a.eb"),
            ("R-bundle-Bioconductor".into(), Some("3.20".into()))
        );
    }

    #[test]
    fn list_lines() {
        let l = parse_list("# c\n\nFoo-1.0-foss-2025a.eb --rebuild\n").unwrap();
        assert_eq!(l.len(), 1);
        assert_eq!(l[0].args, vec!["--rebuild"]);
    }

    #[test]
    fn parse_list_strips_one_initial_utf8_bom() {
        let lines = parse_list("\u{feff}Foo-1.0.eb --rebuild\nBar-2.0.eb\n").unwrap();
        assert_eq!(lines[0].easyconfig, "Foo-1.0.eb");
        assert_eq!(lines[0].raw, "Foo-1.0.eb --rebuild");
        assert_eq!(lines[1].easyconfig, "Bar-2.0.eb");
    }

    #[test]
    fn log_stamps() {
        for ok in ["20261116_024713", "20240229_235959", "19700101_000000"] {
            assert!(is_log_stamp(ok), "{ok}");
        }
        for bad in [
            "",
            "2026-11-16",
            "20261116",
            "20261116_02471",
            "20261116-024713",
            "20261116_0247130",
            "20261301_000000",
            "20250229_000000",
            "20261116_240000",
            "20261116_006000",
            "+2026116_024713",
            " 2026116_024713",
            "../20261116_0247",
        ] {
            assert!(!is_log_stamp(bad), "{bad:?}");
        }
    }
}
