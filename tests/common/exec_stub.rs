use std::ffi::OsStr;
use std::os::unix::ffi::OsStrExt;
use std::os::unix::fs::PermissionsExt;
use std::path::Path;
use std::process::{Child, Command, ExitStatus, Output, Stdio};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{RwLock, RwLockReadGuard};
use std::time::Duration;

static EXEC_FORK_LOCK: RwLock<()> = RwLock::new(());

fn reader_guard() -> RwLockReadGuard<'static, ()> {
    EXEC_FORK_LOCK.read().unwrap_or_else(|e| e.into_inner())
}

const WRITER: &str = r#"out=$1; shift; set -C; : > "$out" || exit 1; set +C; for chunk; do printf '%s' "$chunk" >> "$out" || exit 1; done"#;
const CHUNK: usize = 32 * 1024;

/// Installs `text` as an executable at `path`.
///
/// The bytes are written by a short-lived `sh` child, so this process never
/// holds a writable descriptor for the stub. A concurrent fork in another test
/// thread therefore cannot inherit one, and a later exec of the stub cannot
/// fail with ETXTBSY because of it.
pub fn write_exec(path: &Path, text: &str) {
    assert!(
        !text.contains('\0'),
        "an argv chunk cannot carry a NUL byte"
    );
    static NEXT: AtomicU64 = AtomicU64::new(0);
    let parent = path.parent().unwrap_or_else(|| Path::new("."));
    let stem = path
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or("stub");
    let temp = parent.join(format!(
        ".{stem}.{}.{}.tmp",
        std::process::id(),
        NEXT.fetch_add(1, Ordering::Relaxed)
    ));
    let mut writer = Command::new("/bin/sh");
    writer.args(["-c", WRITER, "stub-writer"]).arg(&temp);
    for chunk in text.as_bytes().chunks(CHUNK) {
        writer.arg(OsStr::from_bytes(chunk));
    }
    let output = writer
        .stdin(Stdio::null())
        .output()
        .unwrap_or_else(|error| panic!("cannot start the stub writer: {error}"));
    if !output.status.success() {
        let _ = std::fs::remove_file(&temp);
        panic!(
            "cannot write {}: {}\n{}",
            temp.display(),
            output.status,
            String::from_utf8_lossy(&output.stderr)
        );
    }
    let installed = std::fs::set_permissions(&temp, std::fs::Permissions::from_mode(0o755))
        .and_then(|()| std::fs::rename(&temp, path));
    if let Err(error) = installed {
        let _ = std::fs::remove_file(&temp);
        panic!("cannot install {}: {error}", path.display());
    }
}

fn retry_etxtbsy<T>(mut run: impl FnMut() -> std::io::Result<T>) -> std::io::Result<T> {
    const ATTEMPTS: usize = 8;
    const BASE_MS: u64 = 5;
    let mut last_error: Option<std::io::Error> = None;
    for attempt in 0..ATTEMPTS {
        let _guard = reader_guard();
        match run() {
            Ok(value) => return Ok(value),
            Err(error) if error.raw_os_error() == Some(26) && attempt + 1 < ATTEMPTS => {
                last_error = Some(error);
            }
            Err(error) => return Err(error),
        }
        std::thread::sleep(Duration::from_millis(BASE_MS * (1 << attempt)));
    }
    Err(last_error.unwrap_or_else(|| std::io::Error::other("cannot run command")))
}

pub trait CommandSyncExt {
    #[allow(dead_code)]
    fn sync_spawn(&mut self) -> Child;
    fn sync_output(&mut self) -> Output;
    #[allow(dead_code)]
    fn sync_status(&mut self) -> ExitStatus;
}

impl CommandSyncExt for Command {
    fn sync_spawn(&mut self) -> Child {
        retry_etxtbsy(|| self.spawn()).unwrap()
    }

    fn sync_output(&mut self) -> Output {
        retry_etxtbsy(|| self.output()).unwrap()
    }

    fn sync_status(&mut self) -> ExitStatus {
        retry_etxtbsy(|| self.status()).unwrap()
    }
}
