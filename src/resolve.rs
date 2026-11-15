//! Resolution of defaults and overrides into launch lines and environments.

use crate::exit::Fail;
use crate::schema::*;
use crate::util::{package_of, split_args};
use serde_json::{json, Value};
use std::collections::BTreeMap;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Role {
    Plan,
    Build,
    Short,
    Cpu,
    Cache,
}

impl Role {
    pub const ALL: [Role; 5] = [Role::Plan, Role::Build, Role::Short, Role::Cpu, Role::Cache];
    pub fn name(self) -> &'static str {
        match self {
            Role::Plan => "plan",
            Role::Build => "build",
            Role::Short => "short",
            Role::Cpu => "cpu",
            Role::Cache => "cache",
        }
    }
}

fn pick(a: &Option<PhaseArgs>, role: Role) -> Option<String> {
    let a = a.as_ref()?;
    match role {
        Role::Plan => a.plan.clone(),
        Role::Build => a.build.clone(),
        Role::Short => a.short.clone(),
        Role::Cpu => a.cpu.clone(),
        Role::Cache => a.cache.clone(),
    }
}

/// Launch arguments of a role: the target override, else the default.
pub fn role_args(site: &Site, target: &Target, role: Role) -> Option<String> {
    pick(&target.phases, role).or_else(|| pick(&site.phase_defaults, role))
}

/// Everything a phase needs to know about where it runs.
#[derive(Clone)]
pub struct Scope<'a> {
    pub site: &'a Site,
    pub target: &'a Target,
    pub build_type: Option<&'a str>,
    pub run_id: String,
    /// Timestamp directory name under `filesystems.log_root`.
    pub log_stamp: String,
}

impl<'a> Scope<'a> {
    pub fn build_name(&self) -> &str {
        self.build_type.unwrap_or("default")
    }

    pub fn log_dir(&self) -> String {
        format!(
            "{}/{}/{}",
            self.site.filesystems.log_root.trim_end_matches('/'),
            self.log_stamp,
            self.target.name
        )
    }

    pub fn software_root(&self) -> String {
        self.target
            .software_root
            .clone()
            .unwrap_or_else(|| format!("{}/software", self.install_prefix()))
    }

    pub fn modules_root(&self) -> String {
        self.target
            .modules_root
            .clone()
            .unwrap_or_else(|| format!("{}/modules", self.install_prefix()))
    }

    pub fn install_prefix(&self) -> String {
        self.target
            .site_prefix
            .clone()
            .unwrap_or_else(|| self.site.filesystems.install_root.clone())
    }

    fn type_override(&self) -> Option<&BuildTypeOverride> {
        self.build_type.and_then(|b| self.target.build_types.get(b))
    }

    pub fn optarch(&self) -> Option<String> {
        self.type_override()
            .and_then(|o| o.eb_optarch.clone())
            .or_else(|| self.target.eb_optarch.clone())
    }

    pub fn max_parallel(&self) -> Option<u32> {
        self.type_override()
            .and_then(|o| o.max_parallel)
            .or(self.target.max_parallel)
    }

    pub fn job_name(&self, phase: &str) -> String {
        self.site
            .scheduler
            .job_name_template
            .replace("{project}", self.site.project())
            .replace("{build}", self.build_name())
            .replace("{phase}", phase)
            .replace("{target}", &self.target.name)
    }

    /// Launch prefix of a phase: route wrapper (CPU role), launch command,
    /// common arguments, job name, role arguments.
    pub fn launch(&self, role: Role, phase: &str) -> Result<Vec<String>, Fail> {
        self.launch_with(role, phase, None)
    }

    /// Like `launch`, with the role arguments replaced by `args`.
    pub fn launch_with(
        &self,
        role: Role,
        phase: &str,
        args: Option<&str>,
    ) -> Result<Vec<String>, Fail> {
        let mut v = Vec::new();
        if role == Role::Cpu {
            if let Some(r) = &self.target.route {
                if r.kind == "command" {
                    v.extend(split_args(r.command.as_deref().unwrap_or(""))?);
                }
            }
        }
        if self.site.scheduler.kind == "slurm" {
            v.extend(split_args(&self.site.scheduler.launch)?);
            v.extend(self.site.scheduler.common_args.iter().cloned());
            v.push(format!("--job-name={}", self.job_name(phase)));
            let a = args
                .map(str::to_string)
                .or_else(|| role_args(self.site, self.target, role));
            if let Some(a) = a {
                v.extend(split_args(&a)?);
            }
        }
        Ok(v)
    }

