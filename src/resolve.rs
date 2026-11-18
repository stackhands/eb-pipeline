//! Resolution of defaults and overrides into launch lines and environments.

use crate::exit::Fail;
use crate::schema::*;
use crate::util::{package_of, split_args};
use serde_json::{json, Value};
use std::collections::BTreeMap;

/// A phase role: which launch arguments a command takes from
/// `[phase_defaults]` or the target's `phases`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Role {
    /// The planning command, `stack shard`.
    Plan,
    /// The builds of the whole list, the prelude and each shard.
    Build,
    /// Admission, source prefetch, the checks command and log handling.
    Short,
    /// The CPU-only prerequisite builds; the target's `route` wraps these.
    Cpu,
    /// The module cache command of the `caches` phase.
    Cache,
}

impl Role {
    /// Every role, in the order `[phase_defaults]` documents them.
    pub const ALL: [Role; 5] = [Role::Plan, Role::Build, Role::Short, Role::Cpu, Role::Cache];
    /// The role's key in `[phase_defaults]` and `phases`.
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
    /// The site file.
    pub site: &'a Site,
    /// The path of the site file, absolute, exported as `EB_PIPELINE_SITE`
    /// so that a command can run eb-pipeline on the same file.
    pub site_file: Option<String>,
    /// The target the phase runs for.
    pub target: &'a Target,
    /// The build type, which picks the target's `build_types` override.
    pub build_type: Option<&'a str>,
    /// The run id, `--run-id`.
    pub run_id: String,
    /// Timestamp directory name under `filesystems.log_root`.
    pub log_stamp: String,
}

impl<'a> Scope<'a> {
    /// The build type, `default` without one: the `{build}` of job names.
    pub fn build_name(&self) -> &str {
        self.build_type.unwrap_or("default")
    }

    /// `<log_root>/<log stamp>/<target>`, where this run writes its logs.
    pub fn log_dir(&self) -> String {
        format!(
            "{}/{}/{}",
            self.site.filesystems.log_root.trim_end_matches('/'),
            self.log_stamp,
            self.target.name
        )
    }

    /// The target's `software_root`, else `software` under
    /// [`Scope::install_prefix`].
    pub fn software_root(&self) -> String {
        self.target
            .software_root
            .clone()
            .unwrap_or_else(|| format!("{}/software", self.install_prefix()))
    }

    /// The target's `modules_root`, else `modules` under
    /// [`Scope::install_prefix`].
    pub fn modules_root(&self) -> String {
        self.target
            .modules_root
            .clone()
            .unwrap_or_else(|| format!("{}/modules", self.install_prefix()))
    }

    /// eb-staged-rebuild `--ledger-config` of the target.
    pub fn ledger_config(&self) -> Option<String> {
        self.target.ledger_config.clone().or_else(|| {
            self.site
                .rebuild
                .as_ref()
                .and_then(|r| r.ledger_config.clone())
        })
    }

    /// The target's `site_prefix`, else `filesystems.install_root`.
    pub fn install_prefix(&self) -> String {
        self.target
            .site_prefix
            .clone()
            .unwrap_or_else(|| self.site.filesystems.install_root.clone())
    }

    /// EASYBUILD_BUILDPATH of the target.
    pub fn build_path(&self) -> String {
        self.per_target(&self.site.filesystems.build_path)
    }

    /// EASYBUILD_SOURCEPATH entries of the target.
    pub fn source_paths(&self) -> Vec<String> {
        let paths = &self.site.filesystems.source_paths;
        paths.iter().map(|p| self.per_target(p)).collect()
    }

    /// eb-staged-rebuild `--state-root` of the target; `None` leaves the
    /// tool's default.
    pub fn state_root(&self) -> Option<String> {
        let r = self.site.rebuild.as_ref()?;
        r.state_root.as_deref().map(|s| self.per_target(s))
    }

    /// Directory of the target's watch state, when `[watch]` is on.
    pub fn watch_state_dir(&self) -> Option<String> {
        let w = self.site.watch_on()?;
        Some(match &w.state_dir {
            Some(d) => self.per_target(d),
            None => format!(
                "{}/.eb-pipeline-watch",
                self.install_prefix().trim_end_matches('/')
            ),
        })
    }

