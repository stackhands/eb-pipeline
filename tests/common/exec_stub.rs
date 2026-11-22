use std::ffi::OsStr;
use std::os::unix::ffi::OsStrExt;
use std::os::unix::fs::PermissionsExt;
use std::path::Path;
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicU64, Ordering};

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