    /// Environment exported to every phase.
    pub fn env(&self) -> BTreeMap<String, String> {
        let s = self.site;
        let mut e: BTreeMap<String, String> = s.scheduler.env.clone();
        let fs = &s.filesystems;
        let mut put = |k: &str, v: String| {
            e.insert(k.to_string(), v);
        };
        put("EASYBUILD_TMPDIR", fs.tmp.clone());
        put("EASYBUILD_BUILDPATH", fs.build_path.clone());
        if !fs.source_paths.is_empty() {
            put("EASYBUILD_SOURCEPATH", fs.source_paths.join(":"));
        }
        put("EASYBUILD_INSTALLPATH", self.install_prefix());
        if let Some(o) = self.optarch() {
            put("EASYBUILD_OPTARCH", o);
        }
        if let Some(p) = self.max_parallel() {
            put("EASYBUILD_MAX_PARALLEL", p.to_string());
        }
        if let Some(c) = &self.target.cuda_compute_capabilities {
            put("EASYBUILD_CUDA_COMPUTE_CAPABILITIES", c.clone());
        }
        if fs.read_only_installdir {
            put("EASYBUILD_READ_ONLY_INSTALLDIR", "1".into());
        }
        if let Some(c) = &fs.configs_repo {
            put("EASYBUILD_CONFIGS_REPOPATH", c.clone());
        }
        put("EB_PIPELINE_SITE", s.site.name.clone());
        put("EB_PIPELINE_TARGET", self.target.name.clone());
        put("EB_PIPELINE_BUILD_TYPE", self.build_name().to_string());
        put("EB_PIPELINE_RUN_ID", self.run_id.clone());
        put("EB_PIPELINE_LOG_STAMP", self.log_stamp.clone());
        put("EB_PIPELINE_LOG_DIR", self.log_dir());
        if let Some(a) = &s.site.build_account {
            put("EB_PIPELINE_BUILD_ACCOUNT", a.clone());
        }
        if let Some(g) = &s.site.default_group {
            put("EB_PIPELINE_DEFAULT_GROUP", g.clone());
        }
        if !s.modules.init.is_empty() {
            put("EB_PIPELINE_MODULES", s.modules.init.join(" "));
        }
        if s.modules.unuse_path {
            put("EB_PIPELINE_UNUSE_PATH", "1".into());
        }
        if let Some(f) = &s.modules.shell_init {
            put("EB_PIPELINE_SHELL_INIT", f.clone());
        }
        if let Some(es) = s.eessi_on() {
            if es.site_install {
                put("EESSI_SITE_INSTALL", "1".into());
            }
            if let Some(b) = &es.bootstrap_modulepath {
                put("EESSI_SITE_BOOTSTRAP_MODULEPATH", b.clone());
            }
            if let Some(c) = &self.target.cpu_target {
                put(&es.cpu_override_var, c.clone());
            }
            if let Some(a) = &self.target.accel_target {
                put(&es.accel_override_var, a.clone());
            }
            if let Some(p) = &self.target.site_prefix {
                put("EESSI_SITE_SOFTWARE_PREFIX", p.clone());
            }
        }
        e
    }

    /// Environment added for one build list line.
    pub fn line_env(&self, easyconfig: &str) -> BTreeMap<String, String> {
        let (name, version) = package_of(easyconfig);
        let mut e = BTreeMap::new();
        if let Some(p) = self.site.modules.package_env.get(&name) {
            e.extend(p.clone());
        }
        if let Some(l) = self.site.licences_on() {
            if let Some(p) = l.env.get(&name) {
                e.extend(p.clone());
            }
            if let Some(g) = licence_group(l, &name, version.as_deref()) {
                e.insert("EB_PIPELINE_LICENCE_GROUP".into(), g);
            }
        }
        e
    }

