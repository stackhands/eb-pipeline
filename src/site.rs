//! Loading and validating site files.

use crate::exit::Fail;
use crate::schema::*;
use crate::util::parse_version;
use std::collections::BTreeSet;
use std::path::Path;

/// Parse a site file from text. Unknown keys and type errors are usage errors.
pub fn parse(text: &str) -> Result<Site, Fail> {
    toml::from_str::<Site>(text).map_err(|e| Fail::usage(format!("invalid site file: {e}")))
}

/// Read and parse a site file.
pub fn load(path: &Path) -> Result<Site, Fail> {
    let text = std::fs::read_to_string(path)
        .map_err(|e| Fail::usage(format!("cannot read {}: {e}", path.display())))?;
    parse(&text)
}

const PLACEHOLDERS: &[&str] = &["project", "build", "phase", "target"];

fn name_ok(s: &str) -> bool {
    !s.is_empty()
        && s.chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '_' | '-'))
}

fn check_template(t: &str, errs: &mut Vec<String>) {
    let mut rest = t;
    while let Some(i) = rest.find('{') {
        match rest[i..].find('}') {
            Some(j) => {
                let key = &rest[i + 1..i + j];
                if !PLACEHOLDERS.contains(&key) {
                    errs.push(format!(
                        "scheduler.job_name_template: unknown placeholder {{{key}}}"
                    ));
                }
                rest = &rest[i + j + 1..];
            }
            None => {
                errs.push("scheduler.job_name_template: unbalanced `{`".into());
                break;
            }
        }
    }
}

/// Semantic validation. Returns every problem found.
pub fn validate(site: &Site, lists_dir: Option<&Path>) -> Vec<String> {
    let mut e = Vec::new();
    if site.schema_version != 1 {
        e.push(format!(
            "schema_version: expected 1, found {}",
            site.schema_version
        ));
    }
    if site.kind != "site" {
        e.push(format!("kind: expected \"site\", found \"{}\"", site.kind));
    }
    if !name_ok(&site.site.name) {
        e.push("site.name: use letters, digits, `.`, `_` and `-`".into());
    }
    if let Some(p) = &site.site.project {
        if !name_ok(p) {
            e.push("site.project: use letters, digits, `.`, `_` and `-`".into());
        }
    }
    if let Some(t) = &site.site.tools {
        for (k, s) in [
            ("eb_stack", &t.eb_stack),
            ("eb_keep_logs", &t.eb_keep_logs),
            ("eb_staged_rebuild", &t.eb_staged_rebuild),
        ] {
            if let Some(v) = s.as_ref().and_then(|s| s.min_version.as_ref()) {
                if parse_version(v).is_none() {
                    e.push(format!(
                        "site.tools.{k}.min_version: `{v}` is not dotted numbers"
                    ));
                }
            }
        }
    }
    match site.scheduler.kind.as_str() {
        "slurm" | "local" => {}
        k => e.push(format!(
            "scheduler.kind: expected slurm or local, found `{k}`"
        )),
    }
    if site.scheduler.launch.trim().is_empty() {
        e.push("scheduler.launch: must not be empty".into());
    }
    check_template(&site.scheduler.job_name_template, &mut e);

    if site.targets.is_empty() {
        e.push("targets: at least one target is required".into());
    }
    let mut seen = BTreeSet::new();
    for t in &site.targets {
        if !name_ok(&t.name) {
            e.push(format!("targets: invalid name `{}`", t.name));
        }
        if !seen.insert(t.name.clone()) {
            e.push(format!("targets: duplicate name `{}`", t.name));
        }
        if let Some(r) = &t.route {
            match r.kind.as_str() {
                "none" => {}
                "command" => {
                    if r.command.as_deref().is_none_or(|c| c.trim().is_empty()) {
                        e.push(format!(
                            "targets.{}.route: kind command needs `command`",
                            t.name
                        ));
                    }
                }
                k => e.push(format!(
                    "targets.{}.route.kind: expected none or command, found `{k}`",
                    t.name
                )),
            }
        }
        if let Some(dir) = lists_dir {
            for l in &t.build_lists {
                if !dir.join(l).is_file() {
                    e.push(format!(
                        "targets.{}.build_lists: `{}` not found in {}",
                        t.name,
                        l,
                        dir.display()
                    ));
                }
            }
        }
    }

    let fs = &site.filesystems;
    for (k, v) in [
        ("scratch", &fs.scratch),
        ("tmp", &fs.tmp),
        ("build_path", &fs.build_path),
        ("install_root", &fs.install_root),
        ("log_root", &fs.log_root),
    ] {
        if v.trim().is_empty() {
            e.push(format!("filesystems.{k}: must not be empty"));
        }
    }

    if let Some(a) = &site.admission {
        if !matches!(a.lock_kind.as_str(), "flock" | "posix") {
            e.push(format!(
                "admission.lock_kind: expected flock or posix, found `{}`",
                a.lock_kind
            ));
        }
        if a.attempts == 0 {
            e.push("admission.attempts: must be at least 1".into());
        }
    }
    if let Some(s) = &site.shards {
        if s.enabled && s.count >= 2 && s.easyconfigs.is_empty() {
            e.push("shards.easyconfigs: at least one tree is required when count >= 2".into());
        }
    }
    if let Some(r) = &site.rebuild {
        if !matches!(
            r.backend.as_str(),
            "auto" | "bwrap" | "userns" | "installpath"
        ) {
            e.push(format!(
                "rebuild.backend: expected auto, bwrap, userns or installpath, found `{}`",
                r.backend
            ));
        }
    }
    if let Some(c) = &site.caches {
        let mut seen = BTreeSet::new();
        for ct in &c.targets {
            if !site.targets.iter().any(|t| t.name == ct.target) {
                e.push(format!("caches.targets: `{}` is not a target", ct.target));
            }
            if !seen.insert(ct.target.clone()) {
                e.push(format!("caches.targets: duplicate target `{}`", ct.target));
            }
        }
    }
    if let Some(l) = &site.licences {
        if l.enabled && l.enforce {
            if l.groups.is_empty() && l.below_version.is_empty() && l.exempt.is_empty() {
                e.push(
                    "licences: enforce is set but no group, threshold or exemption is configured"
                        .into(),
                );
            }
            let required: &[String] = l.required.as_ref().map_or(&[], |r| &r.software);
            for n in required {
                if !l.groups.contains_key(n)
                    && !l.below_version.contains_key(n)
                    && !l.exempt.contains(n)
                {
                    e.push(format!(
                        "licences.required: `{n}` has no group, threshold or exemption"
                    ));
                }
            }
        }
        for (n, v) in &l.below_version {
            if parse_version(v).is_none() {
                e.push(format!(
                    "licences.below_version.{n}: `{v}` is not dotted numbers"
                ));
            }
            if !l.groups.contains_key(n) {
                e.push(format!(
                    "licences.below_version.{n}: no group set for `{n}`"
                ));
            }
        }
    }
    e
}

