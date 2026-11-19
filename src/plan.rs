//! Plan construction: phases, launch lines, environments, tool calls.
//!
//! The planner is pure. `run` executes the commands built here.

use crate::easystack::is_easystack;
use crate::exit::{self, Fail};
use crate::package_plan::{Input as PackagePlanInput, Verification, R_DESCRIPTION_CHECK};
use crate::resolve::{Role, Scope};
use crate::tools::Tool;
use crate::util::{list_word, parse_list, quote, read_text_file, ListLine};
use serde::Serialize;
use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};

/// One subprocess the driver starts.
#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct Cmd {
    /// What it is: `probe:<tool>`, `shard`, `fetch:<stem>`, `admit:<unit>`,
    /// `build:<stem>`, `keep-failed:<stem>`, `check`, `cache` or `rotate`.
    pub name: String,
    /// Full argument vector: launch prefix, module wrapper, tool.
    pub argv: Vec<String>,
    /// Environment added to the phase environment.
    #[serde(skip_serializing_if = "BTreeMap::is_empty")]
    pub env: BTreeMap<String, String>,
    /// What to know about it, such as what its exit codes mean.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub note: Option<String>,
}

/// One phase of a plan.
#[derive(Debug, Clone, Serialize)]
pub struct PlanPhase {
    /// The phase, one of [`PHASES`].
    pub name: String,
    /// `planned` or `not configured`; `refused` for an admission that
    /// `admission.required` asks for and the site file cannot provide.
    pub status: String,
    /// Why the phase is not planned.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub reason: Option<String>,
    /// The [`Role`] its commands take their launch arguments from.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub role: Option<String>,
    /// The phase environment, which every command gets.
    pub env: BTreeMap<String, String>,
    /// The commands, in the order `run` starts them.
    pub commands: Vec<Cmd>,
}

/// The plan file `plan` writes: every phase of a run, with its commands.
#[derive(Debug, Clone, Serialize)]
pub struct Plan {
    /// Version of this format, 1.
    pub schema_version: u32,
    /// `plan`.
    pub kind: String,
    /// `site.name`.
    pub site: String,
    /// The target's name.
    pub target: String,
    /// The build type, `default` without one.
    pub build_type: String,
    /// The run id.
    pub run_id: String,
    /// The build list, as given.
    pub list: String,
    /// SHA-256 of the eb-stack `package.plan.json`, for a package-plan input.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub package_plan_digest: Option<String>,
    /// Where the run writes its logs.
    pub log_dir: String,
    /// The build list's lines, without blank lines and comments.
    pub lines: usize,
    /// Lines kept by the target's selector; absent without `[targets.select]`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub selected: Option<usize>,
    /// File the selected lines are written to, which `stack shard` reads.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub selected_list: Option<String>,
    /// Every phase, in [`PHASES`] order, planned or not.
    pub phases: Vec<PlanPhase>,
    /// Lines left out by the target's selector.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub deselected: Vec<Deselected>,
    /// Checkpoint and inclusion proof of the input record, when `[attest]` is
    /// enabled. Absent, the plan JSON is unchanged.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub input_attestation: Option<crate::attest::InputAttestation>,
}

/// A line that the target's selector left out.
#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct Deselected {
    /// The line's easyconfig.
    pub easyconfig: String,
    /// `all`: the selector reads the whole list, before `stack shard`.
    pub unit: String,
}

/// Every phase, in the order a run goes through them.
pub const PHASES: [&str; 10] = [
    "preflight",
    "licences",
    "shard",
    "sources",
    "admission",
    "build",
    "checks",
    "verification",
    "caches",
    "logs",
];

/// A build unit: the whole list, or one shard list.
#[derive(Debug, Clone)]
pub struct Unit {
    /// `all`, `cpu-prerequisites`, `prelude`, or the shard's name.
    pub name: String,
    /// [`Role::Cpu`] for the CPU prerequisites, else [`Role::Build`].
    pub role: Role,
    /// Its build list lines, in order.
    pub lines: Vec<ListLine>,
    /// Inode footprint from the shard plan.
    pub footprint: Option<u64>,
}

/// Builds the commands of one target's run of a build list.
#[derive(Clone)]
pub struct Planner<'a> {
    /// Where the phases run.
    pub scope: Scope<'a>,
    /// The build list, as given.
    pub list_path: String,
    /// Scheduler job id read at run time; `None` plans the `${NAME}` placeholder.
    pub job_id: Option<String>,
    /// Value of the route's receipt digest variable read at run time.
    pub route_digest: Option<String>,
    /// Values of the `installed_modules_dirs_from_env` variables read at run
    /// time; `None` plans the `${NAME}` placeholders and keeps every
    /// installed-modules directory.
    pub runtime_env: Option<BTreeMap<String, String>>,
    /// Bundle directory requested by `--offline-bundle`.
    pub offline_bundle: Option<PathBuf>,
    /// Network-isolating command requested by `--require-offline`.
    pub offline_wrapper: Vec<String>,
    /// Validated eb-stack package-plan input, when one was selected.
    pub package_plan: Option<PackagePlanInput>,
    /// Whether to run the package plan's ecosystem-native checks.
    pub verify_package_plan: bool,
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