    /// A site path with `{target}` expanded to the target's name.
    fn per_target(&self, path: &str) -> String {
        path.replace("{target}", &self.target.name)
    }

    /// Install path EESSI-extend sets for a site install when it loads, over
    /// the exported EASYBUILD_INSTALLPATH. EasyBuild prefers the split install
    /// variables to it, so split mode gets no note.
    pub fn eessi_install_note(&self) -> Option<String> {
        let es = self.site.eessi_on()?;
        if !es.site_install || self.site.filesystems.export_install_vars == "split" {
            return None;
        }
        let base = match self.env().get("EESSI_SITE_SOFTWARE_PREFIX") {
            Some(p) => format!("{}/versions", p.trim_end_matches('/')),
            None => "<EESSI repository>/host_injections".into(),
        };
        let cpu = self.target.cpu_target.as_deref().unwrap_or("<cpu subdir>");
        let accel = match self.target.accel_target.as_deref() {
            Some(a) if !a.is_empty() => {
                format!(", plus `/{a}` for an NVIDIA target when EESSI_ACCELERATOR_INSTALL is set")
            }
            _ => String::new(),
        };
        Some(format!(
            "EESSI-extend replaces EASYBUILD_INSTALLPATH when it loads: \
             {base}/<release>/software/linux/{cpu}{accel}"
        ))
    }

    fn type_override(&self) -> Option<&BuildTypeOverride> {
        self.build_type.and_then(|b| self.target.build_types.get(b))
    }

    /// The build type's `eb_optarch`, else the target's.
    pub fn optarch(&self) -> Option<String> {
        self.type_override()
            .and_then(|o| o.eb_optarch.clone())
            .or_else(|| self.target.eb_optarch.clone())
    }

    /// The build type's `max_parallel`, else the target's.
    pub fn max_parallel(&self) -> Option<u32> {
        self.type_override()
            .and_then(|o| o.max_parallel)
            .or(self.target.max_parallel)
    }