impl Site {
    /// Admission section when active.
    pub fn admission_on(&self) -> Option<&Admission> {
        self.admission.as_ref().filter(|a| a.enabled)
    }
    /// Shard section when it plans shards.
    pub fn shards_on(&self) -> Option<&Shards> {
        self.shards.as_ref().filter(|s| s.enabled && s.count >= 2)
    }
    pub fn sources_on(&self) -> Option<&Sources> {
        self.sources.as_ref().filter(|s| s.enabled && s.prefetch)
    }
    pub fn caches_on(&self) -> Option<&Caches> {
        self.caches.as_ref().filter(|c| c.enabled)
    }
    pub fn logs_on(&self) -> Option<&Logs> {
        self.logs.as_ref().filter(|l| l.enabled)
    }
    pub fn rebuild_on(&self) -> Option<&Rebuild> {
        self.rebuild.as_ref().filter(|r| r.enabled && r.staged)
    }
    pub fn licences_on(&self) -> Option<&Licences> {
        self.licences.as_ref().filter(|l| l.enabled && l.enforce)
    }
    pub fn eessi_on(&self) -> Option<&Eessi> {
        self.modules.eessi.as_ref().filter(|e| e.enabled)
    }
    pub fn target(&self, name: &str) -> Result<&Target, Fail> {
        self.targets.iter().find(|t| t.name == name).ok_or_else(|| {
            Fail::usage(format!(
                "unknown target `{name}`; known: {}",
                self.targets
                    .iter()
                    .map(|t| t.name.as_str())
                    .collect::<Vec<_>>()
                    .join(", ")
            ))
        })
    }
    pub fn project(&self) -> &str {
        self.site.project.as_deref().unwrap_or(&self.site.name)
    }
}
