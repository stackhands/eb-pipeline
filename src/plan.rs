//! Plan construction: phases, launch lines, environments, tool calls.
//!
//! The planner is pure. `run` executes the commands built here.

use crate::exit::{self, Fail};
use crate::resolve::{Role, Scope};
use crate::tools::Tool;
use crate::util::{parse_list, ListLine};
use serde::Serialize;
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

/// One subprocess the driver starts.
#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct Cmd {
    pub name: String,
    /// Full argument vector: launch prefix, module wrapper, tool.
    pub argv: Vec<String>,
    /// Environment added to the phase environment.
    #[serde(skip_serializing_if = "BTreeMap::is_empty")]
    pub env: BTreeMap<String, String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub note: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
pub struct PlanPhase {
    pub name: String,
    /// `planned` or `not configured`.
    pub status: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub reason: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub role: Option<String>,
    pub env: BTreeMap<String, String>,
    pub commands: Vec<Cmd>,
}

#[derive(Debug, Clone, Serialize)]
pub struct Plan {
    pub schema_version: u32,
    pub kind: String,
    pub site: String,
    pub target: String,
    pub build_type: String,
    pub run_id: String,
    pub list: String,
    pub log_dir: String,
    pub lines: usize,
    /// Lines kept by the target's selector; absent without `[targets.select]`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub selected: Option<usize>,
    /// File the selected lines are written to, which `stack shard` reads.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub selected_list: Option<String>,
    pub phases: Vec<PlanPhase>,
    /// Lines left out by the target's selector.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub deselected: Vec<Deselected>,
}

/// A line that the target's selector left out.
#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct Deselected {
    pub easyconfig: String,
    pub unit: String,
}

pub const PHASES: [&str; 8] = [
    "preflight",
    "licences",
    "shard",
    "sources",
    "admission",
    "build",
    "caches",
    "logs",
];

/// A build unit: the whole list, or one shard list.
#[derive(Debug, Clone)]
pub struct Unit {
    pub name: String,
    pub role: Role,
    pub lines: Vec<ListLine>,
    /// Inode footprint from the shard plan.
    pub footprint: Option<u64>,
}

#[derive(Clone)]
pub struct Planner<'a> {
    pub scope: Scope<'a>,
    pub list_path: String,
    /// Scheduler job id read at run time; `None` plans the `${NAME}` placeholder.
    pub job_id: Option<String>,
    /// Value of the route's receipt digest variable read at run time.
    pub route_digest: Option<String>,
    /// Values of the `installed_modules_dirs_from_env` variables read at run
    /// time; `None` plans the `${NAME}` placeholders and keeps every
    /// installed-modules directory.
    pub runtime_env: Option<BTreeMap<String, String>>,
}

/// Module path of an easyconfig relative to `<modules_root>/all`, without extension.
pub fn module_rel(easyconfig: &str) -> Option<String> {
    let (name, _) = crate::util::package_of(easyconfig);
    let base = easyconfig.rsplit('/').next().unwrap_or(easyconfig);
    let stem = base.strip_suffix(".eb").unwrap_or(base);
    let version = stem.strip_prefix(&format!("{name}-"))?;
    Some(format!("{name}/{version}"))
}

/// Whether a build list line asks for a rebuild, by whole options only.
pub fn wants_rebuild(line: &ListLine) -> bool {
    line.args
        .iter()
        .any(|a| a == "--rebuild" || a == "-f" || a == "--force")
}

