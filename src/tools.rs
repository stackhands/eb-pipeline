//! Sibling tools: binaries, versions, code mapping.

use crate::exit::{self, Fail};
use crate::schema::{Site, ToolSpec};
use crate::util::{cmp_version, find_version, parse_version};
use std::process::Command;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Tool {
    EbStack,
    EbKeepLogs,
    EbStagedRebuild,
}

impl Tool {
    pub const ALL: [Tool; 3] = [Tool::EbStack, Tool::EbKeepLogs, Tool::EbStagedRebuild];

    pub fn key(self) -> &'static str {
        match self {
            Tool::EbStack => "eb_stack",
            Tool::EbKeepLogs => "eb_keep_logs",
            Tool::EbStagedRebuild => "eb_staged_rebuild",
        }
    }

    pub fn default_bin(self) -> &'static str {
        match self {
            Tool::EbStack => "eb-stack",
            Tool::EbKeepLogs => "eb-keep-logs",
            Tool::EbStagedRebuild => "eb-staged-rebuild",
        }
    }

    pub fn spec(self, site: &Site) -> Option<&ToolSpec> {
        let t = site.site.tools.as_ref()?;
        match self {
            Tool::EbStack => t.eb_stack.as_ref(),
            Tool::EbKeepLogs => t.eb_keep_logs.as_ref(),
            Tool::EbStagedRebuild => t.eb_staged_rebuild.as_ref(),
        }
    }

    pub fn bin(self, site: &Site) -> String {
        self.spec(site)
            .and_then(|s| s.bin.clone())
            .unwrap_or_else(|| self.default_bin().to_string())
    }

    pub fn min_version(self, site: &Site) -> Option<String> {
        self.spec(site).and_then(|s| s.min_version.clone())
    }
}

/// Tools that an enabled phase calls.
pub fn needed(site: &Site) -> Vec<Tool> {
    let mut v = Vec::new();
    if site.shards_on().is_some() || site.admission_on().is_some() {
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
    let out = Command::new(bin)
        .arg("--version")
        .output()
        .map_err(|e| format!("cannot run `{bin} --version`: {e}"))?;
    let text = format!(
        "{}{}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    );
    find_version(&text).ok_or_else(|| format!("`{bin} --version` printed no version: {}", text.trim()))
}

/// Check minimum versions of the tools the site needs. With `probe_all`,
/// tools without a minimum are probed for existence too.
pub fn check_versions(site: &Site, probe_all: bool) -> Result<(), Fail> {
    let mut problems = Vec::new();
    for t in needed(site) {
        let min = t.min_version(site);
        if min.is_none() && !probe_all {
            continue;
        }
        let bin = t.bin(site);
        match probe(&bin) {
            Err(e) => problems.push(format!("{}: {e}", t.key())),
            Ok(found) => {
                if let Some(m) = min.as_deref().and_then(parse_version) {
                    if cmp_version(&found, &m) == std::cmp::Ordering::Less {
                        problems.push(format!(
                            "{}: version {} is below min_version {}",
                            t.key(),
                            join(&found),
                            join(&m)
                        ));
                    }
                }
            }
        }
    }
    if problems.is_empty() {
        Ok(())
    } else {
        Err(Fail::new(exit::USAGE, problems.join("; ")))
    }
}

fn join(v: &[u64]) -> String {
    v.iter().map(u64::to_string).collect::<Vec<_>>().join(".")
}

/// Outcome of a staged-rebuild exit code.
#[derive(Debug, PartialEq, Eq, Clone, Copy)]
pub enum StagedOutcome {
    Ok,
    /// Exit 2 in eb-staged-rebuild: refusal.
    Refused,
    /// Exit 75: a required ledger event is pending.
    Pending,
    Failed,
}

pub fn map_staged(code: i32) -> StagedOutcome {
    match code {
        0 => StagedOutcome::Ok,
        2 => StagedOutcome::Refused,
        75 => StagedOutcome::Pending,
        _ => StagedOutcome::Failed,
    }
}