/// Append EasyBuild's options and one easyconfig operand. A leading-dash
/// operand follows `--`; ordinary lines retain their historical order.
fn push_eb_line(argv: &mut Vec<String>, easyconfig: &str, options: Vec<String>) {
    if easyconfig.starts_with('-') {
        argv.extend(options);
        argv.push("--".into());
        argv.push(easyconfig.into());
    } else {
        argv.push(easyconfig.into());
        argv.extend(options);
    }
}

/// Line options for a fetch with the robot disabled: a bare `--robot` goes,
/// and the last `--robot P` or `--robot=P` path stays at the front of the
/// search path so the line's easyconfig is still found. EasyBuild searches `P` then
/// `--robot-paths`, which `--robot-paths=P:X` keeps; `P:` is `P` followed by
/// the default paths.
pub fn without_robot<'a>(args: impl Iterator<Item = &'a String>) -> Vec<String> {
    let mut robot: Option<&str> = None;
    let mut out: Vec<String> = Vec::new();
    let mut args = args.peekable();
    while let Some(a) = args.next() {
        if a == "--robot" {
            robot = match args.peek() {
                Some(path) if !path.starts_with('-') => {
                    Some(args.next().expect("peeked at the path").as_str())
                }
                _ => Some(""),
            };
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
    /// The log stem of the next line with `easyconfig`: its file name without
    /// `.eb`, then `.2`, `.3` and so on for later lines with the same name.
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
    /// A planner without run-time values, which plans placeholders for them.
    pub fn new(scope: Scope<'a>, list_path: String) -> Self {
        Planner {
            scope,
            list_path,
            job_id: None,
            route_digest: None,
            runtime_env: None,
            offline_bundle: None,
            offline_wrapper: Vec::new(),
            package_plan: None,
            verify_package_plan: false,
        }
    }

    /// Apply the target's selector to the list lines, in order. Kept lines
    /// carry the arguments `strip` leaves; the others are recorded. Without a
    /// selector every line is kept as read and nothing is recorded. A stripped
    /// line that cannot be written as one build list line is an error.
    pub fn select_lines(
        &self,
        lines: &[ListLine],
    ) -> Result<(Vec<ListLine>, Vec<Deselected>), Fail> {
        let Some(sel) = &self.scope.target.select else {
            return Ok((lines.to_vec(), Vec::new()));
        };
        let mut kept = Vec::new();
        let mut out = Vec::new();
        for l in lines {
            let r = crate::select::select_line(sel, l)?;
            if r.planned {
                let raw = if sel.strip {
                    let mut words = Vec::new();
                    for a in std::iter::once(&l.easyconfig).chain(&r.args) {
                        words.push(list_word(a)?);
                    }
                    words.join(" ")
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
        Ok((kept, out))
    }

    /// Where the selected build list lives, when the target has a selector.
    pub fn selected_list_path(&self) -> Option<PathBuf> {
        self.scope.target.select.as_ref()?;
        Some(
            PathBuf::from(self.scope.log_dir())
                .join(format!("selected-{}.list", self.scope.target.name)),
        )
    }

    /// Where a run writes generated build-list lines, which `stack shard`
    /// reads: package-plan recipes or lines read from an easystack. With a
    /// selector, the selected list holds them instead.
    pub fn easystack_list_path(&self) -> Option<PathBuf> {
        if self.scope.target.select.is_some() {
            return None;
        }
        let stem = if self.package_plan.is_some() {
            "package-plan"
        } else if is_easystack(Path::new(&self.list_path)) {
            "easystack"
        } else {
            return None;
        };
        Some(
            PathBuf::from(self.scope.log_dir())
                .join(format!("{stem}-{}.list", self.scope.target.name)),
        )
    }

    /// The list `stack shard` reads: the selected list when there is one,
    /// else the lines read from an easystack, else the list as given.
    fn shard_list(&self) -> String {
        match self
            .selected_list_path()
            .or_else(|| self.easystack_list_path())
        {
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
        v.extend(self.offline_inner(phase, inner));
        Ok(v)
    }

    fn offline_inner(&self, phase: &str, inner: Vec<String>) -> Vec<String> {
        if self.offline_wrapper.is_empty()
            || !matches!(phase, "admission" | "build" | "checks" | "caches" | "logs")
        {
            return inner;
        }
        let mut out = self.offline_wrapper.clone();
        out.extend(inner);
        out
    }

    /// `<shard_dir>/<run-id>`, where `stack shard` writes its plan;
    /// `filesystems.shard_dir` defaults to `<scratch>/shards`.
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

    /// The `stack shard` command of the shard phase, for a site with
    /// `[shards]` active.
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

    /// `stack order --hashes-out` for the lines a resume compares.
    ///
    /// The digest is the hash file `eb-stack` writes. This command does not
    /// hash easyconfigs itself.
    pub fn order_cmd(&self, lines: &[ListLine]) -> Result<Cmd, Fail> {
        let trees = crate::resume::easyconfig_trees(self.site());
        if trees.is_empty() {
            return Err(Fail::usage(
                "[resume] needs easyconfigs, on resume.easyconfigs or shards.easyconfigs",
            ));
        }
        let roots = crate::resume::roots(lines);
        if roots.is_empty() {
            return Err(Fail::usage(
                "[resume] found no package name to pass to stack order --root",
            ));
        }
        let dir = self.scope.log_dir();
        let mut a = vec![
            Tool::EbStack.bin(self.site()),
            "stack".into(),
            "order".into(),
            "--out".into(),
            crate::resume::order_path(&dir).display().to_string(),
            "--hashes-out".into(),
            crate::resume::hashes_path(&dir).display().to_string(),
        ];
        for tree in trees {
            a.push("--easyconfigs".into());
            a.push(tree);
        }
        for root in roots {
            a.push("--root".into());
            a.push(root);
        }
        Ok(Cmd {
            name: "order".into(),
            argv: self.wrapped(Role::Plan, "shard", a, true)?,
            env: BTreeMap::new(),
            note: Some(
                "input identity from stack order --hashes-out; a mismatch refuses --resume-result"
                    .into(),
            ),
        })
    }

    /// `stack shard` for restart boundaries when `[shards]` is not planning.
    pub fn boundary_shard_cmd(&self) -> Result<Cmd, Fail> {
        let r = self.site().resume_on().expect("resume active");
        let trees = crate::resume::easyconfig_trees(self.site());
        let mut a = vec![
            Tool::EbStack.bin(self.site()),
            "stack".into(),
            "shard".into(),
            "--build-list".into(),
            self.shard_list(),
            "--shards".into(),
            r.shards.to_string(),
            "--out-dir".into(),
            self.shard_dir(),
        ];
        for tree in trees {
            a.push("--easyconfigs".into());
            a.push(tree);
        }
        Ok(Cmd {
            name: "shard".into(),
            argv: self.wrapped(Role::Plan, "shard", a, true)?,
            env: BTreeMap::new(),
            note: Some(
                "restart boundaries; a successful unit whose outputs remain is skipped on --resume-result"
                    .into(),
            ),
        })
    }

    /// Module files of `line` that exist now, under the target's modules root.
    pub fn declared_outputs(&self, line: &ListLine) -> Vec<String> {
        let Some(rel) = module_rel(&line.easyconfig) else {
            return Vec::new();
        };
        let root = self.scope.modules_root();
        let base = format!("{}/all/{rel}", root.trim_end_matches('/'));
        [format!("{base}.lua"), base]
            .into_iter()
            .filter(|p| Path::new(p).is_file())
            .collect()
    }

    /// A command eb-pipeline runs for `phase` as it runs `stack shard`: with
    /// the plan role's launch, inside the module wrapper.
    pub fn plan_role_cmd(&self, phase: &str, inner: Vec<String>) -> Result<Cmd, Fail> {
        Ok(Cmd {
            name: phase.into(),
            argv: self.wrapped(Role::Plan, phase, inner, true)?,
            env: BTreeMap::new(),
            note: None,
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

    /// The `eb --fetch` command of `line`, logged as `name`, in a unit of
    /// `unit_role`; the fetch of a CPU-role unit goes through its route.
    pub fn prefetch_cmd(&self, line: &ListLine, name: &str, unit_role: Role) -> Result<Cmd, Fail> {
        let extra = self
            .site()
            .sources_on()
            .map(|s| s.extra_args.clone())
            .unwrap_or_default();
        let args = line.args.iter().filter(|x| !x.starts_with("--rebuild"));
        let mut options = vec!["--fetch".into()];
        if extra.iter().any(|x| x == "--disable-robot") {
            options.extend(without_robot(args));
        } else {
            options.push("--robot".into());
            options.extend(args.cloned());
        }
        options.extend(extra);
        let mut a = vec!["eb".to_string()];
        push_eb_line(&mut a, &line.easyconfig, options);
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

    fn source_easyconfigs(&self) -> Vec<String> {
        let sources = self.site().sources_on().expect("sources active");
        if !sources.easyconfigs.is_empty() {
            sources.easyconfigs.clone()
        } else {
            self.site()
                .shards
                .as_ref()
                .map(|s| s.easyconfigs.clone())
                .unwrap_or_default()
        }
    }

    /// The required eb-stack planner calls for an offline bundle.
    pub fn offline_planner_cmds(&self, lines: &[ListLine]) -> Result<Vec<Cmd>, Fail> {
        let Some(bundle) = &self.offline_bundle else {
            return Ok(Vec::new());
        };
        let planner = bundle.join("planner");
        let easystack = planner.join("easystack.yml");
        let trees = self.source_easyconfigs();
        let mut commands = Vec::new();
        if !lines.is_empty() {
            let mut order = vec![
                Tool::EbStack.bin(self.site()),
                "stack".into(),
                "order".into(),
            ];
            for tree in &trees {
                order.extend(["--easyconfigs".into(), tree.clone()]);
            }
            let roots: std::collections::BTreeSet<String> = lines
                .iter()
                .map(|line| {
                    let (name, version) = crate::util::package_of(&line.easyconfig);
                    version.map_or_else(|| name.clone(), |version| format!("{name}=={version}"))
                })
                .collect();
            for root in roots {
                order.extend(["--root".into(), root]);
            }
            order.extend([
                "--out".into(),
                planner.join("build-order.txt").display().to_string(),
                "--easystack-out".into(),
                easystack.display().to_string(),
            ]);
            commands.push(self.plan_role_cmd("offline-order", order)?);

            let mut files = vec![
                Tool::EbStack.bin(self.site()),
                "stack".into(),
                "files".into(),
                "--easystack".into(),
                easystack.display().to_string(),
            ];
            for tree in &trees {
                files.extend(["--easyconfigs".into(), tree.clone()]);
            }
            for path in self.scope.source_paths() {
                files.extend(["--sourcepath".into(), path]);
            }
            commands.push(self.plan_role_cmd("offline-files", files)?);
        }

        let sources = self.site().sources_on().expect("sources active");
        for (i, foreign) in sources.foreign.iter().enumerate() {
            let inspect = vec![
                Tool::EbStack.bin(self.site()),
                "package".into(),
                "inspect".into(),
                "--source".into(),
                foreign.source.clone(),
                "--format".into(),
                foreign.format.clone(),
                "--toolchain".into(),
                foreign.toolchain.clone(),
                "--out-dir".into(),
                planner
                    .join(format!("foreign-{:02}", i + 1))
                    .display()
                    .to_string(),
            ];
            commands.push(self.plan_role_cmd("offline-package-inspect", inspect)?);
        }
        Ok(commands)
    }

    /// The `stack admit` command of `unit`, with its inode `footprint`, for a
    /// site with `[admission]` active.
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

    /// `<state root>/.lock`, the lock a staged line waits
    /// `rebuild.lock_timeout` for. Without `rebuild.state_root` the state
    /// root is eb-staged-rebuild's default: `env_root`, the
    /// EB_STAGED_REBUILD_STATE_ROOT it sees, else `.eb-staged-rebuild` next to
    /// the software tree.
    pub fn state_root_lock(&self, env_root: Option<&str>) -> PathBuf {
        let root = self
            .scope
            .state_root()
            .or_else(|| env_root.filter(|r| !r.is_empty()).map(str::to_string))
            .map(PathBuf::from)
            .unwrap_or_else(|| {
                let software = PathBuf::from(self.scope.software_root());
                let parent = software.parent().unwrap_or(Path::new("/"));
                parent.join(".eb-staged-rebuild")
            });
        root.join(".lock")
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

    /// `<log_dir>/<stem>.stdout.log`, the output of the line's build.
    pub fn log_stdout(&self, stem: &str) -> String {
        format!("{}/{}.stdout.log", self.scope.log_dir(), stem)
    }

    /// `<log_dir>/easybuild-logs/<stem>`, the line's `--tmp-logdir` with
    /// `logs.keep_failed`.
    pub fn tmp_logdir(&self, stem: &str) -> String {
        format!("{}/easybuild-logs/{}", self.scope.log_dir(), stem)
    }

    /// The command that builds `line`, logged as `stem`, with the launch
    /// arguments of `role`: `eb`, or `eb-staged-rebuild run` for a staged
    /// rebuild.
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
            inner.push("--lock-timeout".into());
            inner.push(r.lock_timeout.to_string());
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
        let mut options = line.args.clone();
        if self.site().logs_on().is_some_and(|l| l.keep_failed) {
            options.push(format!("--tmp-logdir={}", self.tmp_logdir(stem)));
        }
        push_eb_line(&mut inner, &line.easyconfig, options);
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

    /// The `eb-keep-logs failed` command for the line logged as `stem`, which
    /// failed with exit `status`.
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

    /// `eb-keep-logs rotate` of the log root; with `lock_timeout`, it gives
    /// up at once on the lock another rotate holds. `run` leaves the option
    /// out for an eb-keep-logs whose `--help` does not list it.
    pub fn rotate_cmd(&self, lock_timeout: bool) -> Result<Option<Cmd>, Fail> {
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
        if lock_timeout {
            // The run holds .eb-pipeline-rotate.lock alone while rotate runs,
            // and every run under the log root that starts meanwhile waits
            // for it; the rotate that holds eb-keep-logs' lock does this
            // rotation's work.
            a.push("--lock-timeout".into());
            a.push("0".into());
        }
        Ok(Some(Cmd {
            name: "rotate".into(),
            argv: self.wrapped(Role::Short, "logs", a, false)?,
            // eb-keep-logs counts a run's age from the local midnight of the
            // date in its name, and run directories are named in UTC.
            env: BTreeMap::from([("TZ".to_string(), "UTC".to_string())]),
            note: lock_timeout.then(|| {
                "without --lock-timeout 0 when `eb-keep-logs --help` does not list it, as in eb-keep-logs 0.2.1".into()
            }),
        }))
    }

    /// `<log_root>/.eb-pipeline-rotate.lock`, with `[logs.rotate]` set. A
    /// run holds it shared while it writes logs and rotates only when it can
    /// take it alone. eb-keep-logs takes a lock of its own, `keep_logs_lock`,
    /// which keeps two rotates apart but not a rotate and a run that is
    /// still writing, and never takes this one: whatever takes both takes
    /// this one first. eb-keep-logs 0.2.1 takes no lock.
    pub fn rotate_lock(&self) -> Option<PathBuf> {
        self.site().logs_on()?.rotate.as_ref()?;
        Some(PathBuf::from(&self.site().filesystems.log_root).join(".eb-pipeline-rotate.lock"))
    }

    /// `<log_root>/.eb-keep-logs-rotate.lock`, which `eb-keep-logs rotate`
    /// holds alone for the whole rotation where it takes a lock of its own.
    pub fn keep_logs_lock(&self) -> PathBuf {
        PathBuf::from(&self.site().filesystems.log_root).join(".eb-keep-logs-rotate.lock")
    }

    /// Whether any kept line rebuilds an installed module.
    pub fn any_staged(&self, lines: &[ListLine]) -> Result<bool, Fail> {
        let (kept, _) = self.select_lines(lines)?;
        Ok(kept.iter().any(|l| self.is_staged(l)))
    }

    /// Why the checks phase is absent, when `check_cmd` returns none.
    pub fn checks_reason(&self, any_staged: bool) -> &'static str {
        let Some(c) = self.site().checks_on() else {
            return "not configured";
        };
        let empty = c
            .command
            .as_ref()
            .map(|s| s.trim().is_empty())
            .unwrap_or(true);
        if empty {
            return "not configured";
        }
        if c.when == "staged" && !any_staged {
            return "no staged rebuild in this list";
        }
        "not configured"
    }

    /// The site test command, or none when the phase does not apply.
    pub fn check_cmd(&self, any_staged: bool) -> Result<Option<Cmd>, Fail> {
        let Some(c) = self.site().checks_on() else {
            return Ok(None);
        };
        let Some(command) = c.command.as_ref().filter(|s| !s.trim().is_empty()) else {
            return Ok(None);
        };
        if c.when == "staged" && !any_staged {
            return Ok(None);
        }
        let mut env = BTreeMap::new();
        env.insert(
            "EB_PIPELINE_CHECK_SOFTWARE".into(),
            self.scope.software_root(),
        );
        env.insert(
            "EB_PIPELINE_CHECK_MODULES".into(),
            self.scope.modules_root(),
        );
        env.insert("EB_PIPELINE_CHECK_RUN_ID".into(), self.scope.run_id.clone());
        env.insert("EB_PIPELINE_CHECK_LOG_DIR".into(), self.scope.log_dir());
        env.insert(
            "EB_PIPELINE_CHECK_STAGED".into(),
            if any_staged { "1" } else { "0" }.into(),
        );
        let mut argv = self.scope.launch_with(Role::Short, "checks", None)?;
        argv.extend(self.scope.module_wrap());
        argv.extend(self.offline_inner("checks", crate::util::split_args(command)?));
        Ok(Some(Cmd {
            name: "check".into(),
            argv,
            env,
            note: Some(format!("when={}", c.when)),
        }))
    }

    /// Ecosystem-native verification commands for an eb-stack package plan.
    pub fn verification_cmds(&self) -> Result<Vec<Cmd>, Fail> {
        let Some(input) = self
            .package_plan
            .as_ref()
            .filter(|_| self.verify_package_plan)
        else {
            return Ok(Vec::new());
        };
        input
            .verification
            .iter()
            .map(|check| {
                let module = check.module()?;
                let inner = match check {
                    Verification::Pip { .. } => vec!["pip".into(), "check".into()],
                    Verification::R { package, .. } => vec![
                        "Rscript".into(),
                        "--vanilla".into(),
                        "-e".into(),
                        R_DESCRIPTION_CHECK.into(),
                        package.clone(),
                    ],
                };
                let mut env = BTreeMap::new();
                let modules = self
                    .site()
                    .modules
                    .init
                    .iter()
                    .map(String::as_str)
                    .chain(std::iter::once(module.as_str()))
                    .collect::<Vec<_>>()
                    .join(" ");
                env.insert("EB_PIPELINE_MODULES".into(), modules);
                let mut argv = self.scope.launch_with(Role::Short, "verification", None)?;
                argv.extend([
                    self.site().modules.shell.clone(),
                    "-c".into(),
                    crate::resolve::MODULE_SCRIPT.into(),
                    "eb-pipeline".into(),
                ]);
                argv.extend(inner);
                Ok(Cmd {
                    name: match check {
                        Verification::Pip { .. } => format!("pip-check:{module}"),
                        Verification::R { .. } => format!("r-description-check:{module}"),
                    },
                    argv,
                    env,
                    note: Some(format!("loads {module}")),
                })
            })
            .collect()
    }

    /// The cache command of the target's `[[caches.targets]]` entry; `None`
    /// without an active `[caches]`, its `command` or such an entry.
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
        v.extend(self.offline_inner("caches", crate::util::split_args(command)?));
        Ok(Some(Cmd {
            name: "cache".into(),
            argv: v,
            env,
            note: None,
        }))
    }
}

/// Refuse a plan whose `[modules] init` names an EESSI generation other than
/// the one the target's watch record was built against. Building into that
/// prefix would mix the two. The installs are [`crate::watch::built_against_other`],
/// the same walk [`crate::watch::other_eessi`] uses, with no CPU subdirectory:
/// a move that keeps the generation is still watch's check. A site with no
/// `[watch]` table, no recorded install, or no `EESSI/YYYY.MM` module is left
/// as it was. The generation is [`crate::eessi::generation_name`].
pub fn refuse_mixed_generation(p: &Planner) -> Result<(), Fail> {
    if p.scope.site.watch_on().is_none() {
        return Ok(());
    }
    let Some(loaded) = crate::eessi::last_init_generation(&p.scope.site.modules.init) else {
        return Ok(());
    };
    let Some(dir) = p.scope.watch_state_dir() else {
        return Ok(());
    };
    let state =
        crate::watch::load(&dir, &p.scope.target.name).map_err(|e| Fail::new(exit::FAILED, e))?;
    if state.installs.is_empty() {
        return Ok(());
    }
    let other = crate::watch::built_against_other(&state, loaded, None);
    if other.is_empty() {
        return Ok(());
    }
    let mut seen: Vec<(&str, &str)> = Vec::new();
    for (_, install) in &other {
        if !seen
            .iter()
            .any(|(v, s)| *v == install.eessi_version && *s == install.software_subdir)
        {
            seen.push((&install.eessi_version, &install.software_subdir));
        }
    }
    let recorded = seen
        .iter()
        .map(|(version, subdir)| format!("EESSI {version} {subdir}"))
        .collect::<Vec<_>>()
        .join(", ");
    Err(Fail::new(
        exit::REFUSED,
        format!(
            "target {} loads EESSI {loaded} in [modules] init, and {} of {} recorded installs were built against {recorded}; a build into this prefix would mix the two. Give the new EESSI its own prefix. eb-stack stack gap --out-dir DIR writes roots.txt, the easyconfigs the new version lacks, and eb-pipeline plan --list DIR/roots.txt plans that list. Then record the new prefix with watch --adopt",
            p.scope.target.name,
            other.len(),
            state.installs.len(),
        ),
    ))
}

/// Build the whole plan for one target and list.
pub fn build_plan(
    p: &Planner,
    lines: &[ListLine],
    shard_units: Option<&[Unit]>,
) -> Result<Plan, Fail> {
    refuse_mixed_generation(p)?;
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
    let mut needed = crate::tools::needed(site);
    if p.offline_bundle.is_some() && !needed.contains(&Tool::EbStack) {
        needed.push(Tool::EbStack);
    }
    let probes: Vec<Cmd> = needed
        .into_iter()
        .filter_map(|t| {
            let min = t.min_version(site);
            if min.is_none() && !(p.offline_bundle.is_some() && t == Tool::EbStack) {
                return None;
            }
            Some(Cmd {
                name: format!("probe:{}", t.key()),
                argv: vec![t.bin(site), "--version".into()],
                env: BTreeMap::new(),
                note: min.map(|min| format!("min_version {min}")),
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

    // shard units come from the selected list, so only the whole list is filtered
    let (kept, deselected) = p.select_lines(lines)?;
    let boundary = site.resume_on().is_some_and(|r| r.shards >= 2) && site.shards_on().is_none();
    if site.shards_on().is_some() || boundary {
        for l in &kept {
            // stack shard splits the line on white space and does not drop
            // quotes. A word with a space or a quote is a different word to
            // it than to the build, so the plan would not be the line's plan.
            let split = l.raw.split_whitespace();
            let words =
                std::iter::once(l.easyconfig.as_str()).chain(l.args.iter().map(String::as_str));
            if !split.eq(words) {
                let ending = if site.shards_on().is_some() {
                    "or run without [shards]"
                } else {
                    "or set [resume] shards below 2"
                };
                return Err(Fail::usage(format!(
                    "build list line `{}`: stack shard reads words split on white space, and those words are not this line's arguments; write each argument without a space or a quote, {ending}",
                    l.raw
                )));
            }
        }
    }
    let mut shard_cmds = Vec::new();
    if !kept.is_empty() && site.resume_on().is_some() {
        shard_cmds.push(p.order_cmd(&kept)?);
    }
    if site.shards_on().is_some() {
        shard_cmds.push(p.shard_cmd()?);
    } else if boundary && !kept.is_empty() {
        shard_cmds.push(p.boundary_shard_cmd()?);
    }
    phases.push(if shard_units.is_some() {
        PlanPhase {
            name: "shard".into(),
            status: "reused".into(),
            reason: Some("using the supplied shard plan".into()),
            role: None,
            env: BTreeMap::new(),
            commands: vec![],
        }
    } else if shard_cmds.is_empty() {
        skip("shard", "not configured")
    } else {
        planned("shard", Some(Role::Plan), shard_cmds)
    });
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
        let offline_lines: Vec<ListLine> = units
            .iter()
            .flat_map(|unit| unit.lines.iter().cloned())
            .collect();
        let mut cmds = p.offline_planner_cmds(&offline_lines)?;
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

    let any_staged = units.iter().flat_map(|u| &u.lines).any(|l| p.is_staged(l));
    phases.push(match p.check_cmd(any_staged)? {
        Some(c) => planned("checks", Some(Role::Short), vec![c]),
        None => skip("checks", p.checks_reason(any_staged)),
    });

    if p.package_plan.is_some() {
        let commands = p.verification_cmds()?;
        phases.push(if p.verify_package_plan {
            planned("verification", Some(Role::Short), commands)
        } else {
            skip("verification", "not requested")
        });
    }

    phases.push(match p.cache_cmd()? {
        Some(c) => planned("caches", Some(Role::Cache), vec![c]),
        None => skip("caches", "not configured"),
    });

    phases.push(match p.rotate_cmd(true)? {
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
        package_plan_digest: p.package_plan.as_ref().map(|input| input.digest.clone()),
        log_dir: p.scope.log_dir(),
        lines: lines.len(),
        selected: p.scope.target.select.as_ref().map(|_| kept.len()),
        selected_list: p.selected_list_path().map(|x| x.display().to_string()),
        phases,
        deselected,
        input_attestation: None,
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
        let list = read_text_file(&path)
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
            if !s["file"].is_string() {
                return Err(Fail::new(
                    exit::FAILED,
                    format!("{}: shards[{i}].file must be a string", plan_path.display()),
                ));
            }
            let name = s["name"]
                .as_str()
                .map(str::to_string)
                .unwrap_or_else(|| format!("shard-{:02}", i + 1));
            add(name, Role::Build, s)?;
        }
    }
    if units.is_empty() {
        return Err(Fail::new(
            exit::FAILED,
            format!("{} contains no build units", plan_path.display()),
        ));
    }
    let mut names = BTreeSet::new();
    for unit in &units {
        if !names.insert(&unit.name) {
            return Err(Fail::new(
                exit::FAILED,
                format!(
                    "{} contains duplicate unit name `{}`",
                    plan_path.display(),
                    unit.name
                ),
            ));
        }
    }
    Ok(units)
}

#[cfg(test)]
mod read_tests {
    use super::{push_eb_line, read_units};

    #[test]
    fn easybuild_operand_order_changes_only_for_a_leading_dash() {
        let mut ordinary = vec!["eb".to_string()];
        push_eb_line(&mut ordinary, "Foo-1.0.eb", vec!["--rebuild".into()]);
        assert_eq!(
            ordinary,
            ["eb", "Foo-1.0.eb", "--rebuild"],
            "ordinary argv bytes and order stay unchanged"
        );

        let mut staged = vec!["eb-staged-rebuild".into(), "run".into(), "--".into()];
        push_eb_line(&mut staged, "-Foo-1.0.eb", vec!["--rebuild".into()]);
        assert_eq!(
            staged,
            [
                "eb-staged-rebuild",
                "run",
                "--",
                "--rebuild",
                "--",
                "-Foo-1.0.eb"
            ]
        );
    }

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

    #[test]
    fn a_shard_plan_cannot_silently_drop_a_shard_without_a_file() {
        let root = tempfile::tempdir().unwrap();
        std::fs::write(root.path().join("one.list"), "Foo-1.0.eb\n").unwrap();
        std::fs::write(
            root.path().join("plan.json"),
            r#"{"shards":[{"name":"one","file":"one.list"},{"name":"two"}]}"#,
        )
        .unwrap();

        let error = read_units(root.path()).unwrap_err();
        assert!(error.msg.contains("shards[1].file"), "{}", error.msg);
    }

    #[test]
    fn a_shard_plan_cannot_reuse_a_unit_name() {
        let root = tempfile::tempdir().unwrap();
        std::fs::write(root.path().join("one.list"), "Foo-1.0.eb\n").unwrap();
        std::fs::write(root.path().join("two.list"), "Bar-2.0.eb\n").unwrap();
        std::fs::write(
            root.path().join("plan.json"),
            r#"{"shards":[{"name":"same","file":"one.list"},{"name":"same","file":"two.list"}]}"#,
        )
        .unwrap();

        let error = read_units(root.path()).unwrap_err();
        assert!(
            error.msg.contains("duplicate unit name `same`"),
            "{}",
            error.msg
        );
    }
}

#[cfg(test)]
mod generation_tests {
    use super::{build_plan, Planner};
    use crate::resolve::Scope;
    use crate::site;
    use crate::util;

    fn site_text(prefix: &str, init: &str, watch: bool) -> String {
        let watch_table = if watch { "[watch]\n" } else { "" };
        format!(
            r#"
schema_version = 1
kind = "site"
[site]
name = "example"
[scheduler]
kind = "local"
[[targets]]
name = "zen4"
site_prefix = "{prefix}"
default = true
[filesystems]
scratch = "/tmp/scratch"
tmp = "/tmp/scratch/tmp"
build_path = "/tmp/scratch/build"
install_root = "/tmp/site"
log_root = "/tmp/logs"
[modules]
init = ["{init}"]
{watch_table}"#
        )
    }

    fn record(prefix: &str, version: &str) {
        let dir = format!("{prefix}/.eb-pipeline-watch");
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(
            format!("{dir}/zen4.json"),
            format!(
                r#"{{
  "schema_version": 1,
  "kind": "watch-state",
  "target": "zen4",
  "installs": {{
    "hiredis/1.2.0-GCCcore-13.2.0": {{
      "easyconfig": "hiredis-1.2.0-GCCcore-13.2.0.eb",
      "how": "built",
      "run": "20261117_090000",
      "recorded_at": "2026-11-17T09:00:00Z",
      "eessi_version": "{version}",
      "software_subdir": "x86_64/amd/zen4",
      "modules_dirs": ["/cvmfs/modules"],
      "deps": {{}}
    }}
  }}
}}"#
            ),
        )
        .unwrap();
    }

    fn plan(prefix: &str, init: &str, watch: bool) -> Result<super::Plan, crate::exit::Fail> {
        let text = site_text(prefix, init, watch);
        let site = site::parse(&text).unwrap();
        let target = site.target("zen4").unwrap();
        let scope = Scope {
            site: &site,
            site_file: None,
            target,
            build_type: Some("production"),
            run_id: "r".into(),
            log_stamp: "20261117_140000".into(),
        };
        let planner = Planner::new(scope, "zen4.list".into());
        let lines = util::parse_list("hiredis-1.2.0-GCCcore-13.2.0.eb\n").unwrap();
        build_plan(&planner, &lines, None)
    }

    #[test]
    fn a_plan_refuses_to_mix_two_eessi_generations() {
        let root = tempfile::tempdir().unwrap();
        let prefix = root.path().to_str().unwrap();
        record(prefix, "2023.06");
        let err = plan(prefix, "EESSI/2025.06", true).unwrap_err();
        assert_eq!(err.code, 78, "{}", err.msg);
        assert!(
            err.msg.contains("loads EESSI 2025.06 in [modules] init"),
            "{}",
            err.msg
        );
        assert!(
            err.msg.contains(
                "1 of 1 recorded installs were built against EESSI 2023.06 x86_64/amd/zen4"
            ),
            "{}",
            err.msg
        );
        assert!(err.msg.contains("roots.txt"), "{}", err.msg);
        assert!(err.msg.contains("watch --adopt"), "{}", err.msg);
    }

    #[test]
    fn a_plan_of_the_recorded_generation_goes_ahead() {
        let root = tempfile::tempdir().unwrap();
        let prefix = root.path().to_str().unwrap();
        record(prefix, "2023.06");
        assert!(plan(prefix, "EESSI/2023.06", true).is_ok());
        // The last EESSI/YYYY.MM in init is the generation, and a month
        // generation_name rejects is not one, so neither refuses.
        assert!(plan(prefix, "EESSI/2025.06\", \"EESSI/2023.06", true).is_ok());
        assert!(plan(prefix, "EESSI/2025.13", true).is_ok());
    }

    #[test]
    fn a_plan_with_no_watch_record_goes_ahead() {
        let root = tempfile::tempdir().unwrap();
        let prefix = root.path().to_str().unwrap();
        assert!(plan(prefix, "EESSI/2025.06", true).is_ok());
        record(prefix, "2023.06");
        assert!(plan(prefix, "EESSI/2025.06", false).is_ok());
    }

    #[test]
    fn the_how_tos_quote_the_refusal() {
        let root = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("docs/orgmode/howto");
        for name in ["from-eb-stack.org", "staged-rebuilds.org"] {
            let page = std::fs::read_to_string(root.join(name)).unwrap();
            assert!(
                page.contains("loads EESSI VERSION in [modules] init"),
                "{name}"
            );
            assert!(page.contains("watch --adopt"), "{name}");
        }
    }
}