/// Line options for a fetch with the robot disabled: a bare `--robot` goes,
/// and the last `--robot=P` stays at the front of the search path so the
/// line's easyconfig is still found. EasyBuild searches `P` then
/// `--robot-paths`, which `--robot-paths=P:X` keeps; `P:` is `P` followed by
/// the default paths.
pub fn without_robot<'a>(args: impl Iterator<Item = &'a String>) -> Vec<String> {
    let mut robot: Option<&str> = None;
    let mut out: Vec<String> = Vec::new();
    for a in args {
        if a == "--robot" {
            robot = Some("");
        } else if let Some(p) = a.strip_prefix("--robot=") {
            robot = Some(p);
        } else {
            out.push(a.clone());
        }
    }
    let Some(lead) = robot.filter(|p| !p.is_empty()) else {
        return out;
    };
    match out.iter().rposition(|a| a.starts_with("--robot-paths=")) {
        Some(i) => out[i] = format!("--robot-paths={lead}:{}", &out[i]["--robot-paths=".len()..]),
        None => out.push(format!("--robot-paths={lead}:")),
    }
    out
}

/// A scheduler job id: digits only, positive, no leading zero.
pub fn valid_job_id(s: &str) -> bool {
    !s.is_empty() && !s.starts_with('0') && s.chars().all(|c| c.is_ascii_digit())
}

fn stem(easyconfig: &str) -> String {
    let base = easyconfig.rsplit('/').next().unwrap_or(easyconfig);
    base.strip_suffix(".eb").unwrap_or(base).to_string()
}

/// Hands out log stems that stay unique across a whole run.
#[derive(Default)]
pub struct StemSeq(BTreeMap<String, usize>);

impl StemSeq {
    pub fn next(&mut self, easyconfig: &str) -> String {
        let s = stem(easyconfig);
        let n = self.0.entry(s.clone()).or_insert(0);
        *n += 1;
        if *n == 1 {
            s
        } else {
            format!("{s}.{n}")
        }
    }
}

/// Log stems, unique across the lines.
pub fn unique_stems(lines: &[ListLine]) -> Vec<String> {
    let mut seq = StemSeq::default();
    lines.iter().map(|l| seq.next(&l.easyconfig)).collect()
}

impl<'a> Planner<'a> {
    pub fn new(scope: Scope<'a>, list_path: String) -> Self {
        Planner {
            scope,
            list_path,
            job_id: None,
            route_digest: None,
            runtime_env: None,
        }
    }

    /// Apply the target's selector to the list lines, in order. Kept lines
    /// carry the arguments `strip` leaves; the others are recorded. Without a
    /// selector every line is kept as read and nothing is recorded.
    pub fn select_lines(&self, lines: &[ListLine]) -> (Vec<ListLine>, Vec<Deselected>) {
        let Some(sel) = &self.scope.target.select else {
            return (lines.to_vec(), Vec::new());
        };
        let mut kept = Vec::new();
        let mut out = Vec::new();
        for l in lines {
            let r = crate::select::select_line(sel, l);
            if r.planned {
                let raw = if sel.strip {
                    std::iter::once(&l.easyconfig)
                        .chain(&r.args)
                        .map(|a| quote(a))
                        .collect::<Vec<_>>()
                        .join(" ")
                } else {
                    l.raw.clone()
                };
                kept.push(ListLine {
                    raw,
                    args: r.args,
                    ..l.clone()
                });
            } else {
                out.push(Deselected {
                    easyconfig: l.easyconfig.clone(),
                    unit: "all".into(),
                });
            }
        }
        (kept, out)
    }

    /// Where the selected build list lives, when the target has a selector.
    pub fn selected_list_path(&self) -> Option<PathBuf> {
        self.scope.target.select.as_ref()?;
        Some(
            PathBuf::from(self.scope.log_dir())
                .join(format!("selected-{}.list", self.scope.target.name)),
        )
    }

    /// The list `stack shard` reads: the selected list when there is one.
    fn shard_list(&self) -> String {
        match self.selected_list_path() {
            Some(p) => p.display().to_string(),
            None => self.list_path.clone(),
        }
    }

    fn site(&self) -> &crate::schema::Site {
        self.scope.site
    }

    fn wrapped(
        &self,
        role: Role,
        phase: &str,
        inner: Vec<String>,
        modules: bool,
    ) -> Result<Vec<String>, Fail> {
        self.wrapped_routed(
            role,
            phase,
            inner,
            modules,
            (role == Role::Cpu).then_some(phase),
        )
    }

