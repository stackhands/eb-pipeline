//! After a clean build, ask eb-stack for the CycloneDX document of a lock
//! and keep it beside `summary.json`. eb-stack reads the lock and writes
//! the document; this module only starts that command.

use crate::exit::{self, Fail};
use crate::schema::{Sbom, Site};
use crate::tools::Tool;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

/// The CycloneDX file in the run directory, beside `summary.json`.
pub const FILE: &str = "sbom.cdx.json";

/// Run `eb-stack stack sbom --lock LOCK --out RUN_DIR/sbom.cdx.json`.
///
/// A non-zero status removes a partial file. The command's output is
/// `sbom.log` in the same directory. The failure ends with [`exit::FAILED`],
/// or with 130 or 143 when the command ended that way, whatever other status
/// the command returned.
pub fn store(site: &Site, sbom: &Sbom, run_dir: &Path) -> Result<PathBuf, Fail> {
    let lock = sbom
        .lock
        .as_deref()
        .filter(|p| !p.is_empty())
        .ok_or_else(|| Fail::usage("sbom.lock: must name the lock file when sbom is enabled"))?;
    std::fs::create_dir_all(run_dir).map_err(|e| {
        Fail::new(
            exit::FAILED,
            format!("cannot create {}: {e}", run_dir.display()),
        )
    })?;
    let out = run_dir.join(FILE);
    if let Err(error) = std::fs::remove_file(&out) {
        if error.kind() != std::io::ErrorKind::NotFound {
            return Err(Fail::new(
                exit::FAILED,
                format!("cannot replace {}: {error}", out.display()),
            ));
        }
    }
    let log_path = run_dir.join("sbom.log");
    let log = std::fs::File::create(&log_path).map_err(|e| {
        Fail::new(
            exit::FAILED,
            format!("cannot write {}: {e}", log_path.display()),
        )
    })?;
    let err = log.try_clone().map_err(|e| {
        Fail::new(
            exit::FAILED,
            format!("cannot write {}: {e}", log_path.display()),
        )
    })?;
    let bin = Tool::EbStack.bin(site);
    let status = Command::new(&bin)
        .args(["stack", "sbom", "--lock", lock, "--out"])
        .arg(&out)
        .stdin(Stdio::null())
        .stdout(log)
        .stderr(err)
        .status()
        .map_err(|e| {
            Fail::new(
                exit::FAILED,
                format!("cannot run `{bin} stack sbom --lock {lock}`: {e}"),
            )
        })?;
    if !status.success() {
        let _ = std::fs::remove_file(&out);
        use std::os::unix::process::ExitStatusExt;
        let code = status
            .code()
            .or_else(|| status.signal().map(|signal| 128 + signal))
            .unwrap_or(exit::FAILED);
        let exit_code = match code {
            exit::SIGINT | exit::SIGTERM => code,
            _ => exit::FAILED,
        };
        return Err(Fail::new(
            exit_code,
            format!(
                "eb-stack stack sbom --lock {lock} exited {code}; its output is in {}",
                log_path.display()
            ),
        ));
    }
    let wrote_document =
        std::fs::metadata(&out).is_ok_and(|metadata| metadata.is_file() && metadata.len() > 0);
    if !wrote_document {
        let _ = std::fs::remove_file(&out);
        return Err(Fail::new(
            exit::FAILED,
            format!(
                "eb-stack stack sbom --lock {lock} wrote no non-empty {}",
                out.display()
            ),
        ));
    }
    let producer = format!("eb-stack stack sbom --lock {lock}");
    if let Err(error) = exit::read_json_output(&out, &producer, exit::FAILED) {
        let _ = std::fs::remove_file(&out);
        return Err(error);
    }
    Ok(out)
}
