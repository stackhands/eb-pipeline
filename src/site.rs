//! Loading and validating site files.

use crate::exit::Fail;
use crate::resolve::{Scope, MODULE_SHELLS};
use crate::schema::*;
use crate::util::parse_version;
use std::collections::{BTreeMap, BTreeSet};
use std::path::Path;

/// Parse a site file from text. Unknown keys and type errors are usage errors.
pub fn parse(text: &str) -> Result<Site, Fail> {
    toml::from_str::<Site>(text).map_err(|e| Fail::usage(format!("invalid site file: {e}")))
}

/// Read and parse a site file, then apply the `EB_PIPELINE__*` overrides
/// from the environment (see [`parse_with_overrides`]).
pub fn load(path: &Path) -> Result<Site, Fail> {
    let text = std::fs::read_to_string(path)
        .map_err(|e| Fail::usage(format!("cannot read {}: {e}", path.display())))?;
    let vars: Vec<(String, String)> = std::env::vars()
        .filter(|(k, _)| k.starts_with(OVERRIDE_PREFIX))
        .collect();
    parse_with_overrides(&text, &vars)
}

/// Prefix of an environment variable that overrides one site file value.
pub const OVERRIDE_PREFIX: &str = "EB_PIPELINE__";

/// Parse a site file and set one value per `(name, value)` pair over it.
///
/// `EB_PIPELINE__FILESYSTEMS__SCRATCH=/scratch/x` sets `filesystems.scratch`:
/// the name after the prefix is split on `__` and each part lowercased. The
/// value is read as a TOML value (`true`, `16`, `["a", "b"]`, `"16"`) and
/// taken as a string when it does not parse as one. Missing tables are
/// created. A path through an array of tables (`targets`) is refused, so
/// per-target values stay in the file. The result is checked by the same
/// schema as the file, so an unknown key or a wrong type is a usage error.
/// Pairs apply in name order, so the outcome does not depend on the order of
/// the environment.
pub fn parse_with_overrides(text: &str, vars: &[(String, String)]) -> Result<Site, Fail> {
    if vars.is_empty() {
        return parse(text);
    }
    let mut table: toml::Table =
        toml::from_str(text).map_err(|e| Fail::usage(format!("invalid site file: {e}")))?;
    let mut sorted: Vec<&(String, String)> = vars.iter().collect();
    sorted.sort();
    for (name, raw) in sorted {
        apply_override(&mut table, name, raw)?;
    }
    toml::Value::Table(table).try_into::<Site>().map_err(|e| {
        Fail::usage(format!(
            "invalid site file after environment overrides: {e}"
        ))
    })
}

fn override_path(name: &str) -> Result<Vec<String>, Fail> {
    let rest = name.strip_prefix(OVERRIDE_PREFIX).unwrap_or(name);
    let parts: Vec<String> = rest.split("__").map(str::to_ascii_lowercase).collect();
    let ok = parts.iter().all(|p| {
        !p.is_empty()
            && p.chars()
                .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '_')
    });
    if !ok {
        return Err(Fail::usage(format!(
            "{name}: an override names SECTION__KEY in letters, digits and single underscores"
        )));
    }
    Ok(parts)
}

fn override_value(raw: &str) -> toml::Value {
    toml::from_str::<toml::Table>(&format!("v = {raw}"))
        .ok()
        .and_then(|mut t| t.remove("v"))
        .unwrap_or_else(|| toml::Value::String(raw.to_string()))
}

fn apply_override(table: &mut toml::Table, name: &str, raw: &str) -> Result<(), Fail> {
    let path = override_path(name)?;
    let (last, parents) = path.split_last().expect("split yields one part");
    let mut here = table;
    for (i, part) in parents.iter().enumerate() {
        let entry = here
            .entry(part.clone())
            .or_insert(toml::Value::Table(toml::Table::new()));
        here = match entry {
            toml::Value::Table(t) => t,
            _ => {
                return Err(Fail::usage(format!(
                    "{name}: `{}` is not a table; an override cannot reach inside it",
                    path[..=i].join(".")
                )))
            }
        };
    }
    match here.get(last) {
        Some(toml::Value::Array(a)) if a.iter().any(|x| x.is_table()) => {
            return Err(Fail::usage(format!(
                "{name}: `{}` is an array of tables; set it in the site file",
                path.join(".")
            )))
        }
        Some(toml::Value::Table(_)) => {
            return Err(Fail::usage(format!(
                "{name}: `{}` is a table; name one of its keys",
                path.join(".")
            )))
        }
        _ => {}
    }
    crate::run::log(
        "INFO",
        "site",
        "-",
        &format!("{} set from {name}", path.join(".")),
    );
    here.insert(last.clone(), override_value(raw));
    Ok(())
}