    fn wrapped_routed(
        &self,
        role: Role,
        phase: &str,
        inner: Vec<String>,
        modules: bool,
        route_phase: Option<&str>,
    ) -> Result<Vec<String>, Fail> {
        let mut v = self.scope.launch_routed(role, phase, None, route_phase)?;
        if modules {
            v.extend(self.scope.module_wrap());
        }
        v.extend(inner);
        Ok(v)
    }

    pub fn shard_dir(&self) -> String {
        let base = self
            .site()
            .filesystems
            .shard_dir
            .clone()
            .unwrap_or_else(|| format!("{}/shards", self.site().filesystems.scratch));
        format!("{}/{}", base.trim_end_matches('/'), self.scope.run_id)
    }

    /// Module trees of `stack shard --installed-modules-dir`: the configured
    /// ones, then those of `installed_modules_dirs_from_env`.
    fn installed_modules_dirs(&self) -> Vec<String> {
        let s = self.site().shards_on().expect("shards active");
        let mut v = s.installed_modules_dirs.clone();
        for n in &s.installed_modules_dirs_from_env {
            match &self.runtime_env {
                None => v.push(format!("${{{n}}}")),
                Some(vars) => v.extend(
                    vars.get(n)
                        .map(String::as_str)
                        .unwrap_or("")
                        .split(':')
                        .filter(|d| !d.is_empty())
                        .map(str::to_string),
                ),
            }
        }
        v
    }

    /// Module trees `run` leaves out of the shard phase because they do not
    /// exist. A site's tree appears with its first install, and eb-stack
    /// refuses a directory that is missing.
    pub fn missing_installed_modules_dirs(&self) -> Vec<String> {
        if self.runtime_env.is_none() {
            return Vec::new();
        }
        self.installed_modules_dirs()
            .into_iter()
            .filter(|d| {
                std::fs::metadata(d).is_err_and(|e| e.kind() == std::io::ErrorKind::NotFound)
            })
            .collect()
    }

    pub fn shard_cmd(&self) -> Result<Cmd, Fail> {
        let s = self.site().shards_on().expect("shards active");
        let mut a = vec![
            Tool::EbStack.bin(self.site()),
            "stack".into(),
            "shard".into(),
            "--build-list".into(),
            self.shard_list(),
            "--shards".into(),
            s.count.to_string(),
            "--out-dir".into(),
            self.shard_dir(),
        ];
        for e in &s.easyconfigs {
            a.push("--easyconfigs".into());
            a.push(e.clone());
        }
        let missing = self.missing_installed_modules_dirs();
        for d in self.installed_modules_dirs() {
            if !missing.contains(&d) {
                a.push("--installed-modules-dir".into());
                a.push(d);
            }
        }
        if let Some(w) = &s.weights {
            a.push("--weights".into());
            a.push(w.clone());
        }
        if s.lift_cpu_prerequisites {
            a.push("--lift-cpu-prerequisites".into());
        }
        if let Some(adm) = self.site().admission_on() {
            if let Some(q) = adm.quota_total {
                a.push("--inode-quota".into());
                a.push(q.to_string());
                if let Some(m) = adm.margin {
                    a.push("--inode-margin".into());
                    a.push(m.to_string());
                }
            }
            if let Some(f) = &adm.footprints {
                a.push("--inode-footprints".into());
                a.push(f.clone());
            }
            if let Some(o) = adm.line_overhead {
                a.push("--inode-line-overhead".into());
                a.push(o.to_string());
            }
        }
        Ok(Cmd {
            name: "shard".into(),
            argv: self.wrapped(Role::Plan, "shard", a, true)?,
            env: BTreeMap::new(),
            note: Some(format!(
                "exit {} means the plan was built unsharded and the list builds as one unit",
                s.planner_exit_unsharded
            )),
        })
    }

