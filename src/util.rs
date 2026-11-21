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

/// Resolve a path as far as the filesystem permits.
pub fn resolved_path(path: &Path) -> PathBuf {
    if let Ok(path) = std::fs::canonicalize(path) {
        return path;
    }
    let parent = path.parent().unwrap_or_else(|| Path::new("."));
    let parent = if parent.as_os_str().is_empty() {
        Path::new(".")
    } else {
        parent
    };
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
    if path.is_symlink() {
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidInput,
            "output path is a symbolic link",
        ));
    }
    let destination = path.to_path_buf();
    let parent = destination.parent().unwrap_or_else(|| Path::new("."));
    let existing_mode = match std::fs::metadata(&destination) {
        Ok(metadata) if metadata.is_file() => Some(metadata.permissions().mode() & 0o7777),
        Ok(_) => None,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => None,
        Err(error) => return Err(error),
    };
    let name = destination
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or("output");
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
            Err(error) => return Err(error),
        };
        #[cfg(test)]
        maybe_kill_atomic_writer("after-temp-created");
        if let Some(mode) = existing_mode {
            if let Err(error) =
                std::fs::set_permissions(&temporary, std::fs::Permissions::from_mode(mode))
            {
                drop(file);
                let _ = std::fs::remove_file(&temporary);
                return Err(error);
            }
        }
        let written = write_atomic_bytes(&mut file, bytes).and_then(|()| file.sync_all());
        drop(file);
        if let Err(error) = written {
            let _ = std::fs::remove_file(&temporary);
            return Err(error);
        }
        #[cfg(test)]
        maybe_kill_atomic_writer("before-rename");
        if let Err(error) = std::fs::rename(&temporary, &destination) {
            let _ = std::fs::remove_file(&temporary);
            return Err(error);
        }
        #[cfg(test)]
        maybe_kill_atomic_writer("after-rename-before-dir-sync");
        return Ok(());
    }
    Err(std::io::Error::new(
        std::io::ErrorKind::AlreadyExists,
        "cannot allocate a temporary output file",
    ))
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
        // SAFETY: test hook only; intentionally terminates the process.
        unsafe {
            libc::raise(libc::SIGKILL);
        }
    }
}

#[cfg(test)]
fn kill_stage_matches(stage: &str) -> bool {
    std::env::var_os("EB_PIPELINE_TEST_KILL_WRITE_ATOMIC").as_deref() == Some(stage.as_ref())
}

#[derive(Clone, Debug)]
struct WriterIdentity {
    host: String,
    host_reap_safe: bool,
    boot_id: Option<String>,
}

const HOST_UNKNOWN_TAG: &str = "host-unknown";
const BOOT_UNKNOWN_TAG: &str = "boot-unknown";

struct SanitizedTag {
    value: String,
    used_fallback: bool,
}

#[cfg(test)]
mod writer_identity_test_hook {
    use super::WriterIdentity;
    use std::sync::{Mutex, OnceLock};

    static OVERRIDE: OnceLock<Mutex<Option<WriterIdentity>>> = OnceLock::new();

    pub(super) fn get() -> Option<WriterIdentity> {
        OVERRIDE
            .get()
            .and_then(|slot| slot.lock().ok().and_then(|value| value.clone()))
    }

    pub(super) fn install(identity: WriterIdentity) {
        *OVERRIDE
            .get_or_init(|| Mutex::new(None))
            .lock()
            .expect("writer identity override mutex poisoned") = Some(identity);
    }

    pub(super) fn clear() {
        if let Some(slot) = OVERRIDE.get() {
            *slot
                .lock()
                .expect("writer identity override mutex poisoned") = None;
        }
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
    // Host from HOSTNAME or /proc hostname; boot id from Linux procfs.
    let raw_host = std::env::var("HOSTNAME")
        .ok()
        .or_else(|| std::fs::read_to_string("/proc/sys/kernel/hostname").ok())
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty());
    let (host, host_reap_safe) = host_tag_and_reap_safety(raw_host.as_deref());
    // /proc/sys/kernel/random/boot_id names one kernel boot instance.
    let boot_id = std::fs::read_to_string("/proc/sys/kernel/random/boot_id")
        .ok()
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty());
    WriterIdentity {
        host,
        host_reap_safe,
        boot_id: boot_id.map(|boot| sanitize_tag_component(&boot).value),
    }
}