const PLACEHOLDERS: &[&str] = &["project", "build", "phase", "target"];

/// Placeholders of a site path that takes one value per target.
const PER_TARGET: &[&str] = &["target"];

fn name_ok(s: &str) -> bool {
    !s.is_empty()
        && s.chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '_' | '-'))
}

fn env_name_ok(s: &str) -> bool {
    !s.is_empty()
        && !s.starts_with(|c: char| c.is_ascii_digit())
        && s.chars().all(|c| c.is_ascii_alphanumeric() || c == '_')
}

/// Report unknown or unbalanced `{placeholder}` uses in `text`.
fn check_placeholders(key: &str, text: &str, allowed: &[&str], errs: &mut Vec<String>) {
    let mut rest = text;
    while let Some(i) = rest.find('{') {
        match rest[i..].find('}') {
            Some(j) => {
                let name = &rest[i + 1..i + j];
                if !allowed.contains(&name) {
                    let known: Vec<String> = allowed.iter().map(|a| format!("{{{a}}}")).collect();
                    errs.push(format!(
                        "{key}: unknown placeholder {{{name}}}; known: {}",
                        known.join(", ")
                    ));
                }
                rest = &rest[i + j + 1..];
            }
            None => {
                errs.push(format!("{key}: unbalanced `{{`"));
                break;
            }
        }
    }
}

const ROUTE_ALL: &[&str] = &[
    "phase_dir",
    "log_dir",
    "work_dir",
    "target",
    "phase",
    "runtime_root",
    "shares_ro",
    "shares_rw",
];
const ROUTE_BASIC: &[&str] = &["phase_dir", "log_dir", "work_dir", "target", "phase"];

fn validate_route(t: &str, r: &Route, e: &mut Vec<String>) {
    let extra = !r.args.is_empty()
        || !r.env.is_empty()
        || r.runtime_root.is_some()
        || !r.shares_ro.is_empty()
        || !r.shares_rw.is_empty()
        || r.receipt_digest_env.is_some()
        || r.phases != ["build"];
    if r.kind == "none" && extra {
        e.push(format!(
            "targets.{t}.route: args, env, runtime_root, shares, receipt_digest_env and phases need kind command"
        ));
    }
    for p in &r.phases {
        if !matches!(p.as_str(), "build" | "sources") {
            e.push(format!(
                "targets.{t}.route.phases: expected build or sources, found `{p}`"
            ));
        }
    }
    if r.phases.is_empty() {
        e.push(format!("targets.{t}.route.phases: must not be empty"));
    }
    for a in &r.args {
        check_placeholders(&format!("targets.{t}.route.args"), a, ROUTE_ALL, e);
    }
    for (k, v) in &r.env {
        check_placeholders(&format!("targets.{t}.route.env.{k}"), v, ROUTE_ALL, e);
    }
    for s in r.shares_ro.iter().chain(&r.shares_rw) {
        check_placeholders(&format!("targets.{t}.route.shares"), s, ROUTE_BASIC, e);
    }
    if let Some(n) = &r.receipt_digest_env {
        if !env_name_ok(n) {
            e.push(format!(
                "targets.{t}.route.receipt_digest_env: use letters, digits and `_`"
            ));
        }
    }
}

fn validate_select(t: &str, s: &Select, e: &mut Vec<String>) {
    if !s.option.starts_with("--") || s.option.len() < 3 || s.option.contains(['=', ' ']) {
        let bare = s.option.trim_start_matches('-');
        let hint = if bare.is_empty() || bare.contains(['=', ' ']) {
            String::new()
        } else {
            format!("; write `--{bare}`")
        };
        e.push(format!(
            "targets.{t}.select.option: expected a long option with its leading dashes, such as `--name`, found `{}`{hint}",
            s.option
        ));
    }
    if s.value.trim().is_empty() {
        e.push(format!("targets.{t}.select.value: must not be empty"));
    }
    if s.separator.is_empty() {
        e.push(format!("targets.{t}.select.separator: must not be empty"));
    } else if s.value.contains(&s.separator) {
        e.push(format!(
            "targets.{t}.select.value: contains the separator `{}`",
            s.separator
        ));
    }
    if !matches!(s.when_absent.as_str(), "all" | "none") {
        e.push(format!(
            "targets.{t}.select.when_absent: expected all or none, found `{}`",
            s.when_absent
        ));
    }
}