    /// Wrapper that loads modules before the wrapped command runs.
    pub fn module_wrap(&self) -> Vec<String> {
        let m = &self.site.modules;
        if m.init.is_empty() && !m.unuse_path && m.shell_init.is_none() {
            return Vec::new();
        }
        vec![
            m.shell.clone(),
            "-c".into(),
            MODULE_SCRIPT.into(),
            "eb-pipeline".into(),
        ]
    }
}

/// Fixed script of the module wrapper; module names arrive through the
/// environment, never through interpolation.
pub const MODULE_SCRIPT: &str = "set -eu
if [ -n \"${EB_PIPELINE_SHELL_INIT:-}\" ]; then . \"$EB_PIPELINE_SHELL_INIT\"; fi
if [ -n \"${EB_PIPELINE_UNUSE_PATH:-}\" ]; then MODULEPATH=; export MODULEPATH; fi
module purge
for m in ${EB_PIPELINE_MODULES:-}; do module load \"$m\"; done
exec \"$@\"";

/// Group that applies to a package, if any. A version that does not parse
/// gets the group (fail closed).
pub fn licence_group(l: &Licences, name: &str, version: Option<&str>) -> Option<String> {
    let g = l.groups.get(name)?;
    if let Some(limit) = l.below_version.get(name) {
        let lim = crate::util::parse_version(limit)?;
        if let Some(v) = version.and_then(|v| {
            let core: String = v
                .chars()
                .take_while(|c| c.is_ascii_digit() || *c == '.')
                .collect();
            crate::util::parse_version(core.trim_end_matches('.'))
        }) {
            if crate::util::cmp_version(&v, &lim) != std::cmp::Ordering::Less {
                return None;
            }
        }
    }
    Some(g.clone())
}

fn strip_nulls(v: Value) -> Value {
    match v {
        Value::Object(m) => Value::Object(
            m.into_iter()
                .filter(|(_, v)| !v.is_null())
                .map(|(k, v)| (k, strip_nulls(v)))
                .collect(),
        ),
        Value::Array(a) => Value::Array(a.into_iter().map(strip_nulls).collect()),
        o => o,
    }
}

/// Resolved view of one or all targets after defaults and overrides.
pub fn explain(site: &Site, target: Option<&str>, build_type: Option<&str>) -> Result<Value, Fail> {
    let selected: Vec<&Target> = match target {
        Some(t) => vec![site.target(t)?],
        None => site.targets.iter().collect(),
    };
    let mut targets = Vec::new();
    for t in selected {
        let scope = Scope {
            site,
            target: t,
            build_type,
            run_id: "<run-id>".into(),
            log_stamp: "<log-stamp>".into(),
        };
        let mut phases = serde_json::Map::new();
        for role in Role::ALL {
            phases.insert(
                role.name().into(),
                json!({
                    "args": role_args(site, t, role),
                    "launch": scope.launch(role, role.name())?,
                }),
            );
        }
        targets.push(json!({
            "name": t.name,
            "default": t.default,
            "build_lists": t.build_lists,
            "eb_optarch": scope.optarch(),
            "max_parallel": scope.max_parallel(),
            "cuda_compute_capabilities": t.cuda_compute_capabilities,
            "cpu_target": t.cpu_target,
            "accel_target": t.accel_target,
            "site_prefix": scope.install_prefix(),
            "software_root": scope.software_root(),
            "modules_root": scope.modules_root(),
            "staged": t.staged,
            "route": t.route,
            "hardware": t.hardware,
            "phases": phases,
            "env": scope.env(),
        }));
    }
    Ok(strip_nulls(json!({
        "site": {
            "name": site.site.name,
            "project": site.project(),
        },
        "build_type": build_type,
        "scheduler": {
            "kind": site.scheduler.kind,
            "launch": site.scheduler.launch,
            "common_args": site.scheduler.common_args,
            "job_name_template": site.scheduler.job_name_template,
        },
        "targets": targets,
    })))
}
