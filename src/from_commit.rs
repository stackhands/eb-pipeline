//! Opt-in `check --check-from-commit` delegation to `eb-stack stack from-commit`.

use crate::exit::{self, Fail};
use crate::run::log;
use crate::schema::Site;
use crate::tools::Tool;
use std::io::Write;
use std::os::unix::process::ExitStatusExt;
use std::path::Path;
use std::process::{Command, Stdio};

/// Paths and flags forwarded to `eb-stack stack from-commit`.
pub struct Options<'a> {
    /// easybuild-easyconfigs checkout for `from-commit` pins.
    pub easyconfigs_repo: Option<&'a Path>,
    /// easybuild-easyblocks checkout for `include-easyblocks-from-commit` pins.
    pub easyblocks_repo: Option<&'a Path>,
    /// EasyBuild release `X.Y.Z`; overrides `eb-X.Y.Z` in the easystack file name.
    pub easybuild_version: Option<&'a str>,
}

/// Whether `path` names an easystack file eb-stack reads.
pub fn easystack_path(path: &Path) -> Result<(), Fail> {
    match path
        .extension()
        .and_then(|e| e.to_str())
        .map(str::to_ascii_lowercase)
    {
        Some(ext) if ext == "yml" || ext == "yaml" => Ok(()),
        _ => Err(Fail::usage(format!(
            "--check-from-commit needs an easystack .yml or .yaml path, not {}",
            path.display()
        ))),
    }
}

/// The `eb-stack` binary to run: `[site.tools.eb_stack].bin` when a site file
/// is loaded, else `eb-stack` on the executable search path (`PATH`).
pub fn eb_stack_bin(site: Option<&Site>) -> String {
    site.map(|s| Tool::EbStack.bin(s))
        .unwrap_or_else(|| Tool::EbStack.default_bin().to_string())
}

/// The argv passed to `eb-stack` after its program name.
pub fn argv(list: &Path, opts: &Options<'_>) -> Vec<String> {
    let mut v = vec![
        "stack".into(),
        "from-commit".into(),
        "--easystack".into(),
        list.display().to_string(),
        "--json".into(),
    ];
    if let Some(repo) = opts.easyconfigs_repo {
        v.extend(["--easyconfigs-repo".into(), repo.display().to_string()]);
    }
    if let Some(repo) = opts.easyblocks_repo {
        v.extend(["--easyblocks-repo".into(), repo.display().to_string()]);
    }
    if let Some(version) = opts.easybuild_version {
        v.extend(["--easybuild-version".into(), version.into()]);
    }
    v
}

/// Run `eb-stack stack from-commit` and return its exit code on completion.
pub fn run(site: Option<&Site>, list: &Path, opts: &Options<'_>) -> Result<i32, Fail> {
    easystack_path(list)?;
    let bin = eb_stack_bin(site);
    let label = site.map(|s| s.site.name.as_str()).unwrap_or("-");
    log("INFO", "from-commit", label, "running from-commit");
    let output = Command::new(&bin)
        .args(argv(list, opts))
        .stdin(Stdio::null())
        .output()
        .map_err(|e| Fail::usage(format!("cannot run `{bin} stack from-commit`: {e}")))?;
    let _ = std::io::stdout().write_all(&output.stdout);
    let _ = std::io::stderr().write_all(&output.stderr);
    let code = output
        .status
        .code()
        .unwrap_or_else(|| 128 + output.status.signal().unwrap_or(0));
    if output.status.success() {
        log("INFO", "from-commit", label, "from-commit passed");
        Ok(exit::OK)
    } else {
        log(
            "ERROR",
            "from-commit",
            label,
            &format!("from-commit failed with status {code}"),
        );
        Ok(code)
    }
}