/// The forms EESSI accepts: `cpu_target` as `<arch>/<uarch>` or
/// `<arch>/<vendor>/<uarch>`, `accel_target` as EESSI-extend matches it.
fn validate_eessi_target(t: &Target, e: &mut Vec<String>) {
    if let Some(c) = &t.cpu_target {
        let parts: Vec<&str> = c.split('/').collect();
        let ok = matches!(parts[0], "x86_64" | "aarch64" | "riscv64" | "ppc64le")
            && (2..=3).contains(&parts.len())
            && parts
                .iter()
                .all(|p| !p.is_empty() && !p.contains(char::is_whitespace));
        if !ok {
            e.push(format!(
                "targets.{}.cpu_target: expected an EESSI CPU subdirectory such as `x86_64/amd/zen4` or `aarch64/neoverse_v1`, found `{c}`",
                t.name
            ));
        }
    }
    if let Some(a) = t.accel_target.as_deref().filter(|a| !a.is_empty()) {
        let nvidia = a
            .strip_prefix("accel/nvidia/cc")
            .is_some_and(|cc| !cc.is_empty() && cc.bytes().all(|b| b.is_ascii_digit()));
        let amd = a.strip_prefix("accel/amd/gfx").is_some_and(|g| {
            !g.is_empty()
                && g.bytes()
                    .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
        });
        if !(nvidia || amd) {
            e.push(format!(
                "targets.{}.accel_target: expected `accel/nvidia/cc<digits>` or `accel/amd/gfx<hex>`, found `{a}`",
                t.name
            ));
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
    check_placeholders(
        "scheduler.job_name_template",
        &site.scheduler.job_name_template,
        PLACEHOLDERS,
        &mut e,
    );

    if site.targets.is_empty() {
        e.push("targets: at least one target is required".into());
    }
    let mut seen = BTreeSet::new();
    for t in &site.targets {
        if !name_ok(&t.name) {
            e.push(format!("targets: invalid name `{}`", t.name));
        } else if matches!(t.name.as_str(), "." | "..") {
            e.push(format!(
                "targets: invalid name `{}`; a target name is a directory name, of its log directory and wherever {{target}} expands",
                t.name
            ));
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
            validate_route(&t.name, r, &mut e);
        }
        if let Some(s) = &t.select {
            validate_select(&t.name, s, &mut e);
        }
        if site.eessi_on().is_some() {
            validate_eessi_target(t, &mut e);
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
    if !matches!(fs.export_install_vars.as_str(), "single" | "split") {
        e.push(format!(
            "filesystems.export_install_vars: expected single or split, found `{}`",
            fs.export_install_vars
        ));
    }
    if site.eessi_on().is_some_and(|es| es.site_install) && fs.export_install_vars != "split" {
        for t in &site.targets {
            let staged = site
                .rebuild_on()
                .is_some_and(|r| t.staged.unwrap_or(r.staged));
            if staged && (t.software_root.is_none() || t.modules_root.is_none()) {
                e.push(format!(
                    "targets.{}: staged rebuilds with modules.eessi.site_install need software_root and modules_root; EESSI-extend installs into <site prefix>/versions/<release>/software/linux/<cpu_target>, so set them to its software and modules directories",
                    t.name
                ));
            }
        }
    }
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
    check_placeholders("filesystems.build_path", &fs.build_path, PER_TARGET, &mut e);
    for p in &fs.source_paths {
        check_placeholders("filesystems.source_paths", p, PER_TARGET, &mut e);
    }
    let shared = [
        ("scratch", Some(&fs.scratch)),
        ("tmp", Some(&fs.tmp)),
        ("install_root", Some(&fs.install_root)),
        ("log_root", Some(&fs.log_root)),
        ("shard_dir", fs.shard_dir.as_ref()),
        ("configs_repo", fs.configs_repo.as_ref()),
    ];
    for (k, v) in shared {
        if v.is_some_and(|v| v.contains("{target}")) {
            e.push(format!(
                "filesystems.{k}: {{target}} expands in filesystems.build_path, filesystems.source_paths and rebuild.state_root only"
            ));
        }
    }

    if let Some(a) = &site.admission {
        if !matches!(a.lock_kind.as_str(), "flock" | "posix") {
            e.push(format!(
                "admission.lock_kind: expected flock or posix, found `{}`",
                a.lock_kind
            ));
        }
        if a.enabled && a.default_footprint.is_none() {
            e.push("admission.default_footprint: required when admission is enabled".into());
        }
        let env_ok = !a.job_id_env.is_empty()
            && a.job_id_env
                .chars()
                .all(|c| c.is_ascii_alphanumeric() || c == '_');
        if !env_ok {
            e.push("admission.job_id_env: use letters, digits and `_`".into());
        }
        if a.attempts == 0 {
            e.push("admission.attempts: must be at least 1".into());
        }
    }
    if let Some(s) = &site.shards {
        for n in &s.installed_modules_dirs_from_env {
            if !env_name_ok(n) {
                e.push(format!(
                    "shards.installed_modules_dirs_from_env: `{n}` is not a variable name"
                ));
            }
        }
        if s.enabled && s.count >= 2 && s.easyconfigs.is_empty() {
            e.push("shards.easyconfigs: at least one tree is required when count >= 2".into());
        }
    }
    for m in site
        .modules
        .init
        .iter()
        .filter(|m| m.contains(char::is_whitespace))
    {
        e.push(format!(
            "modules.init: `{m}` has white space; an entry is one module name, so give each module its own entry"
        ));
    }
    let shell = &site.modules.shell;
    let shell_name = Path::new(shell).file_name().and_then(|n| n.to_str());
    if !shell_name.is_some_and(|n| MODULE_SHELLS.contains(&n)) {
        let (last, rest) = MODULE_SHELLS
            .split_last()
            .expect("MODULE_SHELLS is not empty");
        e.push(format!(
            "modules.shell: expected a POSIX shell that runs the module wrapper, one of {} or {last}, by name or by path, found `{shell}`",
            rest.join(", ")
        ));
    }
    // eb-staged-rebuild copies the one `target` of a ledger configuration into
    // every record it writes.
    if let Some(r) = site.rebuild_on() {
        let mut first: BTreeMap<&str, &str> = BTreeMap::new();
        for t in site.targets.iter().filter(|t| t.staged.unwrap_or(r.staged)) {
            let Some(c) = t.ledger_config.as_deref().or(r.ledger_config.as_deref()) else {
                continue;
            };
            match first.get(c) {
                Some(other) => e.push(format!(
                    "targets.{}: records staged rebuilds through {c} as targets.{other} does, under the one target that file names; set ledger_config in each target",
                    t.name
                )),
                None => {
                    first.insert(c, &t.name);
                }
            }
        }
    }
    // eb-staged-rebuild locks a live tree through its state root, so one tree
    // takes one state root.
    if let Some(r) = site.rebuild_on().filter(|r| {
        r.state_root
            .as_deref()
            .is_some_and(|s| s.contains("{target}"))
    }) {
        let mut trees: BTreeMap<String, &str> = BTreeMap::new();
        for t in site.targets.iter().filter(|t| t.staged.unwrap_or(r.staged)) {
            let scope = Scope {
                site,
                site_file: None,
                target: t,
                build_type: None,
                run_id: String::new(),
                log_stamp: String::new(),
            };
            let tree = scope.software_root().trim_end_matches('/').to_string();
            match trees.get(&tree) {
                Some(other) => e.push(format!(
                    "targets.{}: takes staged rebuilds of the live tree {tree} as targets.{other} does, and rebuild.state_root has {{target}}, which gives that tree two state roots and two locks; drop {{target}} from state_root or give each target its own software_root",
                    t.name
                )),
                None => {
                    trees.insert(tree, &t.name);
                }
            }
        }
    }
    if let Some(r) = &site.rebuild {
        if let Some(s) = &r.state_root {
            check_placeholders("rebuild.state_root", s, PER_TARGET, &mut e);
        }
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
    if let Some(r) = site.logs.as_ref().and_then(|l| l.rotate.as_ref()) {
        if let Some(d) = r.delete_after_days.filter(|d| *d <= r.compress_after_days) {
            e.push(format!(
                "logs.rotate.delete_after_days: must be greater than compress_after_days ({}), found {d}",
                r.compress_after_days
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
    /// Sources section when it prefetches.
    pub fn sources_on(&self) -> Option<&Sources> {
        self.sources.as_ref().filter(|s| s.enabled && s.prefetch)
    }
    /// Caches section when active.
    pub fn caches_on(&self) -> Option<&Caches> {
        self.caches.as_ref().filter(|c| c.enabled)
    }
    /// Logs section when active.
    pub fn logs_on(&self) -> Option<&Logs> {
        self.logs.as_ref().filter(|l| l.enabled)
    }
    /// Rebuild section when active and `staged` is set in it or on a target.
    pub fn rebuild_on(&self) -> Option<&Rebuild> {
        self.rebuild.as_ref().filter(|r| {
            r.enabled && (r.staged || self.targets.iter().any(|t| t.staged == Some(true)))
        })
    }
    /// Licences section when active and enforced.
    pub fn licences_on(&self) -> Option<&Licences> {
        self.licences.as_ref().filter(|l| l.enabled && l.enforce)
    }
    /// `[modules.eessi]` when active.
    pub fn eessi_on(&self) -> Option<&Eessi> {
        self.modules.eessi.as_ref().filter(|e| e.enabled)
    }
    /// The target called `name`.
    ///
    /// # Errors
    ///
    /// A usage [`Fail`] that lists the known targets when none has the name.
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
    /// The `{project}` of job names: `site.project`, else `site.name`.
    pub fn project(&self) -> &str {
        self.site.project.as_deref().unwrap_or(&self.site.name)
    }
}

#[cfg(test)]
mod override_tests {
    use super::*;

    const BASE: &str = include_str!("../templates/small-cluster.toml");

    fn set(pairs: &[(&str, &str)]) -> Result<Site, Fail> {
        let v: Vec<(String, String)> = pairs
            .iter()
            .map(|(k, v)| (k.to_string(), v.to_string()))
            .collect();
        parse_with_overrides(BASE, &v)
    }

    #[test]
    fn no_overrides_is_plain_parse() {
        let a = set(&[]).unwrap();
        let b = parse(BASE).unwrap();
        assert_eq!(
            serde_json::to_string(&a).unwrap(),
            serde_json::to_string(&b).unwrap()
        );
    }

    #[test]
    fn sets_strings_and_arrays() {
        let s = set(&[
            ("EB_PIPELINE__FILESYSTEMS__SCRATCH", "/scratch/other"),
            (
                "EB_PIPELINE__FILESYSTEMS__SOURCE_PATHS",
                r#"["/opt/a", "/opt/b"]"#,
            ),
            ("EB_PIPELINE__SITE__STACK_GENERATION", "2027a"),
        ])
        .unwrap();
        assert_eq!(s.filesystems.scratch, "/scratch/other");
        assert_eq!(s.filesystems.source_paths, vec!["/opt/a", "/opt/b"]);
        assert_eq!(s.site.stack_generation.as_deref(), Some("2027a"));
    }

    #[test]
    fn refuses_unknown_key_and_wrong_type() {
        assert_eq!(
            set(&[("EB_PIPELINE__SITE__NO_SUCH", "x")])
                .unwrap_err()
                .code,
            2
        );
        assert_eq!(
            set(&[("EB_PIPELINE__FILESYSTEMS__SOURCE_PATHS", "7")])
                .unwrap_err()
                .code,
            2
        );
    }

    #[test]
    fn refuses_tables_arrays_of_tables_and_bad_names() {
        for (k, v) in [
            ("EB_PIPELINE__TARGETS__NAME", "x"),
            ("EB_PIPELINE__TARGETS", "x"),
            ("EB_PIPELINE__SITE", "x"),
            ("EB_PIPELINE__SITE____NAME", "x"),
            ("EB_PIPELINE__SITE__NA-ME", "x"),
        ] {
            assert_eq!(set(&[(k, v)]).unwrap_err().code, 2, "{k}");
        }
    }

    #[test]
    fn order_of_the_environment_does_not_matter() {
        let a = set(&[
            ("EB_PIPELINE__SITE__NAME", "one"),
            ("EB_PIPELINE__SITE__PROJECT", "p"),
        ])
        .unwrap();
        let b = set(&[
            ("EB_PIPELINE__SITE__PROJECT", "p"),
            ("EB_PIPELINE__SITE__NAME", "one"),
        ])
        .unwrap();
        assert_eq!(
            serde_json::to_string(&a).unwrap(),
            serde_json::to_string(&b).unwrap()
        );
    }
}

#[cfg(test)]
mod route_doc_tests {
    use super::*;
    use std::collections::BTreeSet;

    /// The placeholders that the schema text of a route key names.
    fn named(key: &str) -> BTreeSet<&'static str> {
        let doc = Route::FIELDS.iter().find(|f| f.key == key).unwrap().doc;
        let mut out = BTreeSet::new();
        let mut rest = doc;
        while let Some(i) = rest.find('{') {
            let j = rest[i..].find('}').unwrap();
            out.insert(&rest[i + 1..i + j]);
            rest = &rest[i + j + 1..];
        }
        out
    }

    #[test]
    fn route_texts_name_the_placeholders_check_accepts() {
        assert_eq!(named("args"), ROUTE_ALL.iter().copied().collect());
        assert_eq!(named("shares_ro"), ROUTE_BASIC.iter().copied().collect());
    }
}
