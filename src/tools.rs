//! Sibling tools: binaries, versions, code mapping.

use crate::exit::{self, Fail};
use crate::schema::{Site, ToolSpec};
use crate::util::{cmp_version, find_version, parse_version};
use std::collections::BTreeMap;
use std::io;
use std::process::{Command, Stdio};

#[cfg(test)]
use std::time::Duration;

/// A sibling tool that eb-pipeline starts as a subprocess.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Tool {
    /// eb-stack, for stack planning, admission, SBOMs, resume, offline bundles
    /// and, when `[attest]` is enabled, `recipe easyblock`.
    EbStack,
    /// eb-keep-logs, for `failed` and `rotate`.
    EbKeepLogs,
    /// eb-staged-rebuild, for `run` on a line that rebuilds a live install.
    EbStagedRebuild,
}

impl Tool {
    /// Every tool, in the order `[site.tools]` documents them.
    pub const ALL: [Tool; 3] = [Tool::EbStack, Tool::EbKeepLogs, Tool::EbStagedRebuild];

    /// The tool's key under `[site.tools]`: `eb_stack`, `eb_keep_logs` or
    /// `eb_staged_rebuild`.
    pub fn key(self) -> &'static str {
        match self {
            Tool::EbStack => "eb_stack",
            Tool::EbKeepLogs => "eb_keep_logs",
            Tool::EbStagedRebuild => "eb_staged_rebuild",
        }
    }

    /// The program name looked up on `PATH` when the site file sets no `bin`.
    pub fn default_bin(self) -> &'static str {
        match self {
            Tool::EbStack => "eb-stack",
            Tool::EbKeepLogs => "eb-keep-logs",
            Tool::EbStagedRebuild => "eb-staged-rebuild",
        }
    }

    /// The tool's `[site.tools.<tool>]` table, when the site file has one.
    pub fn spec(self, site: &Site) -> Option<&ToolSpec> {
        let t = site.site.tools.as_ref()?;
        match self {
            Tool::EbStack => t.eb_stack.as_ref(),
            Tool::EbKeepLogs => t.eb_keep_logs.as_ref(),
            Tool::EbStagedRebuild => t.eb_staged_rebuild.as_ref(),
        }
    }

    /// The program to start: the site file's `bin`, else
    /// [`Tool::default_bin`].
    pub fn bin(self, site: &Site) -> String {
        self.spec(site)
            .and_then(|s| s.bin.clone())
            .unwrap_or_else(|| self.default_bin().to_string())
    }

    /// The site file's `min_version` for the tool, as written.
    pub fn min_version(self, site: &Site) -> Option<String> {
        self.spec(site).and_then(|s| s.min_version.clone())
    }
}

/// Tools that an enabled phase calls.
pub fn needed(site: &Site) -> Vec<Tool> {
    let mut v = Vec::new();
    if site.shards_on().is_some()
        || site.resume_on().is_some()
        || site.admission_on().is_some()
        || site.sbom_on().is_some()
        || site.attest_on().is_some()
    {
        v.push(Tool::EbStack);
    }
    if site.logs_on().is_some() {
        v.push(Tool::EbKeepLogs);
    }
    if site.rebuild_on().is_some() {
        v.push(Tool::EbStagedRebuild);
    }
    v
}