    /// Route phase of a command of `role`; only CPU-role units are routed.
    fn route_phase(role: Role, phase: &'static str) -> Option<&'static str> {
        (role == Role::Cpu).then_some(phase)
    }

    /// Environment of a routed command of `role` in `phase`.
    fn route_env(&self, role: Role, phase: &'static str) -> BTreeMap<String, String> {
        match Self::route_phase(role, phase) {
            Some(p) => self.scope.route_env(p, self.route_digest.as_deref()),
            None => BTreeMap::new(),
        }
    }

    pub fn prefetch_cmd(&self, line: &ListLine, name: &str, unit_role: Role) -> Result<Cmd, Fail> {
        let extra = self
            .site()
            .sources_on()
            .map(|s| s.extra_args.clone())
            .unwrap_or_default();
        let args = line.args.iter().filter(|x| !x.starts_with("--rebuild"));
        let mut a = vec!["eb".to_string(), line.easyconfig.clone(), "--fetch".into()];
        if extra.iter().any(|x| x == "--disable-robot") {
            a.extend(without_robot(args));
        } else {
            a.push("--robot".into());
            a.extend(args.cloned());
        }
        a.extend(extra);
        let mut env = self.scope.line_env(&line.easyconfig);
        env.extend(self.scope.fetch_env());
        env.extend(self.scope.hardware_env(unit_role));
        env.extend(self.route_env(unit_role, "sources"));
        Ok(Cmd {
            name: format!("fetch:{name}"),
            argv: self.wrapped_routed(
                Role::Short,
                "sources",
                a,
                true,
                Self::route_phase(unit_role, "sources"),
            )?,
            env,
            note: None,
        })
    }

    pub fn admission_cmd(&self, unit: &str, footprint: u64) -> Result<Cmd, Fail> {
        let adm = self.site().admission_on().expect("admission active");
        let job = self
            .job_id
            .clone()
            .unwrap_or_else(|| format!("${{{}}}", adm.job_id_env));
        let mut a = vec![
            Tool::EbStack.bin(self.site()),
            "stack".into(),
            "admit".into(),
            "--quota-cmd".into(),
            adm.quota_cmd.clone().unwrap_or_default(),
            "--reservations".into(),
            adm.reservations.clone().unwrap_or_default(),
            "--footprint".into(),
            footprint.to_string(),
            "--job".into(),
            job,
            "--lock-kind".into(),
            adm.lock_kind.clone(),
        ];
        if let Some(l) = &adm.lock {
            a.push("--lock".into());
            a.push(l.clone());
        }
        if let Some(m) = adm.margin {
            a.push("--margin".into());
            a.push(m.to_string());
        }
        if let Some(u) = adm.unknown_reserve {
            a.push("--unknown-reserve".into());
            a.push(u.to_string());
        }
        if let Some(c) = &adm.live_jobs_cmd {
            a.push("--live-jobs-cmd".into());
            a.push(c.clone());
        }
        Ok(Cmd {
            name: format!("admit:{unit}"),
            argv: self.wrapped(Role::Short, "admission", a, false)?,
            env: BTreeMap::new(),
            note: Some(format!(
                "exit 78 retries up to {} attempts, waiting {}s doubling to at most {}s",
                adm.attempts, adm.wait_seconds, adm.max_wait_seconds
            )),
        })
    }

    /// Whether the target routes rebuilds through staged rebuild.
    pub fn staged_enabled(&self) -> bool {
        match self.site().rebuild_on() {
            Some(r) => self.scope.target.staged.unwrap_or(r.staged),
            None => false,
        }
    }

    /// Whether the line's module file exists under the target's modules root.
    pub fn installed(&self, line: &ListLine) -> bool {
        let Some(rel) = module_rel(&line.easyconfig) else {
            return false;
        };
        let base = format!("{}/all/{}", self.scope.modules_root(), rel);
        std::path::Path::new(&format!("{base}.lua")).is_file()
            || std::path::Path::new(&base).is_file()
    }

    /// Staged rebuild applies when it is enabled for the target, the line
    /// carries a whole rebuild option, and the module is already installed.
    pub fn is_staged(&self, line: &ListLine) -> bool {
        self.staged_enabled() && wants_rebuild(line) && self.installed(line)
    }

    /// eb-staged-rebuild run id of one line. That tool refuses a run id it
    /// has seen before, so each line gets its own: `<run-id>-<stem>`.
    pub fn staged_run_id(&self, stem: &str) -> String {
        format!("{}-{stem}", self.scope.run_id)
    }

    /// Why a rebuild line on a staged target builds with plain `eb`: no
    /// module file under the modules root.
    pub fn in_place_reason(&self, line: &ListLine) -> Option<String> {
        if !self.staged_enabled() || !wants_rebuild(line) || self.installed(line) {
            return None;
        }
        let rel = module_rel(&line.easyconfig).unwrap_or_else(|| "<Name>/<version>".into());
        Some(format!(
            "no module file {}/all/{rel}[.lua]; the rebuild runs as plain eb",
            self.scope.modules_root()
        ))
    }

    pub fn log_stdout(&self, stem: &str) -> String {
        format!("{}/{}.stdout.log", self.scope.log_dir(), stem)
    }

    pub fn tmp_logdir(&self, stem: &str) -> String {
        format!("{}/easybuild-logs/{}", self.scope.log_dir(), stem)
    }

    pub fn build_cmd(&self, role: Role, line: &ListLine, stem: &str) -> Result<Cmd, Fail> {
        let mut inner: Vec<String>;
        let note;
        if self.is_staged(line) {
            let r = self.site().rebuild_on().expect("rebuild active");
            inner = vec![
                Tool::EbStagedRebuild.bin(self.site()),
                "run".into(),
                "--software".into(),
                self.scope.software_root(),
                "--modules".into(),
                self.scope.modules_root(),
                "--backend".into(),
                r.backend.clone(),
                "--run-id".into(),
                self.staged_run_id(stem),
            ];
            if let Some(s) = self.scope.state_root() {
                inner.push("--state-root".into());
                inner.push(s);
            }
            if let Some(g) = r.grace_hours {
                inner.push("--grace-hours".into());
                inner.push(g.to_string());
            }
            if let Some(p) = &r.prepare {
                inner.push("--prepare".into());
                inner.push(p.clone());
            }
            if let Some(c) = self.scope.ledger_config() {
                inner.push("--ledger-config".into());
                inner.push(c);
            }
            inner.push("--".into());
            note = Some("staged rebuild: exit 2 is a refusal, 75 is pending".to_string());
        } else {
            inner = vec!["eb".into()];
            note = self.in_place_reason(line);
        }
        inner.push(line.easyconfig.clone());
        inner.extend(line.args.iter().cloned());
        if self.site().logs_on().is_some_and(|l| l.keep_failed) {
            inner.push(format!("--tmp-logdir={}", self.tmp_logdir(stem)));
        }
        let mut env = self.scope.line_env(&line.easyconfig);
        env.extend(self.scope.hardware_env(role));
        env.extend(self.route_env(role, "build"));
        Ok(Cmd {
            name: format!("build:{stem}"),
            argv: self.wrapped(role, "build", inner, true)?,
            env,
            note,
        })
    }

    pub fn keep_failed_cmd(&self, stem: &str, status: &str) -> Result<Cmd, Fail> {
        let a = vec![
            Tool::EbKeepLogs.bin(self.site()),
            "failed".into(),
            status.to_string(),
            stem.to_string(),
            self.scope.log_dir(),
            self.tmp_logdir(stem),
        ];
        // `failed` reads the log path `eb --last-log` prints; without a
        // terminal, EasyBuild's default style wraps it at 80 columns.
        let env = BTreeMap::from([("EASYBUILD_OUTPUT_STYLE".to_string(), "basic".to_string())]);
        Ok(Cmd {
            name: format!("keep-failed:{stem}"),
            argv: self.wrapped(Role::Short, "logs", a, true)?,
            env,
            note: Some("runs after a failed line".into()),
        })
    }

    pub fn rotate_cmd(&self) -> Result<Option<Cmd>, Fail> {
        let Some(l) = self.site().logs_on() else {
            return Ok(None);
        };
        let Some(r) = &l.rotate else {
            return Ok(None);
        };
        let mut a = vec![
            Tool::EbKeepLogs.bin(self.site()),
            "rotate".into(),
            self.site().filesystems.log_root.clone(),
            "--compress-after".into(),
            r.compress_after_days.to_string(),
        ];
        if let Some(d) = r.delete_after_days {
            a.push("--delete-after".into());
            a.push(d.to_string());
        }
        Ok(Some(Cmd {
            name: "rotate".into(),
            argv: self.wrapped(Role::Short, "logs", a, false)?,
            // eb-keep-logs counts a run's age from the local midnight of the
            // date in its name, and run directories are named in UTC.
            env: BTreeMap::from([("TZ".to_string(), "UTC".to_string())]),
            note: None,
        }))
    }

    pub fn cache_cmd(&self) -> Result<Option<Cmd>, Fail> {
        let Some(c) = self.site().caches_on() else {
            return Ok(None);
        };
        let Some(command) = &c.command else {
            return Ok(None);
        };
        let Some(entry) = c
            .targets
            .iter()
            .find(|t| t.target == self.scope.target.name)
        else {
            return Ok(None);
        };
        let mut env = BTreeMap::new();
        env.insert("EB_PIPELINE_CACHE_CONFIG".to_string(), entry.config.clone());
        if let Some(b) = &c.bindings {
            env.insert("EB_PIPELINE_CACHE_BINDINGS".into(), b.clone());
        }
        if let Some(m) = entry.init_module.as_ref().or(c.init_module.as_ref()) {
            env.insert("EB_PIPELINE_CACHE_INIT_MODULE".into(), m.clone());
        }
        let mut v = self
            .scope
            .launch_with(Role::Cache, "caches", entry.alloc.as_deref())?;
        v.extend(self.scope.module_wrap());
        v.extend(crate::util::split_args(command)?);
        Ok(Some(Cmd {
            name: "cache".into(),
            argv: v,
            env,
            note: None,
        }))
    }
}

/// Build the whole plan for one target and list.
pub fn build_plan(
    p: &Planner,
    lines: &[ListLine],
    shard_units: Option<&[Unit]>,
) -> Result<Plan, Fail> {
    let site = p.scope.site;
    let env = p.scope.env();
    let mut phases = Vec::new();
    let skip = |name: &str, reason: &str| PlanPhase {
        name: name.into(),
        status: "not configured".into(),
        reason: Some(reason.into()),
        role: None,
        env: BTreeMap::new(),
        commands: vec![],
    };
    let planned = |name: &str, role: Option<Role>, commands: Vec<Cmd>| PlanPhase {
        name: name.into(),
        status: "planned".into(),
        reason: None,
        role: role.map(|r| r.name().to_string()),
        env: env.clone(),
        commands,
    };

    // preflight: tool probes
    let probes: Vec<Cmd> = crate::tools::needed(site)
        .into_iter()
        .filter_map(|t| {
            let min = t.min_version(site)?;
            Some(Cmd {
                name: format!("probe:{}", t.key()),
                argv: vec![t.bin(site), "--version".into()],
                env: BTreeMap::new(),
                note: Some(format!("min_version {min}")),
            })
        })
        .collect();
    phases.push(PlanPhase {
        name: "preflight".into(),
        status: "planned".into(),
        reason: None,
        role: None,
        env: BTreeMap::new(),
        commands: probes,
    });

    phases.push(if site.licences_on().is_some() {
        PlanPhase {
            name: "licences".into(),
            status: "planned".into(),
            reason: None,
            role: None,
            env: BTreeMap::new(),
            commands: vec![],
        }
    } else {
        skip("licences", "not configured")
    });

    phases.push(if site.shards_on().is_some() {
        planned("shard", Some(Role::Plan), vec![p.shard_cmd()?])
    } else {
        skip("shard", "not configured")
    });

    // shard units come from the selected list, so only the whole list is filtered
    let (kept, deselected) = p.select_lines(lines);
    let units: Vec<Unit> = match shard_units {
        Some(u) if !u.is_empty() => u.to_vec(),
        _ if p.scope.target.select.is_some() && kept.is_empty() => vec![],
        _ => vec![Unit {
            name: "all".into(),
            role: Role::Build,
            lines: kept.clone(),
            footprint: None,
        }],
    };
    let mut seq = StemSeq::default();
    let stems: Vec<Vec<String>> = units
        .iter()
        .map(|u| u.lines.iter().map(|l| seq.next(&l.easyconfig)).collect())
        .collect();
    phases.push(if site.sources_on().is_some() {
        let mut cmds = Vec::new();
        for (u, ss) in units.iter().zip(&stems) {
            for (l, s) in u.lines.iter().zip(ss) {
                cmds.push(p.prefetch_cmd(l, s, u.role)?);
            }
        }
        planned("sources", Some(Role::Short), cmds)
    } else {
        skip("sources", "not configured")
    });

    phases.push(match site.admission_on() {
        None => skip("admission", "not configured"),
        Some(a) if a.quota_cmd.is_none() || a.reservations.is_none() => {
            if a.required {
                PlanPhase {
                    name: "admission".into(),
                    status: "refused".into(),
                    reason: Some(
                        "admission.required is set but quota_cmd or reservations is missing".into(),
                    ),
                    role: None,
                    env: BTreeMap::new(),
                    commands: vec![],
                }
            } else {
                skip("admission", "not configured")
            }
        }
        Some(a) => {
            let mut cmds = Vec::new();
            for u in &units {
                let fp = u.footprint.or(a.default_footprint).unwrap_or(0);
                cmds.push(p.admission_cmd(&u.name, fp)?);
            }
            planned("admission", Some(Role::Short), cmds)
        }
    });

    let mut cmds = Vec::new();
    for (u, ss) in units.iter().zip(&stems) {
        for (l, s) in u.lines.iter().zip(ss) {
            cmds.push(p.build_cmd(u.role, l, s)?);
            if site.logs_on().is_some_and(|l| l.keep_failed) {
                cmds.push(p.keep_failed_cmd(s, "<exit-status>")?);
            }
        }
    }
    phases.push(planned("build", Some(Role::Build), cmds));

    phases.push(match p.cache_cmd()? {
        Some(c) => planned("caches", Some(Role::Cache), vec![c]),
        None => skip("caches", "not configured"),
    });

    phases.push(match p.rotate_cmd()? {
        Some(c) => planned("logs", Some(Role::Short), vec![c]),
        None => skip("logs", "not configured"),
    });

    Ok(Plan {
        schema_version: 1,
        kind: "plan".into(),
        site: site.site.name.clone(),
        target: p.scope.target.name.clone(),
        build_type: p.scope.build_name().to_string(),
        run_id: p.scope.run_id.clone(),
        list: p.list_path.clone(),
        log_dir: p.scope.log_dir(),
        lines: lines.len(),
        selected: p.scope.target.select.as_ref().map(|_| kept.len()),
        selected_list: p.selected_list_path().map(|x| x.display().to_string()),
        phases,
        deselected,
    })
}

/// Render a command as one shell-quoted line.
pub fn shell_line(c: &Cmd) -> String {
    let mut s = String::new();
    for (k, v) in &c.env {
        s.push_str(&format!("{k}={} ", quote(v)));
    }
    s.push_str(
        &c.argv
            .iter()
            .map(|a| quote(a))
            .collect::<Vec<_>>()
            .join(" "),
    );
    s
}

fn quote(a: &str) -> String {
    if !a.is_empty()
        && a.chars()
            .all(|c| c.is_ascii_alphanumeric() || "-_./=:,@%+".contains(c))
    {
        a.to_string()
    } else {
        format!("'{}'", a.replace('\'', "'\\''"))
    }
}

/// A shard-plan `file` value: one name in `dir`, never an absolute path or
/// a path that leaves the directory. `stack shard` writes `prelude.list`,
/// `cpu-prerequisites.list` and `shard-NN.list` beside `plan.json`.
fn shard_list_path(dir: &Path, file: &str) -> Result<PathBuf, Fail> {
    let path = Path::new(file);
    let one_name = !file.is_empty()
        && !file.contains('\0')
        && !file.contains('\\')
        && path.components().count() == 1
        && matches!(
            path.components().next(),
            Some(std::path::Component::Normal(_))
        );
    if !one_name {
        return Err(Fail::new(
            exit::FAILED,
            format!(
                "shard plan file `{file}` is not a file name in {}",
                dir.display()
            ),
        ));
    }
    Ok(dir.join(path))
}

/// Read the units a `stack shard` run wrote into `dir`.
pub fn read_units(dir: &Path) -> Result<Vec<Unit>, Fail> {
    let plan_path = dir.join("plan.json");
    let text = std::fs::read_to_string(&plan_path).map_err(|e| {
        Fail::new(
            exit::FAILED,
            format!("cannot read {}: {e}", plan_path.display()),
        )
    })?;
    let v: serde_json::Value = serde_json::from_str(&text)
        .map_err(|e| Fail::new(exit::FAILED, format!("{}: {e}", plan_path.display())))?;
    let mut units = Vec::new();
    let mut add = |name: String, role: Role, entry: &serde_json::Value| -> Result<(), Fail> {
        let Some(file) = entry["file"].as_str() else {
            return Ok(());
        };
        let path = shard_list_path(dir, file)?;
        let list = std::fs::read_to_string(&path)
            .map_err(|e| Fail::new(exit::FAILED, format!("cannot read {}: {e}", path.display())))?;
        let lines = parse_list(&list)?;
        if lines.is_empty() {
            return Ok(());
        }
        units.push(Unit {
            name,
            role,
            lines,
            footprint: entry["inode_footprint"].as_u64(),
        });
        Ok(())
    };
    add(
        "cpu-prerequisites".into(),
        Role::Cpu,
        &v["cpu_prerequisites"],
    )?;
    add("prelude".into(), Role::Build, &v["prelude"])?;
    if let Some(shards) = v["shards"].as_array() {
        for (i, s) in shards.iter().enumerate() {
            let name = s["name"]
                .as_str()
                .map(str::to_string)
                .unwrap_or_else(|| format!("shard-{:02}", i + 1));
            add(name, Role::Build, s)?;
        }
    }
    Ok(units)
}

#[cfg(test)]
mod read_tests {
    use super::read_units;