    /// The job name of `phase`: `scheduler.job_name_template` with
    /// `{project}`, `{build}`, `{phase}` and `{target}` filled in.
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
        self.launch_routed(role, phase, args, (role == Role::Cpu).then_some(phase))
    }

    /// Like `launch_with`; `route_phase` names the phase to look up in the
    /// route's `phases` list, or `None` for a command that is not routed.
    pub fn launch_routed(
        &self,
        role: Role,
        phase: &str,
        args: Option<&str>,
        route_phase: Option<&str>,
    ) -> Result<Vec<String>, Fail> {
        let mut v = Vec::new();
        if let Some(rp) = route_phase {
            v.extend(self.route_prefix(rp)?);
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
            v.extend(self.job_wrap());
        }
        Ok(v)
    }

    /// Wrapper that resolves `{job_dir}` inside the job, before the module
    /// wrapper and the command, when `filesystems.job_dir` is set.
    pub fn job_wrap(&self) -> Vec<String> {
        if self.site.filesystems.job_dir.is_none() {
            return Vec::new();
        }
        let script = if self.site.resume_on().is_some() {
            JOB_DIR_RESUME_SCRIPT
        } else {
            JOB_DIR_SCRIPT
        };
        vec![
            self.site.modules.shell.clone(),
            "-c".into(),
            script.into(),
            "eb-pipeline".into(),
        ]
    }

    /// The target's route when it applies to `phase`.
    fn active_route(&self, phase: &str) -> Option<&Route> {
        self.target
            .route
            .as_ref()
            .filter(|r| r.kind == "command" && r.phases.iter().any(|p| p == phase))
    }

    fn render_basic(&self, t: &str, phase: &str) -> String {
        let log_dir = self.log_dir();
        t.replace("{phase_dir}", &format!("{log_dir}/{phase}"))
            .replace("{log_dir}", &log_dir)
            .replace("{work_dir}", &self.build_path())
            .replace("{target}", &self.target.name)
            .replace("{phase}", phase)
    }

    fn render_shares(&self, l: &[String], phase: &str) -> String {
        l.iter()
            .map(|s| self.render_basic(s, phase))
            .collect::<Vec<_>>()
            .join(":")
    }

    /// Render a route template: the basic placeholders, then the route's own.
    fn render_route(&self, r: &Route, t: &str, phase: &str) -> String {
        self.render_basic(t, phase)
            .replace("{runtime_root}", r.runtime_root.as_deref().unwrap_or(""))
            .replace("{shares_ro}", &self.render_shares(&r.shares_ro, phase))
            .replace("{shares_rw}", &self.render_shares(&r.shares_rw, phase))
    }

    /// Route command, rendered arguments and the `--` separator.
    fn route_prefix(&self, phase: &str) -> Result<Vec<String>, Fail> {
        let Some(r) = self.active_route(phase) else {
            return Ok(Vec::new());
        };
        let mut v = split_args(r.command.as_deref().unwrap_or(""))?;
        if !r.args.is_empty() {
            v.extend(r.args.iter().map(|a| self.render_route(r, a, phase)));
            v.push("--".into());
        }
        Ok(v)
    }

    /// Environment of a routed command. `digest` is the run-time value of
    /// the receipt digest variable; `None` plans the `${NAME}` placeholder.
    pub fn route_env(&self, phase: &str, digest: Option<&str>) -> BTreeMap<String, String> {
        let mut e = BTreeMap::new();
        let Some(r) = self.active_route(phase) else {
            return e;
        };
        for (k, v) in &r.env {
            e.insert(k.clone(), self.render_route(r, v, phase));
        }
        if let Some(root) = &r.runtime_root {
            e.insert("EB_PIPELINE_ROUTE_RUNTIME_ROOT".into(), root.clone());
        }
        if !r.shares_ro.is_empty() {
            e.insert(
                "EB_PIPELINE_ROUTE_SHARES_RO".into(),
                self.render_shares(&r.shares_ro, phase),
            );
        }
        if !r.shares_rw.is_empty() {
            e.insert(
                "EB_PIPELINE_ROUTE_SHARES_RW".into(),
                self.render_shares(&r.shares_rw, phase),
            );
        }
        if let Some(n) = &r.receipt_digest_env {
            e.insert(
                "EB_PIPELINE_ROUTE_RECEIPT_DIGEST".into(),
                digest.map_or_else(|| format!("${{{n}}}"), str::to_string),
            );
        }
        e
    }

    /// Directories a routed command expects to exist.
    pub fn route_dirs(&self, phase: &str) -> Vec<String> {
        match self.active_route(phase) {
            Some(r)
                if r.args
                    .iter()
                    .chain(&r.shares_ro)
                    .chain(&r.shares_rw)
                    .any(|a| a.contains("{phase_dir}")) =>
            {
                vec![format!("{}/{}", self.log_dir(), phase)]
            }
            _ => Vec::new(),
        }
    }

    /// Hardware class variables of a command, exported only when the target
    /// defines a CPU class.
    pub fn hardware_env(&self, role: Role) -> BTreeMap<String, String> {
        let mut e = BTreeMap::new();
        let Some(cpu) = &self.target.cpu_hardware else {
            return e;
        };
        let hw = if role == Role::Cpu {
            Some(cpu)
        } else {
            self.target.hardware.as_ref()
        };
        if let Some(h) = hw {
            if let Some(v) = &h.vendor_id {
                e.insert("EB_PIPELINE_HW_VENDOR_ID".into(), v.clone());
            }
            if let Some(v) = h.cpu_family {
                e.insert("EB_PIPELINE_HW_CPU_FAMILY".into(), v.to_string());
            }
            if let Some(v) = h.model {
                e.insert("EB_PIPELINE_HW_MODEL".into(), v.to_string());
            }
        }
        e
    }

    /// The target's fetch profile as JSON, for source prefetch commands.
    pub fn fetch_env(&self) -> BTreeMap<String, String> {
        let mut e = BTreeMap::new();
        if let Some(p) = &self.target.fetch_profile {
            if let Ok(j) = serde_json::to_string(p) {
                e.insert("EB_PIPELINE_FETCH_PROFILE".into(), j);
            }
        }
        e
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
        put("EASYBUILD_BUILDPATH", self.build_path());
        if let Some(j) = &fs.job_dir {
            put("EB_PIPELINE_JOB_DIR", j.clone());
        }
        if self.site.resume_on().is_some() {
            if let (Some(repo), Some(_)) = (&fs.configs_repo, &fs.job_dir) {
                put(
                    "EB_PIPELINE_WORKSPACE_REPO",
                    crate::resume::workspace_key(repo),
                );
            }
        }
        if !fs.source_paths.is_empty() {
            put("EASYBUILD_SOURCEPATH", self.source_paths().join(":"));
        }
        if fs.export_install_vars == "split" {
            put("EASYBUILD_INSTALLPATH_SOFTWARE", self.software_root());
            put("EASYBUILD_INSTALLPATH_MODULES", self.modules_root());
        } else {
            put("EASYBUILD_INSTALLPATH", self.install_prefix());
        }
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
            put(
                "EASYBUILD_READ_ONLY_INSTALLDIR",
                fs.read_only_installdir_value.clone(),
            );
        }
        if let Some(c) = &fs.configs_repo {
            put("EASYBUILD_CONFIGS_REPOPATH", c.clone());
        }
        if let Some(f) = &self.site_file {
            put("EB_PIPELINE_SITE", f.clone());
        }
        put("EB_PIPELINE_SITE_NAME", s.site.name.clone());
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
            // The same prefix EasyBuild installs into. An explicit
            // `site_prefix` wins; otherwise this is `filesystems.install_root`,
            // so a site layer and EasyBuild do not install to two places.
            put("EESSI_SITE_SOFTWARE_PREFIX", self.install_prefix());
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
        if let Some(p) = self.target.package_env.get(&name) {
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
/// environment, never through interpolation. The site's init is sourced
/// without `-e` or `-u`, as a login shell sources it: EESSI's Lmod init
/// reads variables it never sets. `module` and the command run under `-e`.
/// The names are split with pathname expansion off, so that `module load`
/// gets each one as written, and back on for `module load` itself.
pub const MODULE_SCRIPT: &str =
    "if [ -n \"${EB_PIPELINE_SHELL_INIT:-}\" ]; then . \"$EB_PIPELINE_SHELL_INIT\"; fi
set -e
if [ -n \"${EB_PIPELINE_UNUSE_PATH:-}\" ]; then MODULEPATH=; export MODULEPATH; fi
module purge
set -f
for m in ${EB_PIPELINE_MODULES:-}; do set +f; module load \"$m\"; done
set +f
exec \"$@\"";

/// Fixed script of the job directory wrapper. The job id and the user are
/// known only inside the job, so `{job}` and `{user}` of
/// EB_PIPELINE_JOB_DIR, then `{job_dir}` of EASYBUILD_BUILDPATH and
/// EASYBUILD_TMPDIR, are replaced there, as text, by a function that a
/// POSIX shell runs.
pub const JOB_DIR_SCRIPT: &str = "sub() {
  r= t=$1
  while :; do
    case $t in
      *\"$2\"*) r=$r${t%%\"$2\"*}$3; t=${t#*\"$2\"} ;;
      *) break ;;
    esac
  done
  printf '%s\\n' \"$r$t\"
}
if [ -z \"${SLURM_JOB_ID:-}\" ]; then
  echo \"eb-pipeline: SLURM_JOB_ID is not set, so filesystems.job_dir names no directory\" >&2
  exit 1
fi
d=$(sub \"$EB_PIPELINE_JOB_DIR\" '{job}' \"$SLURM_JOB_ID\")
d=$(sub \"$d\" '{user}' \"${SLURM_JOB_USER:-$(id -un)}\")
if [ ! -d \"$d\" ] || [ ! -w \"$d\" ]; then
  echo \"eb-pipeline: job $SLURM_JOB_ID has no directory $d that it can write to, which filesystems.job_dir names\" >&2
  exit 1
fi
EB_PIPELINE_JOB_DIR=$d
EASYBUILD_BUILDPATH=$(sub \"$EASYBUILD_BUILDPATH\" '{job_dir}' \"$d\")
EASYBUILD_TMPDIR=$(sub \"$EASYBUILD_TMPDIR\" '{job_dir}' \"$d\")
export EB_PIPELINE_JOB_DIR EASYBUILD_BUILDPATH EASYBUILD_TMPDIR
exec \"$@\"";

/// [`JOB_DIR_SCRIPT`] plus a workspace for a requeued Slurm job.
///
/// When `SLURM_RESTART_COUNT` is not 0 and `EB_PIPELINE_WORKSPACE_REPO` is
/// set, the job's directory becomes `<job_dir>/<repo>.r<count>`. A count of
/// 0, or an empty one, keeps the directory [`JOB_DIR_SCRIPT`] would use.
pub const JOB_DIR_RESUME_SCRIPT: &str = "sub() {
  r= t=$1
  while :; do
    case $t in
      *\"$2\"*) r=$r${t%%\"$2\"*}$3; t=${t#*\"$2\"} ;;
      *) break ;;
    esac
  done
  printf '%s\\n' \"$r$t\"
}
if [ -z \"${SLURM_JOB_ID:-}\" ]; then
  echo \"eb-pipeline: SLURM_JOB_ID is not set, so filesystems.job_dir names no directory\" >&2
  exit 1
fi
d=$(sub \"$EB_PIPELINE_JOB_DIR\" '{job}' \"$SLURM_JOB_ID\")
d=$(sub \"$d\" '{user}' \"${SLURM_JOB_USER:-$(id -un)}\")
if [ ! -d \"$d\" ] || [ ! -w \"$d\" ]; then
  echo \"eb-pipeline: job $SLURM_JOB_ID has no directory $d that it can write to, which filesystems.job_dir names\" >&2
  exit 1
fi
case ${SLURM_RESTART_COUNT:-0} in
  \"\"|0) ;;
  *)
    if [ -n \"${EB_PIPELINE_WORKSPACE_REPO:-}\" ]; then
      d=$d/$EB_PIPELINE_WORKSPACE_REPO.r$SLURM_RESTART_COUNT
      if ! mkdir -p \"$d\"; then
        echo \"eb-pipeline: job $SLURM_JOB_ID cannot create $d, which filesystems.job_dir names for a requeued job\" >&2
        exit 1
      fi
      if [ ! -w \"$d\" ]; then
        echo \"eb-pipeline: job $SLURM_JOB_ID has no directory $d that it can write to, which filesystems.job_dir names\" >&2
        exit 1
      fi
    fi
    ;;
esac
EB_PIPELINE_JOB_DIR=$d
EASYBUILD_BUILDPATH=$(sub \"$EASYBUILD_BUILDPATH\" '{job_dir}' \"$d\")
EASYBUILD_TMPDIR=$(sub \"$EASYBUILD_TMPDIR\" '{job_dir}' \"$d\")
export EB_PIPELINE_JOB_DIR EASYBUILD_BUILDPATH EASYBUILD_TMPDIR
exec \"$@\"";

/// The first line of `log` in which [`JOB_DIR_SCRIPT`] says why it started no
/// command, trimmed.
pub fn job_dir_refusal(log: &str) -> Option<&str> {
    log.lines()
        .map(str::trim)
        .find(|l| l.contains("eb-pipeline: ") && l.contains("filesystems.job_dir names"))
}

/// Shells that `modules.shell` may name, by name or by path. The wrapper
/// needs a POSIX shell, which splits an unquoted expansion at white space and
/// takes `set -f`: zsh does neither, and csh, tcsh and fish cannot parse it.
pub const MODULE_SHELLS: &[&str] = &["sh", "bash", "dash", "ash", "ksh", "ksh93", "mksh", "yash"];

/// Group that applies to a package, if any. A version that does not parse
/// gets the group (fail closed).
pub fn licence_group(l: &Licences, name: &str, version: Option<&str>) -> Option<String> {
    let g = l.groups.get(name)?;
    if let Some(limit) = l.below_version.get(name) {
        let lim = crate::util::parse_version(limit)?;
        if let Some(v) = version.and_then(crate::util::parse_version) {
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
/// `site_file` is the `EB_PIPELINE_SITE` of the environment it shows.
pub fn explain(
    site: &Site,
    site_file: Option<&str>,
    target: Option<&str>,
    build_type: Option<&str>,
) -> Result<Value, Fail> {
    let selected: Vec<&Target> = match target {
        Some(t) => vec![site.target(t)?],
        None => site.targets.iter().collect(),
    };
    let mut targets = Vec::new();
    for t in selected {
        let scope = Scope {
            site,
            site_file: site_file.map(str::to_string),
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
                    "launch": scope.launch_routed(
                        role,
                        role.name(),
                        None,
                        (role == Role::Cpu).then_some("build"),
                    )?,
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
            "install_note": scope.eessi_install_note(),
            "staged": t.staged,
            "state_root": scope.state_root(),
            "ledger_config": scope.ledger_config(),
            "route": t.route,
            "select": t.select,
            "hardware": t.hardware,
            "cpu_hardware": t.cpu_hardware,
            "fetch_profile": t.fetch_profile,
            "package_env": if t.package_env.is_empty() { Value::Null } else { json!(t.package_env) },
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_unparsed_package_version_keeps_its_licence_group() {
        let licences: Licences = toml::from_str(
            r#"
[groups]
Foo = "licensed"
[below_version]
Foo = "2"
"#,
        )
        .unwrap();
        assert_eq!(
            licence_group(&licences, "Foo", Some("1.9")),
            Some("licensed".into())
        );
        assert_eq!(licence_group(&licences, "Foo", Some("2")), None);
        assert_eq!(
            licence_group(&licences, "Foo", Some("2rc1")),
            Some("licensed".into())
        );
    }

    #[test]
    fn the_first_refusal_of_the_job_directory_wrapper_is_found_trimmed() {
        let refusals: Vec<&str> = JOB_DIR_SCRIPT
            .lines()
            .filter(|l| l.contains(">&2"))
            .collect();
        assert_eq!(refusals.len(), 2, "{refusals:?}");
        for l in refusals {
            assert!(job_dir_refusal(l).is_some(), "{l}");
        }
        let log = "srun: error: node01: task 0: Exited with exit code 1\n  eb-pipeline: job 6 has no directory /local/alice.6 that it can write to, which filesystems.job_dir names \r\neb-pipeline: SLURM_JOB_ID is not set, so filesystems.job_dir names no directory\n";
        assert_eq!(
            job_dir_refusal(log),
            Some("eb-pipeline: job 6 has no directory /local/alice.6 that it can write to, which filesystems.job_dir names")
        );
        assert_eq!(
            job_dir_refusal("srun: error: node01: task 0: Exited with exit code 1\n"),
            None
        );
    }

    #[test]
    fn a_requeued_job_uses_a_repository_qualified_workspace() {
        let root = tempfile::tempdir().unwrap();
        let parent = root.path().join("alice.9");
        std::fs::create_dir(&parent).unwrap();
        let run = |restart: &str| {
            std::process::Command::new("sh")
                .arg("-c")
                .arg(JOB_DIR_RESUME_SCRIPT)
                .arg("eb-pipeline")
                .arg("sh")
                .arg("-c")
                .arg("printf %s \"$EASYBUILD_BUILDPATH\"")
                .env("SLURM_JOB_ID", "9")
                .env("SLURM_JOB_USER", "alice")
                .env("SLURM_RESTART_COUNT", restart)
                .env(
                    "EB_PIPELINE_JOB_DIR",
                    format!("{}/{{user}}.{{job}}", root.path().display()),
                )
                .env("EB_PIPELINE_WORKSPACE_REPO", "opt_easyconfigs")
                .env("EASYBUILD_BUILDPATH", "{job_dir}/build")
                .env("EASYBUILD_TMPDIR", "{job_dir}/tmp")
                .output()
                .unwrap()
        };
        let first = run("0");
        assert_eq!(
            first.status.code(),
            Some(0),
            "{}",
            String::from_utf8_lossy(&first.stderr)
        );
        assert_eq!(
            String::from_utf8_lossy(&first.stdout),
            format!("{}/build", parent.display())
        );
        assert!(!parent.join("opt_easyconfigs.r0").exists());
        let second = run("2");
        assert_eq!(
            second.status.code(),
            Some(0),
            "{}",
            String::from_utf8_lossy(&second.stderr)
        );
        let qualified = parent.join("opt_easyconfigs.r2");
        assert!(qualified.is_dir(), "{}", qualified.display());
        assert_eq!(
            String::from_utf8_lossy(&second.stdout),
            format!("{}/build", qualified.display())
        );
    }
}