/// Run `bin --version` and return the parsed version.
pub fn probe(bin: &str) -> Result<Vec<u64>, String> {
    let mut command = Command::new(bin);
    command.arg("--version");
    let out =
        command_output(&mut command).map_err(|e| format!("cannot run `{bin} --version`: {e}"))?;
    let text = format!(
        "{}{}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    );
    if !out.status.success() {
        let code = out
            .status
            .code()
            .map_or_else(|| out.status.to_string(), |code| code.to_string());
        let detail = text.trim();
        return Err(if detail.is_empty() {
            format!("`{bin} --version` exited {code}")
        } else {
            format!("`{bin} --version` exited {code}: {detail}")
        });
    }
    find_version(&text)
        .ok_or_else(|| format!("`{bin} --version` printed no version: {}", text.trim()))
}

/// Whether `bin --help`, run with `env`, lists `option` on stdout.
/// eb-keep-logs took `rotate --lock-timeout` without a version of its own:
/// its Zig build says 0.2.1, and its shell script 0.1.0, with the option
/// and without it.
pub fn help_lists(bin: &str, env: &BTreeMap<String, String>, option: &str) -> Result<bool, String> {
    let mut command = Command::new(bin);
    command.arg("--help").envs(env).stdin(Stdio::null());
    let out =
        command_output(&mut command).map_err(|e| format!("cannot run `{bin} --help`: {e}"))?;
    if !out.status.success() {
        let code = out
            .status
            .code()
            .map_or_else(|| out.status.to_string(), |c| c.to_string());
        return Err(format!("`{bin} --help` exited {code}"));
    }
    Ok(String::from_utf8_lossy(&out.stdout).contains(option))
}

/// Check minimum versions of the tools the site needs. With `probe_all`,
/// tools without a minimum are probed for existence too.
pub fn check_versions(site: &Site, probe_all: bool) -> Result<(), Fail> {
    let mut problems = Vec::new();
    for t in needed(site) {
        if let Err(problem) = check_one(site, t, probe_all) {
            problems.push(problem);
        }
    }
    if problems.is_empty() {
        Ok(())
    } else {
        Err(Fail::new(exit::USAGE, problems.join("; ")))
    }
}

/// Check one tool's configured minimum. With `probe_without_minimum`, also
/// require a tool that has no minimum to answer `--version`.
pub fn check_one(site: &Site, tool: Tool, probe_without_minimum: bool) -> Result<(), String> {
    let min = tool.min_version(site);
    if min.is_none() && !probe_without_minimum {
        return Ok(());
    }
    let bin = tool.bin(site);
    let found = probe(&bin).map_err(|e| format!("{}: {e}", tool.key()))?;
    if let Some(m) = min.as_deref().and_then(parse_version) {
        if cmp_version(&found, &m) == std::cmp::Ordering::Less {
            return Err(format!(
                "{}: version {} is below min_version {}",
                tool.key(),
                join(&found),
                join(&m)
            ));
        }
    }
    Ok(())
}

fn join(v: &[u64]) -> String {
    v.iter().map(u64::to_string).collect::<Vec<_>>().join(".")
}

#[cfg(not(test))]
fn command_output(command: &mut Command) -> io::Result<std::process::Output> {
    command.output()
}

#[cfg(test)]
fn command_output(command: &mut Command) -> io::Result<std::process::Output> {
    const ATTEMPTS: usize = 8;
    const BASE_MS: u64 = 5;
    let mut last_error: Option<io::Error> = None;
    for attempt in 0..ATTEMPTS {
        match command.output() {
            Ok(output) => return Ok(output),
            Err(error) if error.raw_os_error() == Some(26) && attempt + 1 < ATTEMPTS => {
                last_error = Some(error);
                std::thread::sleep(Duration::from_millis(BASE_MS * (1 << attempt)));
            }
            Err(error) => return Err(error),
        }
    }
    Err(last_error.unwrap_or_else(|| io::Error::other("cannot run command")))
}

/// Outcome of a staged-rebuild exit code.
#[derive(Debug, PartialEq, Eq, Clone, Copy)]
pub enum StagedOutcome {
    /// Exit 0: the staged rebuild succeeded.
    Ok,
    /// Exit 2 in eb-staged-rebuild: refusal.
    Refused,
    /// Exit 75: a required ledger event is pending.
    Pending,
    /// Any other exit code.
    Failed,
}

/// Read an `eb-staged-rebuild run` exit code: 0 is [`StagedOutcome::Ok`],
/// 2 [`StagedOutcome::Refused`], 75 [`StagedOutcome::Pending`], and any other
/// [`StagedOutcome::Failed`].
pub fn map_staged(code: i32) -> StagedOutcome {
    match code {
        0 => StagedOutcome::Ok,
        2 => StagedOutcome::Refused,
        75 => StagedOutcome::Pending,
        _ => StagedOutcome::Failed,
    }
}

/// Outcome of an `eb-keep-logs rotate` exit code.
#[derive(Debug, PartialEq, Eq, Clone, Copy)]
pub enum RotateOutcome {
    /// Exit 0: the log tree was rotated.
    Ok,
    /// Exit 75: another rotate of the tree held eb-keep-logs' lock past
    /// `--lock-timeout`, and this one changed nothing.
    Locked,
    /// Any other exit code.
    Failed,
}

/// Read an `eb-keep-logs rotate` exit code: 0 is [`RotateOutcome::Ok`], 75
/// [`RotateOutcome::Locked`], and any other [`RotateOutcome::Failed`].
pub fn map_rotate(code: i32) -> RotateOutcome {
    match code {
        0 => RotateOutcome::Ok,
        75 => RotateOutcome::Locked,
        _ => RotateOutcome::Failed,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;
    use std::os::unix::fs::PermissionsExt;

    #[test]
    fn a_failed_version_command_does_not_supply_an_accepted_version() {
        let dir = tempfile::tempdir().unwrap();
        let bin = dir.path().join("eb-stack");
        let mut file = std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&bin)
            .unwrap();
        file.write_all(b"#!/bin/sh\necho 'eb-stack 99.0' >&2\nexit 1\n")
            .unwrap();
        file.sync_all().unwrap();
        drop(file);
        std::fs::set_permissions(&bin, std::fs::Permissions::from_mode(0o755)).unwrap();

        let error = probe(bin.to_str().unwrap()).unwrap_err();
        assert!(error.contains("exited 1"), "{error}");
        assert!(error.contains("eb-stack 99.0"), "{error}");
    }
}