    #[test]
    fn a_shard_plan_cannot_name_a_file_outside_its_directory() {
        let root = tempfile::tempdir().unwrap();
        let shard = root.path().join("out");
        std::fs::create_dir(&shard).unwrap();
        std::fs::write(root.path().join("secret.list"), "Secret-1.0.eb\n").unwrap();
        std::fs::write(shard.join("local.list"), "Foo-1.0.eb\n").unwrap();

        std::fs::write(
            shard.join("plan.json"),
            r#"{"shards":[{"name":"s","file":"../secret.list"}]}"#,
        )
        .unwrap();
        let err = read_units(&shard).unwrap_err();
        assert!(err.msg.contains("not a file name"), "{}", err.msg);

        let abs = root.path().join("secret.list");
        std::fs::write(
            shard.join("plan.json"),
            format!(
                r#"{{"shards":[{{"name":"s","file":"{}"}}]}}"#,
                abs.display()
            ),
        )
        .unwrap();
        assert!(read_units(&shard).is_err());

        std::fs::write(
            shard.join("plan.json"),
            r#"{"shards":[{"name":"s","file":"local.list","inode_footprint":3}]}"#,
        )
        .unwrap();
        let units = read_units(&shard).unwrap();
        assert_eq!(units.len(), 1);
        assert_eq!(units[0].lines[0].easyconfig, "Foo-1.0.eb");
        assert_eq!(units[0].footprint, Some(3));
    }
}