fn host_tag_and_reap_safety(raw_host: Option<&str>) -> (String, bool) {
    let Some(raw_host) = raw_host else {
        return (HOST_UNKNOWN_TAG.into(), false);
    };
    if raw_host.is_empty() {
        return (HOST_UNKNOWN_TAG.into(), false);
    }
    let sanitized = sanitize_tag_component(raw_host);
    if raw_host.eq_ignore_ascii_case("localhost")
        || sanitized.value.eq_ignore_ascii_case("localhost")
        || sanitized.used_fallback
    {
        return (sanitized.value, false);
    }
    (sanitized.value, true)
}

fn atomic_temp_file_name(
    name: &str,
    identity: &WriterIdentity,
    pid: libc::pid_t,
    attempt: u32,
) -> String {
    let boot = identity.boot_id.as_deref().unwrap_or(BOOT_UNKNOWN_TAG);
    format!(".{name}.{}.{}.{pid}.{attempt}.tmp", identity.host, boot)
}

fn sanitize_tag_component(text: &str) -> SanitizedTag {
    let mut out = String::with_capacity(text.len());
    let mut used_fallback = false;
    for ch in text.chars() {
        if ch.is_ascii_alphanumeric() || matches!(ch, '-' | '_') {
            out.push(ch);
        } else {
            out.push('_');
            used_fallback = true;
        }
    }
    if out.is_empty() {
        SanitizedTag {
            value: HOST_UNKNOWN_TAG.into(),
            used_fallback: true,
        }
    } else if used_fallback {
        // A sanitized host tag is not unique enough to authorize reaping.
        SanitizedTag {
            value: HOST_UNKNOWN_TAG.into(),
            used_fallback: true,
        }
    } else {
        SanitizedTag {
            value: out,
            used_fallback: false,
        }
    }
}

fn remove_stale_atomic_temporary_files(parent: &Path, name: &str, local_identity: &WriterIdentity) {
    if !local_identity.host_reap_safe {
        return;
    }
    // Without a boot id, this process cannot tell current-boot temps from
    // previous-boot temps on the same host, so it must not reap.
    let Some(local_boot) = local_identity.boot_id.as_deref() else {
        return;
    };
    let entries = match std::fs::read_dir(parent) {
        Ok(entries) => entries,
        Err(_) => return,
    };
    for entry in entries.filter_map(Result::ok) {
        let shown = entry.file_name().to_string_lossy().to_string();
        let Some((host, boot, pid, _attempt)) = parse_atomic_temporary_name(&shown, name) else {
            continue;
        };
        if host != local_identity.host
            || host == HOST_UNKNOWN_TAG
            || host.eq_ignore_ascii_case("localhost")
            || boot == BOOT_UNKNOWN_TAG
        {
            continue;
        }
        if boot != local_boot || !process_is_alive(pid) {
            let _ = std::fs::remove_file(entry.path());
        }
    }
}

fn parse_atomic_temporary_name(
    file_name: &str,
    output_name: &str,
) -> Option<(String, String, libc::pid_t, u32)> {
    let prefix = format!(".{output_name}.");
    if file_name.len() < prefix.len() + ".tmp".len() {
        return None;
    }
    let middle = file_name
        .strip_prefix(prefix.as_str())?
        .strip_suffix(".tmp")?;
    let mut fields = middle.split('.');
    let host = fields.next()?.to_string();
    let boot = fields.next()?.to_string();
    let pid = fields
        .next()?
        .parse::<libc::pid_t>()
        .ok()
        .filter(|pid| *pid > 0)?;
    let attempt = fields.next()?.parse::<u32>().ok()?;
    if host.is_empty() || boot.is_empty() || fields.next().is_some() {
        return None;
    }
    Some((host, boot, pid, attempt))
}

