use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::{Mutex, OnceLock};
use std::time::{Duration, Instant};

fn test_guard() -> std::sync::MutexGuard<'static, ()> {
    static GUARD: OnceLock<Mutex<()>> = OnceLock::new();
    GUARD
        .get_or_init(|| Mutex::new(()))
        .lock()
        .expect("atomic temp cleanup guard poisoned")
}

#[test]
fn atomic_temp_cleanup_helper() {
    let guard = test_guard();
    if std::env::var_os("EB_PIPELINE_TEST_ATOMIC_HELPER").is_none() {
        return;
    }
    drop(guard);
    let path = PathBuf::from(std::env::var("EB_PIPELINE_TEST_ATOMIC_PATH").unwrap());
    let bytes_len: usize = std::env::var("EB_PIPELINE_TEST_ATOMIC_BYTES")
        .unwrap()
        .parse()
        .unwrap();
    let bytes = vec![b'x'; bytes_len];
    loop {
        let _ = eb_pipeline::util::write_atomic(&path, &bytes);
    }
}

#[test]
fn write_atomic_reaps_only_local_stale_temps() {
    let _guard = test_guard();
    std::env::set_var("HOSTNAME", "atomic-test-host");
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("result.json");
    std::fs::write(&path, b"old bytes\n").unwrap();

    // The helper can be killed in the instant between one rename and the next
    // temp file, leaving nothing behind; try again rather than flake.
    let local_temp = (0..10)
        .find_map(|_| {
            let (mut helper, observed_temp) = spawn_helper_and_capture_temp(&path);
            // SAFETY: this test started the helper process.
            unsafe {
                libc::kill(helper.id() as libc::pid_t, libc::SIGKILL);
            }
            let _ = helper.wait();
            if observed_temp.exists() {
                Some(observed_temp)
            } else {
                temporary_atomic_siblings(&path).into_iter().next()
            }
        })
        .expect("no stale temp remained after killing helper");
    let output_name = path.file_name().unwrap().to_string_lossy().into_owned();
    let foreign = match parse_atomic_temporary_name(&local_temp, &output_name) {
        Some(AtomicTempName::Tagged { boot, .. }) => {
            let foreign_pid = spawned_and_reaped_pid();
            let foreign = dir.path().join(format!(
                ".result.json.{}.{}.{}.1.tmp",
                "other-host", boot, foreign_pid
            ));
            std::fs::write(&foreign, b"foreign temp\n").unwrap();
            Some(foreign)
        }
        Some(AtomicTempName::Legacy) => None,
        None => panic!("bad temp name"),
    };

    eb_pipeline::util::write_atomic(&path, b"new bytes\n").unwrap();

    assert!(!local_temp.exists(), "local stale temp was not removed");
    if let Some(foreign) = foreign {
        assert!(foreign.exists(), "foreign temp should stay untouched");
    }
    assert_eq!(std::fs::read(&path).unwrap(), b"new bytes\n");
}

#[test]
fn temp_name_helper_rejects_short_and_overlapping_names() {
    for (file_name, output_name) in [
        (".cpu.json.tmp", "cpu.json"),
        (".result.json.tmp", "result.json"),
        (".result.json.", "result.json"),
        (".result.json", "result.json"),
        (".tmp", ""),
        ("..tmp", ""),
    ] {
        assert!(
            parse_atomic_temporary_name(Path::new(file_name), output_name).is_none(),
            "{file_name:?} for {output_name:?}"
        );
    }
    assert!(matches!(
        parse_atomic_temporary_name(Path::new(".result.json.node.boot-1.42.1.tmp"), "result.json"),
        Some(AtomicTempName::Tagged { boot }) if boot == "boot-1"
    ));
    assert!(matches!(
        parse_atomic_temporary_name(Path::new(".result.json.42.1.tmp"), "result.json"),
        Some(AtomicTempName::Legacy)
    ));
}

#[test]
fn write_atomic_ignores_a_sibling_named_like_a_bare_temp_file() {
    let _guard = test_guard();
    std::env::set_var("HOSTNAME", "atomic-test-host");
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("result.json");
    std::fs::write(&path, b"old bytes\n").unwrap();
    let sibling = dir.path().join(".result.json.tmp");
    std::fs::write(&sibling, b"unrelated\n").unwrap();

    eb_pipeline::util::write_atomic(&path, b"new bytes\n").unwrap();

    assert_eq!(std::fs::read(&path).unwrap(), b"new bytes\n");
    assert_eq!(std::fs::read(&sibling).unwrap(), b"unrelated\n");
}

fn spawn_helper_and_capture_temp(path: &Path) -> (std::process::Child, PathBuf) {
    let mut helper = Command::new(std::env::current_exe().unwrap())
        .arg("--exact")
        .arg("atomic_temp_cleanup_helper")
        .env("EB_PIPELINE_TEST_ATOMIC_HELPER", "1")
        .env("HOSTNAME", "atomic-test-host")
        .env("EB_PIPELINE_TEST_ATOMIC_PATH", path)
        .env(
            "EB_PIPELINE_TEST_ATOMIC_BYTES",
            (16 * 1024 * 1024).to_string(),
        )
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .unwrap();
    let t0 = Instant::now();
    loop {
        let candidates = temporary_atomic_siblings(path);
        if let Some(candidate) = candidates.into_iter().next() {
            return (helper, candidate);
        }
        assert!(
            t0.elapsed() < Duration::from_secs(20),
            "helper never produced an atomic temp file"
        );
        assert!(
            helper.try_wait().unwrap().is_none(),
            "helper exited before producing an atomic temp file"
        );
        std::thread::yield_now();
    }
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
                .and_then(|name| name.to_str())
                .is_some_and(|shown| {
                    shown.starts_with(&format!(".{output_name}.")) && shown.ends_with(".tmp")
                })
        })
        .collect::<Vec<_>>();
    found.sort();
    found
}

enum AtomicTempName {
    Tagged { boot: String },
    Legacy,
}

fn parse_atomic_temporary_name(path: &Path, output_name: &str) -> Option<AtomicTempName> {
    let file_name = path.file_name()?.to_str()?;
    let prefix = format!(".{output_name}.");
    if file_name.len() < prefix.len() + ".tmp".len() {
        return None;
    }
    let middle = file_name
        .strip_prefix(prefix.as_str())?
        .strip_suffix(".tmp")?;
    let fields: Vec<&str> = middle.split('.').collect();
    match fields.as_slice() {
        [host, boot, pid, attempt]
            if !host.is_empty()
                && !boot.is_empty()
                && pid.parse::<libc::pid_t>().is_ok()
                && attempt.parse::<u32>().is_ok() =>
        {
            Some(AtomicTempName::Tagged {
                boot: (*boot).to_string(),
            })
        }
        [pid, attempt] if pid.parse::<libc::pid_t>().is_ok() && attempt.parse::<u32>().is_ok() => {
            Some(AtomicTempName::Legacy)
        }
        _ => None,
    }
}

fn spawned_and_reaped_pid() -> libc::pid_t {
    let mut child = Command::new("sh").arg("-c").arg("exit 0").spawn().unwrap();
    let pid = child.id() as libc::pid_t;
    let _ = child.wait();
    pid
}
