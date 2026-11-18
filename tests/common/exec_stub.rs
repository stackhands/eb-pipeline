use std::fs::OpenOptions;
use std::io::Write;
use std::os::unix::fs::PermissionsExt;
use std::path::Path;
use std::process::{Child, Command, ExitStatus, Output};
use std::sync::{RwLock, RwLockReadGuard, RwLockWriteGuard};
use std::time::Duration;

static EXEC_FORK_LOCK: RwLock<()> = RwLock::new(());

fn writer_guard() -> RwLockWriteGuard<'static, ()> {
    EXEC_FORK_LOCK.write().unwrap_or_else(|e| e.into_inner())
}

fn reader_guard() -> RwLockReadGuard<'static, ()> {
    EXEC_FORK_LOCK.read().unwrap_or_else(|e| e.into_inner())
}

pub fn write_exec(path: &Path, text: &str) {
    let _guard = writer_guard();
    let parent = path.parent().unwrap_or_else(|| Path::new("."));
    let stem = path
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or("stub");
    for attempt in 0..1000u32 {
        let temp = parent.join(format!(".{stem}.{}.{}.tmp", std::process::id(), attempt));
        let mut file = match OpenOptions::new().write(true).create_new(true).open(&temp) {
            Ok(file) => file,
            Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => continue,
            Err(error) => panic!("cannot create {}: {error}", temp.display()),
        };
        if let Err(error) = std::fs::set_permissions(&temp, std::fs::Permissions::from_mode(0o755))
        {
            drop(file);
            let _ = std::fs::remove_file(&temp);
            panic!("cannot chmod {}: {error}", temp.display());
        }
        let written = file
            .write_all(text.as_bytes())
            .and_then(|()| file.sync_all());
        drop(file);
        if let Err(error) = written {
            let _ = std::fs::remove_file(&temp);
            panic!("cannot write {}: {error}", temp.display());
        }
        if let Err(error) = std::fs::rename(&temp, path) {
            let _ = std::fs::remove_file(&temp);
            panic!(
                "cannot move {} to {}: {error}",
                temp.display(),
                path.display()
            );
        }
        return;
    }
    panic!(
        "cannot allocate temporary executable near {}",
        path.display()
    );
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