fn process_is_alive(pid: libc::pid_t) -> bool {
    // SAFETY: signal 0 checks whether a process exists without signalling it.
    let rc = unsafe { libc::kill(pid, 0) };
    if rc == 0 {
        return true;
    }
    std::io::Error::last_os_error().raw_os_error() == Some(libc::EPERM)
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
mod tests {
    use super::*;
    use std::ffi::CString;
    use std::os::unix::ffi::OsStrExt;
    use std::os::unix::process::ExitStatusExt;
    use std::process::Command;
    use std::sync::{Mutex, MutexGuard, OnceLock};

    fn atomic_test_guard() -> MutexGuard<'static, ()> {
        static GUARD: OnceLock<Mutex<()>> = OnceLock::new();
        GUARD
            .get_or_init(|| Mutex::new(()))
            .lock()
            .expect("atomic test guard mutex poisoned")
    }

    struct IdentityOverrideGuard;

    impl Drop for IdentityOverrideGuard {
        fn drop(&mut self) {
            writer_identity_test_hook::clear();
        }
    }

    fn install_identity_override(identity: WriterIdentity) -> IdentityOverrideGuard {
        writer_identity_test_hook::install(identity);
        IdentityOverrideGuard
    }

    const REAP_SAFE_TEST_HOST: &str = "atomic-test-host";

    /// A writer identity that may reap, whatever `HOSTNAME` the host has.
    /// `None` when the kernel offers no boot id to pair with the host tag.
    fn reap_safe_test_identity() -> Option<WriterIdentity> {
        let boot_id = detect_writer_identity().boot_id?;
        assert_eq!(
            host_tag_and_reap_safety(Some(REAP_SAFE_TEST_HOST)),
            (REAP_SAFE_TEST_HOST.to_string(), true)
        );
        Some(WriterIdentity {
            host: REAP_SAFE_TEST_HOST.into(),
            host_reap_safe: true,
            boot_id: Some(boot_id),
        })
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
        let _guard = atomic_test_guard();
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
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("result.json");
        std::fs::write(&path, b"old bytes\n").unwrap();

        let local = local_writer_identity();
        let foreign_pid = spawned_and_reaped_pid();
        let foreign = dir.path().join(format!(
            ".result.json.{}.{}.{}.{}.tmp",
            "other-host",
            local.boot_id.clone().unwrap_or_else(|| "other-boot".into()),
            foreign_pid,
            1,
        ));
        std::fs::write(&foreign, b"foreign temp\n").unwrap();
        let status = crash_write_atomic(&path, "after-temp-created", b"new bytes\n");
        assert_eq!(status.signal(), Some(libc::SIGKILL), "{status:?}");
        let locals_before = temporary_atomic_siblings(&path)
            .into_iter()
            .filter(|candidate| candidate != &foreign)
            .count();
        assert!(
            locals_before >= 1,
            "no local temp was left by killed writer"
        );

        write_atomic(&path, b"new bytes\n").unwrap();

        assert!(foreign.exists(), "foreign temp unexpectedly removed");
        let locals_after = temporary_atomic_siblings(&path)
            .into_iter()
            .filter(|candidate| candidate != &foreign)
            .count();
        assert_eq!(locals_after, 0, "local stale temp was not removed");
        assert_eq!(std::fs::read(&path).unwrap(), b"new bytes\n");
    }

    #[test]
    fn atomic_write_reaps_same_host_temps_from_earlier_boot() {
        let _guard = atomic_test_guard();
        let local = local_writer_identity();
        if !local.host_reap_safe {
            return;
        }
        let Some(local_boot) = local.boot_id.clone() else {
            return;
        };
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("result.json");
        std::fs::write(&path, b"old bytes\n").unwrap();
        let earlier_boot = if local_boot == "earlier-boot" {
            "different-boot".to_string()
        } else {
            "earlier-boot".to_string()
        };
        let stale = dir.path().join(format!(
            ".result.json.{}.{}.{}.{}.tmp",
            local.host,
            earlier_boot,
            std::process::id(),
            1
        ));
        std::fs::write(&stale, b"earlier boot temp\n").unwrap();

        write_atomic(&path, b"new bytes\n").unwrap();

        assert!(!stale.exists(), "earlier-boot temp was not removed");
        assert_eq!(std::fs::read(&path).unwrap(), b"new bytes\n");
    }

    #[test]
    fn atomic_write_does_not_reap_when_boot_id_is_unavailable() {
        let _guard = atomic_test_guard();
        let local = local_writer_identity();
        let _override = install_identity_override(WriterIdentity {
            host: local.host.clone(),
            host_reap_safe: local.host_reap_safe,
            boot_id: None,
        });
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("result.json");
        std::fs::write(&path, b"old bytes\n").unwrap();
        let stale_pid = spawned_and_reaped_pid();
        let stale = dir.path().join(format!(
            ".result.json.{}.earlier-boot.{}.1.tmp",
            local.host, stale_pid
        ));
        std::fs::write(&stale, b"stale temp\n").unwrap();

        write_atomic(&path, b"new bytes\n").unwrap();

        assert!(stale.exists(), "temp was reaped without local boot id");
        assert_eq!(std::fs::read(&path).unwrap(), b"new bytes\n");
    }

    #[test]
    fn atomic_write_does_not_reap_when_host_tag_is_not_unique() {
        let _guard = atomic_test_guard();
        let raw_cases = [None, Some(""), Some("localhost"), Some("node.example")];
        for raw in raw_cases {
            let (host, host_reap_safe) = host_tag_and_reap_safety(raw);
            assert!(!host_reap_safe, "{raw:?} unexpectedly reaped");
            let _override = install_identity_override(WriterIdentity {
                host: host.clone(),
                host_reap_safe,
                boot_id: Some("boot-1".into()),
            });
            let dir = tempfile::tempdir().unwrap();
            let path = dir.path().join("result.json");
            std::fs::write(&path, b"old bytes\n").unwrap();
            let stale_pid = spawned_and_reaped_pid();
            let stale = dir.path().join(format!(
                ".result.json.{}.earlier-boot.{}.1.tmp",
                host, stale_pid
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
        let local = local_writer_identity();
        if !local.host_reap_safe {
            return;
        }
        let Some(local_boot) = local.boot_id.clone() else {
            return;
        };
        let mut child = Command::new("sh")
            .arg("-c")
            .arg("sleep 30")
            .spawn()
            .unwrap();
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("result.json");
        std::fs::write(&path, b"old bytes\n").unwrap();
        let live_temp = dir.path().join(format!(
            ".result.json.{}.{}.{}.1.tmp",
            local.host,
            local_boot,
            child.id(),
        ));
        std::fs::write(&live_temp, b"live temp\n").unwrap();

        write_atomic(&path, b"new bytes\n").unwrap();

        assert!(live_temp.exists(), "temp from a live pid was reaped");
        assert_eq!(std::fs::read(&path).unwrap(), b"new bytes\n");
        // SAFETY: this test started the child process.
        unsafe {
            libc::kill(child.id() as libc::pid_t, libc::SIGKILL);
        }
        let _ = child.wait();
    }

    #[test]
    fn atomic_write_recovers_after_kills_at_write_steps() {
        let _guard = atomic_test_guard();
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
            assert!(
                seen == b"old bytes\n" || seen == new_bytes,
                "{stage}: saw {:?}",
                String::from_utf8_lossy(&seen)
            );

            write_atomic(&path, new_bytes).unwrap();
            assert_eq!(std::fs::read(&path).unwrap(), new_bytes, "{stage}");
            assert!(
                temporary_atomic_siblings(&path).is_empty(),
                "{stage}: stale local temp files remained: {:?}",
                temporary_atomic_siblings(&path)
            );
        }
    }

    #[test]
    fn atomic_temporary_name_parser_accepts_only_complete_tagged_names() {
        let parsed =
            parse_atomic_temporary_name(".result.json.node-1.boot-1.42.7.tmp", "result.json");
        assert_eq!(
            parsed,
            Some(("node-1".to_string(), "boot-1".to_string(), 42, 7))
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
            (".result.json.node-1.boot-1.42.tmp", "result.json"),
            (".result.json.node-1.boot-1.42.7.8.tmp", "result.json"),
            (".result.json..boot-1.42.7.tmp", "result.json"),
            (".result.json.node-1..42.7.tmp", "result.json"),
            (".result.json.node-1.boot-1.0.7.tmp", "result.json"),
            (".result.json.node-1.boot-1.-1.7.tmp", "result.json"),
            (".result.json.node-1.boot-1.42.7.tmp", "other.json"),
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
    fn atomic_write_ignores_siblings_with_short_or_malformed_temp_names() {
        let _guard = atomic_test_guard();
        let Some(identity) = reap_safe_test_identity() else {
            return;
        };
        let _override = install_identity_override(identity);
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("result.json");
        std::fs::write(&path, b"old bytes\n").unwrap();
        let siblings = [
            ".result.json.tmp",
            ".result.json..tmp",
            ".result.json.tmp.tmp",
            ".result.json.1.2.tmp",
            ".result.json.atomic-test-host.boot.0.1.tmp",
            ".result.json.atomic-test-host.boot.-1.1.tmp",
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
    fn atomic_write_refuses_output_symlinks() {
        let _guard = atomic_test_guard();
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
    fn atomic_write_crash_helper() {
        if std::env::var_os("EB_PIPELINE_TEST_ATOMIC_HELPER").is_none() {
            return;
        }
        let path = PathBuf::from(std::env::var("EB_PIPELINE_TEST_ATOMIC_PATH").unwrap());
        let stage = std::env::var("EB_PIPELINE_TEST_ATOMIC_STAGE").unwrap();
        let bytes = std::env::var("EB_PIPELINE_TEST_ATOMIC_BYTES").unwrap();
        std::env::set_var("EB_PIPELINE_TEST_KILL_WRITE_ATOMIC", stage);
        let _ = write_atomic(&path, bytes.as_bytes());
    }

    fn crash_write_atomic(path: &Path, stage: &str, bytes: &[u8]) -> std::process::ExitStatus {
        let output = Command::new(std::env::current_exe().unwrap())
            .arg("--exact")
            .arg("util::tests::atomic_write_crash_helper")
            .env("EB_PIPELINE_TEST_ATOMIC_HELPER", "1")
            .env("EB_PIPELINE_TEST_ATOMIC_PATH", path)
            .env(
                "EB_PIPELINE_TEST_ATOMIC_BYTES",
                String::from_utf8_lossy(bytes).into_owned(),
            )
            .env("EB_PIPELINE_TEST_ATOMIC_STAGE", stage)
            .output()
            .unwrap();
        output.status
    }

    fn spawned_and_reaped_pid() -> libc::pid_t {
        let mut child = Command::new("sh").arg("-c").arg("exit 0").spawn().unwrap();
        let pid = child.id() as libc::pid_t;
        let _ = child.wait();
        pid
    }

    fn temporary_atomic_siblings(path: &Path) -> Vec<PathBuf> {
        let parent = path.parent().unwrap_or_else(|| Path::new("."));
        let output_name = path.file_name().unwrap().to_string_lossy().into_owned();
        let mut found = std::fs::read_dir(parent)
            .unwrap()
            .filter_map(Result::ok)
            .map(|entry| entry.path())
            .filter(|candidate| {
                candidate
                    .file_name()
                    .and_then(|file_name| file_name.to_str())
                    .is_some_and(|shown| {
                        shown.starts_with(&format!(".{output_name}.")) && shown.ends_with(".tmp")
                    })
            })
            .collect::<Vec<_>>();
        found.sort();
        found
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
