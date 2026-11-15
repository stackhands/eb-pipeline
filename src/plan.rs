//! Plan construction: phases, launch lines, environments, tool calls.
//!
//! The planner is pure. `run` executes the commands built here.

use crate::exit::Fail;
use crate::resolve::{Role, Scope};
use crate::tools::Tool;
use crate::util::ListLine;
use serde::Serialize;
use std::collections::BTreeMap;

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
    pub phases: Vec<PlanPhase>,
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

pub struct Planner<'a> {
    pub scope: Scope<'a>,
    pub list_path: String,
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
        Planner { scope, list_path }
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
        let mut v = self.scope.launch(role, phase)?;
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

    pub fn shard_cmd(&self) -> Result<Cmd, Fail> {
        let s = self.site().shards_on().expect("shards active");
        let mut a = vec![
            Tool::EbStack.bin(self.site()),
            "stack".into(),
            "shard".into(),
            "--build-list".into(),
            self.list_path.clone(),
            "--shards".into(),
            s.count.to_string(),
            "--out-dir".into(),
            self.shard_dir(),
        ];
        for e in &s.easyconfigs {
            a.push("--easyconfigs".into());
            a.push(e.clone());
        }
        for d in &s.installed_modules_dirs {
            a.push("--installed-modules-dir".into());
            a.push(d.clone());
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
            argv: self.wrapped(Role::Plan, "shard", a, false)?,
            env: BTreeMap::new(),
            note: Some(format!(
                "exit {} means the plan was built unsharded and the list builds as one unit",
                s.planner_exit_unsharded
            )),
        })
    }

    pub fn prefetch_cmd(&self, line: &ListLine, name: &str) -> Result<Cmd, Fail> {
        let extra = self
            .site()
            .sources_on()
            .map(|s| s.extra_args.clone())
            .unwrap_or_default();
        let mut a = vec![
            "eb".to_string(),
            line.easyconfig.clone(),
            "--fetch".into(),
            "--robot".into(),
        ];
        a.extend(
            line.args
                .iter()
                .filter(|x| !x.starts_with("--rebuild"))
                .cloned(),
        );
        a.extend(extra);
        Ok(Cmd {
            name: format!("fetch:{name}"),
            argv: self.wrapped(Role::Short, "sources", a, true)?,
            env: self.scope.line_env(&line.easyconfig),
            note: None,
        })
    }

    pub fn admission_cmd(&self, unit: &str, footprint: u64) -> Result<Cmd, Fail> {
        let adm = self.site().admission_on().expect("admission active");
        let job = format!("{}-{}", self.scope.run_id, unit);
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

    /// Whether a line replaces a live install and goes through staged rebuild.
    pub fn is_staged(&self, line: &ListLine) -> bool {
        self.site().rebuild_on().is_some()
            && line
                .args
                .iter()
                .any(|a| a == "--rebuild" || a == "--force" || a.starts_with("--rebuild="))
    }

    pub fn log_stdout(&self, stem: &str) -> String {
        format!("{}/{}.stdout.log", self.scope.log_dir(), stem)
    }

    pub fn tmp_logdir(&self, stem: &str) -> String {
        format!("{}/easybuild-logs/{}", self.scope.log_dir(), stem)
    }

    pub fn build_cmd(&self, role: Role, line: &ListLine, stem: &str) -> Result<Cmd, Fail> {
        let mut inner: Vec<String>;
        let mut note = None;
        if self.is_staged(line) {
            let r = self.site().rebuild_on().expect("rebuild active");
            let prefix = self.scope.install_prefix();
            inner = vec![
                Tool::EbStagedRebuild.bin(self.site()),
                "run".into(),
                "--software".into(),
                format!("{prefix}/software"),
                "--modules".into(),
                format!("{prefix}/modules"),
                "--backend".into(),
                r.backend.clone(),
                "--run-id".into(),
                self.scope.run_id.clone(),
            ];
            if let Some(s) = &r.state_root {
                inner.push("--state-root".into());
                inner.push(s.clone());
            }
            if let Some(g) = r.grace_hours {
                inner.push("--grace-hours".into());
                inner.push(g.to_string());
            }
            if let Some(p) = &r.prepare {
                inner.push("--prepare".into());
                inner.push(p.clone());
            }
            if let Some(c) = &r.ledger_config {
                inner.push("--ledger-config".into());
                inner.push(c.clone());
            }
            inner.push("--".into());
            note = Some("staged rebuild: exit 2 is a refusal, 75 is pending".to_string());
        } else {
            inner = vec!["eb".into()];
        }
        inner.push(line.easyconfig.clone());
        inner.extend(line.args.iter().cloned());
        if self.site().logs_on().is_some_and(|l| l.keep_failed) {
            inner.push(format!("--tmp-logdir={}", self.tmp_logdir(stem)));
        }
        Ok(Cmd {
            name: format!("build:{stem}"),
            argv: self.wrapped(role, "build", inner, true)?,
            env: self.scope.line_env(&line.easyconfig),
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
        Ok(Cmd {
            name: format!("keep-failed:{stem}"),
            argv: self.wrapped(Role::Short, "logs", a, true)?,
            env: BTreeMap::new(),
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
        ];
        if let Some(d) = r.compress_after_days {
            a.push("--compress-after".into());
            a.push(d.to_string());
        }
        if let Some(d) = r.delete_after_days {
            a.push("--delete-after".into());
            a.push(d.to_string());
        }
        Ok(Some(Cmd {
            name: "rotate".into(),
            argv: self.wrapped(Role::Short, "logs", a, false)?,
            env: BTreeMap::new(),
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
        if let Some(m) = &entry.init_module {
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
pub fn build_plan(p: &Planner, lines: &[ListLine]) -> Result<Plan, Fail> {
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

    let stems = unique_stems(lines);
    phases.push(if site.sources_on().is_some() {
        let mut cmds = Vec::new();
        for (l, s) in lines.iter().zip(&stems) {
            cmds.push(p.prefetch_cmd(l, s)?);
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
        Some(a) => planned(
            "admission",
            Some(Role::Short),
            vec![p.admission_cmd("all", a.unknown_reserve.unwrap_or(0))?],
        ),
    });

    let mut cmds = Vec::new();
    for (l, s) in lines.iter().zip(&stems) {
        cmds.push(p.build_cmd(Role::Build, l, s)?);
        if site.logs_on().is_some_and(|l| l.keep_failed) {
            cmds.push(p.keep_failed_cmd(s, "<exit-status>")?);
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
        phases,
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
