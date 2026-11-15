//! Site file schema.
//!
//! Every struct is declared through `doc_struct!`, which emits the serde
//! struct and its field documentation from one declaration. The parser, the
//! `schema` reference and the documentation test all read the same table.

use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

/// One documented key of the site file.
#[derive(Debug, Clone, Copy)]
pub struct FieldDoc {
    pub key: &'static str,
    pub ty: &'static str,
    pub default: &'static str,
    pub required: bool,
    pub doc: &'static str,
}

macro_rules! doc_struct {
    (
        $(#[$m:meta])*
        $name:ident {
            $( $(#[$fm:meta])* $f:ident : $t:ty => ($ty:expr, $def:expr, $req:expr, $doc:expr) ),* $(,)?
        }
    ) => {
        $(#[$m])*
        #[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
        #[serde(deny_unknown_fields)]
        pub struct $name {
            $( $(#[$fm])* pub $f: $t, )*
        }
        impl $name {
            pub const FIELDS: &'static [FieldDoc] = &[
                $( FieldDoc { key: stringify!($f), ty: $ty, default: $def, required: $req, doc: $doc } ),*
            ];
        }
    };
}

fn yes() -> bool {
    true
}
fn d_launch() -> String {
    "srun".into()
}
fn d_template() -> String {
    "{project}-{build}-{phase}".into()
}
fn d_lock_kind() -> String {
    "flock".into()
}
fn d_attempts() -> u32 {
    5
}
fn d_wait() -> u64 {
    60
}
fn d_max_wait() -> u64 {
    600
}
fn d_one() -> u32 {
    1
}
fn d_unsharded() -> i32 {
    3
}
fn d_shell() -> String {
    "bash".into()
}
fn d_backend() -> String {
    "auto".into()
}
fn d_route_kind() -> String {
    "none".into()
}
fn d_sep() -> String {
    ",".into()
}
fn d_all() -> String {
    "all".into()
}
fn d_route_phases() -> Vec<String> {
    vec!["build".into()]
}
fn d_single() -> String {
    "single".into()
}
fn d_ro_value() -> String {
    "1".into()
}
fn d_job_env() -> String {
    "SLURM_JOB_ID".into()
}
fn d_cpu_var() -> String {
    "EESSI_SOFTWARE_SUBDIR_OVERRIDE".into()
}
fn d_accel_var() -> String {
    "EESSI_ACCELERATOR_TARGET_OVERRIDE".into()
}

doc_struct! {
    /// A sibling tool: binary and minimum version.
    ToolSpec {
        bin: Option<String> => ("string", "the tool's own name, looked up on PATH", false,
            "Executable path or name."),
        min_version: Option<String> => ("string", "none", false,
            "Minimum version as dotted numbers. The tool's `--version` output is probed and a lower version stops the run with exit 2 before any work."),
    }
}

doc_struct! {
    /// Sibling tool table.
    Tools {
        eb_stack: Option<ToolSpec> => ("table", "bin eb-stack", false,
            "eb-stack, called for `stack shard` and `stack admit`."),
        eb_keep_logs: Option<ToolSpec> => ("table", "bin eb-keep-logs", false,
            "eb-keep-logs, called for `failed` and `rotate`."),
        eb_staged_rebuild: Option<ToolSpec> => ("table", "bin eb-staged-rebuild", false,
            "eb-staged-rebuild, called for lines that rebuild a live install."),
    }
}

doc_struct! {
    /// Site identity.
    SiteInfo {
        name: String => ("string", "", true, "Site label used in receipts and job names."),
        project: Option<String> => ("string", "site.name", false,
            "Value of the `{project}` placeholder in `scheduler.job_name_template`."),
        build_account: Option<String> => ("string", "none", false,
            "Account that owns installs; informational, exported as EB_PIPELINE_BUILD_ACCOUNT."),
        default_group: Option<String> => ("string", "none", false,
            "Default unix group of installs; exported as EB_PIPELINE_DEFAULT_GROUP."),
        stack_generation: Option<String> => ("string", "none", false,
            "Label of the software generation; not interpreted."),
        tools: Option<Tools> => ("table", "all tools looked up on PATH, no minimum version", false,
            "Tool binaries and minimum versions, `[site.tools.<tool>]`."),
    }
}

doc_struct! {
    /// How a phase becomes a job.
    Scheduler {
        kind: String => ("string", "", true,
            "`slurm` (phases launch through `launch`) or `local` (phases run directly, phase arguments ignored)."),
        #[serde(default = "d_launch")]
        launch: String => ("string", "srun", false, "Launch command for `slurm`."),
        #[serde(default)]
        common_args: Vec<String> => ("array of string", "[]", false,
            "Arguments added to every launch line, before the per-phase arguments."),
        #[serde(default)]
        env: BTreeMap<String, String> => ("table of string", "{}", false,
            "Environment exported to every phase."),
        #[serde(default = "d_template")]
        job_name_template: String => ("string", "{project}-{build}-{phase}", false,
            "Job name. Placeholders: {project}, {build}, {phase}, {target}."),
    }
}

doc_struct! {
    /// Launch arguments per phase role.
    PhaseArgs {
        plan: Option<String> => ("string", "none", false,
            "Launch arguments of the planning phase (`stack shard`), split on whitespace with quotes honoured."),
        build: Option<String> => ("string", "none", false,
            "Launch arguments of build phases."),
        short: Option<String> => ("string", "none", false,
            "Launch arguments of short phases: admission, source prefetch, log handling."),
        cpu: Option<String> => ("string", "none", false,
            "Launch arguments of CPU-only prerequisite builds; the target's `route` wraps these."),
        cache: Option<String> => ("string", "none", false,
            "Launch arguments of the module cache phase when a cache entry sets none."),
    }
}

doc_struct! {
    /// Wrapper for CPU phases on a host that is not native to the target.
    Route {
        #[serde(default = "d_route_kind")]
        kind: String => ("string", "none", false, "`none` or `command`."),
        command: Option<String> => ("string", "none", false,
            "Wrapper command placed before the launch line of CPU-role phases; required for kind `command`."),
        #[serde(default)]
        args: Vec<String> => ("array of string", "[]", false,
            "Argument templates placed after `command`. When present, a `--` follows them and the phase command comes after it. Placeholders: {phase_dir}, {log_dir}, {work_dir}, {target}, {phase}, {runtime_root}, {shares_ro}, {shares_rw}. {phase_dir} is `<log_dir>/<phase>` and is created before the command starts; {work_dir} is `filesystems.build_path`; the share placeholders join the rendered lists with `:`."),
        #[serde(default)]
        env: BTreeMap<String, String> => ("table of string", "{}", false,
            "Environment exported to the routed command; values accept the same placeholders as `args`."),
        runtime_root: Option<String> => ("string", "none", false,
            "Root of the route's runtime; exported as EB_PIPELINE_ROUTE_RUNTIME_ROOT and available as {runtime_root}."),
        #[serde(default)]
        shares_ro: Vec<String> => ("array of string", "[]", false,
            "Paths shared read-only into the route; placeholders {phase_dir}, {log_dir}, {work_dir} allowed. Exported joined with `:` as EB_PIPELINE_ROUTE_SHARES_RO."),
        #[serde(default)]
        shares_rw: Vec<String> => ("array of string", "[]", false,
            "Paths shared read-write into the route; same placeholders as `shares_ro`. Exported as EB_PIPELINE_ROUTE_SHARES_RW."),
        receipt_digest_env: Option<String> => ("string", "none", false,
            "Name of an environment variable that holds a receipt digest. `run` refuses with exit 78 when it is unset or empty and passes the value to the routed command as EB_PIPELINE_ROUTE_RECEIPT_DIGEST; `plan` shows `${NAME}`."),
        #[serde(default = "d_route_phases")]
        phases: Vec<String> => ("array of string", "[\"build\"]", false,
            "Phases whose CPU-role commands go through the route: `build`, `sources`."),
    }
}

doc_struct! {
    /// Per-target line selection by a build list option.
    Select {
        option: String => ("string", "", true,
            "Long option name that carries the selection on a build list line, written `--name=a,b` or `--name a,b`."),
        value: String => ("string", "", true, "This target's token in the option's value list."),
        #[serde(default = "d_sep")]
        separator: String => ("string", ",", false, "Separator between tokens in the option's value."),
        #[serde(default = "d_all")]
        when_absent: String => ("string", "all", false,
            "`all` plans a line that lacks the option for this target; `none` skips it."),
        #[serde(default)]
        strip: bool => ("boolean", "false", false,
            "Remove the option from the line before `eb` is called."),
    }
}

doc_struct! {
    /// Hardware identity of a target.
    Hardware {
        vendor_id: Option<String> => ("string", "none", false, "CPU vendor id."),
        cpu_family: Option<u32> => ("integer", "none", false, "CPU family."),
        model: Option<u32> => ("integer", "none", false, "CPU model."),
    }
}

doc_struct! {
    /// Per build type override of a target.
    BuildTypeOverride {
        eb_optarch: Option<String> => ("string", "target value", false,
            "EASYBUILD_OPTARCH for this build type."),
        max_parallel: Option<u32> => ("integer", "target value", false,
            "EASYBUILD_MAX_PARALLEL for this build type."),
    }
}

doc_struct! {
    /// One node type.
    Target {
        name: String => ("string", "", true, "Target name; unique across the file."),
        #[serde(default)]
        build_lists: Vec<String> => ("array of string", "[]", false,
            "Names of build list files that belong to this target; checked against `--lists-dir`."),
        eb_optarch: Option<String> => ("string", "none", false, "EASYBUILD_OPTARCH."),
        max_parallel: Option<u32> => ("integer", "none", false, "EASYBUILD_MAX_PARALLEL."),
        cuda_compute_capabilities: Option<String> => ("string", "none", false,
            "EASYBUILD_CUDA_COMPUTE_CAPABILITIES."),
        cpu_target: Option<String> => ("string", "none", false,
            "CPU software subdirectory, exported through `modules.eessi.cpu_override_var`."),
        accel_target: Option<String> => ("string", "none", false,
            "Accelerator subdirectory, exported through `modules.eessi.accel_override_var`; empty means none."),
        site_prefix: Option<String> => ("string", "none", false,
            "Install prefix of the target; EASYBUILD_INSTALLPATH and EESSI_SITE_SOFTWARE_PREFIX. Falls back to `filesystems.install_root`."),
        software_root: Option<String> => ("string", "<install prefix>/software", false,
            "Live software root of the target; passed to eb-staged-rebuild `--software`."),
        modules_root: Option<String> => ("string", "<install prefix>/modules", false,
            "Live modules root of the target; passed to eb-staged-rebuild `--modules` and searched for installed modules at `<modules_root>/all/<Name>/<version>[.lua]`."),
        staged: Option<bool> => ("boolean", "rebuild.staged", false,
            "Overrides `rebuild.staged` for this target."),
        #[serde(default)]
        default: bool => ("boolean", "false", false, "Marks the default target of the site; informational."),
        phases: Option<PhaseArgs> => ("table", "phase_defaults", false,
            "Overrides of `[phase_defaults]` for this target."),
        route: Option<Route> => ("table", "none", false, "CPU phase wrapper."),
        select: Option<Select> => ("table", "none", false,
            "Per-line target selection; without it every line is planned for every target."),
        hardware: Option<Hardware> => ("table", "none", false, "Hardware identity."),
        cpu_hardware: Option<Hardware> => ("table", "none", false,
            "Hardware identity of CPU-role phases. When set, every command exports its class as EB_PIPELINE_HW_VENDOR_ID, EB_PIPELINE_HW_CPU_FAMILY and EB_PIPELINE_HW_MODEL: `cpu_hardware` for CPU-role commands, `hardware` for the rest."),
        fetch_profile: Option<toml::Table> => ("table", "none", false,
            "Free-form table passed to source prefetch commands as EB_PIPELINE_FETCH_PROFILE, serialised as JSON with sorted keys."),
        #[serde(default)]
        package_env: BTreeMap<String, BTreeMap<String, String>> => ("table of tables", "{}", false,
            "Environment per package name for this target, `[targets.package_env.<Name>]`; per key it wins over `modules.package_env`."),
        #[serde(default)]
        build_types: BTreeMap<String, BuildTypeOverride> => ("table of tables", "{}", false,
            "Per build type overrides, `[targets.build_types.<name>]`."),
    }
}

doc_struct! {
    /// Paths.
    Filesystems {
        scratch: String => ("string", "", true, "Scratch root."),
        tmp: String => ("string", "", true, "EASYBUILD_TMPDIR."),
        build_path: String => ("string", "", true, "EASYBUILD_BUILDPATH."),
        #[serde(default)]
        source_paths: Vec<String> => ("array of string", "[]", false,
            "EASYBUILD_SOURCEPATH entries, joined with `:`."),
        install_root: String => ("string", "", true, "EASYBUILD_INSTALLPATH unless a target sets `site_prefix`."),
        #[serde(default = "d_single")]
        export_install_vars: String => ("string", "single", false,
            "`single` exports EASYBUILD_INSTALLPATH; `split` exports EASYBUILD_INSTALLPATH_SOFTWARE and EASYBUILD_INSTALLPATH_MODULES from the target's software and modules roots instead."),
        log_root: String => ("string", "", true,
            "Root of per-run log directories, `<log_root>/<run-id>/`."),
        shard_dir: Option<String> => ("string", "<scratch>/shards", false,
            "Output directory of `stack shard`; one subdirectory per run id."),
        #[serde(default)]
        read_only_installdir: bool => ("boolean", "false", false, "EASYBUILD_READ_ONLY_INSTALLDIR."),
        #[serde(default = "d_ro_value")]
        read_only_installdir_value: String => ("string", "1", false,
            "Value exported as EASYBUILD_READ_ONLY_INSTALLDIR when `read_only_installdir` is set."),
        configs_repo: Option<String> => ("string", "none", false, "EASYBUILD_CONFIGS_REPOPATH."),
    }
}

doc_struct! {
    /// Inode admission before a build unit starts.
    Admission {
        #[serde(default = "yes")]
        enabled: bool => ("boolean", "true", false, "Set false to skip the phase."),
        #[serde(default)]
        required: bool => ("boolean", "false", false,
            "When true, a missing `quota_cmd` or `reservations` refuses the run with exit 78. When false, the phase is skipped as not configured."),
        quota_cmd: Option<String> => ("string", "none", false,
            "Shell command printing the quota JSON, passed to `stack admit --quota-cmd`."),
        live_jobs_cmd: Option<String> => ("string", "none", false,
            "Shell command printing live job ids, passed to `stack admit --live-jobs-cmd`."),
        lock: Option<String> => ("string", "none", false, "Lock file shared by all admission controllers."),
        #[serde(default = "d_lock_kind")]
        lock_kind: String => ("string", "flock", false, "`flock` or `posix`; must match the other lock holders."),
        reservations: Option<String> => ("string", "none", false, "Directory of reservations."),
        margin: Option<u64> => ("integer", "tool default", false, "Inodes kept free."),
        unknown_reserve: Option<u64> => ("integer", "0", false,
            "Inodes counted per live job that has no reservation record (`stack admit --unknown-reserve`)."),
        default_footprint: Option<u64> => ("integer", "none", false,
            "Inode footprint of a unit when the shard plan gives none. Required when admission is enabled; `check` exits 2 without it."),
        #[serde(default = "d_job_env")]
        job_id_env: String => ("string", "SLURM_JOB_ID", false,
            "Environment variable that holds the scheduler job id at run time. `run` refuses with exit 78 before any build when it is unset or not a positive integer without a leading zero; `plan` shows `${NAME}`."),
        quota_total: Option<u64> => ("integer", "none", false,
            "Planning cap, passed to `stack shard --inode-quota`."),
        footprints: Option<String> => ("string", "none", false,
            "footprints.json for `stack shard --inode-footprints`."),
        line_overhead: Option<u64> => ("integer", "tool default", false,
            "Inodes per build list line beyond its build tree."),
        #[serde(default = "d_attempts")]
        attempts: u32 => ("integer", "5", false, "Admission attempts before the run is refused."),
        #[serde(default = "d_wait")]
        wait_seconds: u64 => ("integer", "60", false,
            "Seconds before the second attempt; doubles on each further attempt."),
        #[serde(default = "d_max_wait")]
        max_wait_seconds: u64 => ("integer", "600", false, "Upper bound of the wait between attempts."),
    }
}

doc_struct! {
    /// Shard planning.
    Shards {
        #[serde(default = "yes")]
        enabled: bool => ("boolean", "true", false, "Set false to skip the phase."),
        #[serde(default = "d_one")]
        count: u32 => ("integer", "1", false,
            "Most shards to plan. A count below 2 skips the phase and builds the list as one unit."),
        #[serde(default = "d_unsharded")]
        planner_exit_unsharded: i32 => ("integer", "3", false,
            "Exit code of `stack shard` that means the plan was built unsharded; the run continues unsharded."),
        #[serde(default)]
        lift_cpu_prerequisites: bool => ("boolean", "false", false,
            "Passes `--lift-cpu-prerequisites` to `stack shard`."),
        #[serde(default)]
        easyconfigs: Vec<String> => ("array of string", "[]", false,
            "Robot path trees for `stack shard --easyconfigs`; at least one is required when the phase is active."),
        weights: Option<String> => ("string", "none", false, "Build time table for `stack shard --weights`."),
        #[serde(default)]
        installed_modules_dirs: Vec<String> => ("array of string", "[]", false,
            "Module trees counted as installed, `--installed-modules-dir`."),
        #[serde(default)]
        installed_modules_dirs_from_env: Vec<String> => ("array of string", "[]", false,
            "Environment variables whose colon-separated values are appended to `installed_modules_dirs` at run time; `run` refuses with exit 78 when one is unset, an empty value adds nothing, and `plan` shows `${NAME}`."),
    }
}

doc_struct! {
    /// Source prefetch.
    Sources {
        #[serde(default = "yes")]
        enabled: bool => ("boolean", "true", false, "Set false to skip the phase."),
        #[serde(default = "yes")]
        prefetch: bool => ("boolean", "true", false,
            "Run `eb --fetch --robot` for every list line before building."),
        #[serde(default)]
        extra_args: Vec<String> => ("array of string", "[]", false, "Extra arguments for the prefetch command."),
    }
}

doc_struct! {
    /// EESSI layer.
    Eessi {
        #[serde(default = "yes")]
        enabled: bool => ("boolean", "true", false, "Set false to ignore the table."),
        #[serde(default)]
        site_install: bool => ("boolean", "false", false, "Exports EESSI_SITE_INSTALL=1."),
        bootstrap_modulepath: Option<String> => ("string", "none", false,
            "Exported as EESSI_SITE_BOOTSTRAP_MODULEPATH."),
        #[serde(default = "d_cpu_var")]
        cpu_override_var: String => ("string", "EESSI_SOFTWARE_SUBDIR_OVERRIDE", false,
            "Variable that carries the target's `cpu_target`."),
        #[serde(default = "d_accel_var")]
        accel_override_var: String => ("string", "EESSI_ACCELERATOR_TARGET_OVERRIDE", false,
            "Variable that carries the target's `accel_target`."),
    }
}

doc_struct! {
    /// Environment modules.
    Modules {
        init: Vec<String> => ("array of string", "", true,
            "Modules loaded, in order, after `module purge` in every EasyBuild phase. May be empty."),
        #[serde(default)]
        unuse_path: bool => ("boolean", "false", false, "Clears MODULEPATH before loading."),
        #[serde(default = "d_shell")]
        shell: String => ("string", "bash", false, "Shell that runs the module wrapper."),
        shell_init: Option<String> => ("string", "none", false,
            "File sourced before `module` is called, for shells that do not define it."),
        eessi: Option<Eessi> => ("table", "none", false, "EESSI site layer."),
        #[serde(default)]
        package_env: BTreeMap<String, BTreeMap<String, String>> => ("table of tables", "{}", false,
            "Environment per package name, `[modules.package_env.<Name>]`, exported for lines of that package."),
    }
}

doc_struct! {
    /// One module cache entry.
    CacheTarget {
        target: String => ("string", "", true, "Name of a `[[targets]]` entry."),
        config: String => ("string", "", true, "Cache configuration file; EB_PIPELINE_CACHE_CONFIG."),
        alloc: Option<String> => ("string", "phase_defaults.cache", false, "Launch arguments of the cache phase."),
        init_module: Option<String> => ("string", "none", false, "EB_PIPELINE_CACHE_INIT_MODULE."),
    }
}

doc_struct! {
    /// Module caches, one finaliser per target.
    Caches {
        #[serde(default = "yes")]
        enabled: bool => ("boolean", "true", false, "Set false to skip the phase."),
        command: Option<String> => ("string", "none", false,
            "Cache writer, run once per run for the selected target with EB_PIPELINE_CACHE_* in the environment. Without it the phase is not configured."),
        bindings: Option<String> => ("string", "none", false, "EB_PIPELINE_CACHE_BINDINGS."),
        init_module: Option<String> => ("string", "none", false,
            "Default EB_PIPELINE_CACHE_INIT_MODULE for entries that set none."),
        targets: Vec<CacheTarget> => ("array of table", "[]", false, "`[[caches.targets]]` entries."),
    }
}

doc_struct! {
    /// Rotation limits.
    Rotate {
        compress_after_days: Option<u32> => ("integer", "none", false, "`eb-keep-logs rotate --compress-after`."),
        delete_after_days: Option<u32> => ("integer", "none", false, "`eb-keep-logs rotate --delete-after`."),
    }
}

doc_struct! {
    /// Log handling.
    Logs {
        #[serde(default = "yes")]
        enabled: bool => ("boolean", "true", false, "Set false to skip the phase."),
        #[serde(default = "yes")]
        keep_failed: bool => ("boolean", "true", false,
            "Run `eb-keep-logs failed` after a failed line."),
        rotate: Option<Rotate> => ("table", "none", false,
            "Run `eb-keep-logs rotate <log_root>` at the end of the run."),
    }
}

doc_struct! {
    /// Staged rebuild.
    Rebuild {
        #[serde(default = "yes")]
        enabled: bool => ("boolean", "true", false, "Set false to skip the phase."),
        #[serde(default = "yes")]
        staged: bool => ("boolean", "true", false,
            "Route a line through `eb-staged-rebuild run` when it carries `--rebuild`, `-f` or `--force` as a whole option and its module is already installed. A target can override this with `staged`."),
        state_root: Option<String> => ("string", "tool default", false, "`--state-root`."),
        #[serde(default = "d_backend")]
        backend: String => ("string", "auto", false, "`auto`, `bwrap`, `userns` or `installpath`."),
        grace_hours: Option<u32> => ("integer", "tool default", false, "`--grace-hours`."),
        prepare: Option<String> => ("string", "none", false, "Executable passed as `--prepare`."),
        ledger_config: Option<String> => ("string", "none", false, "JSON file passed as `--ledger-config`."),
    }
}

doc_struct! {
    /// Required licensed software.
    LicenceRequired {
        #[serde(default)]
        software: Vec<String> => ("array of string", "[]", false,
            "Package names that must each have a group, a threshold or an exemption."),
    }
}

doc_struct! {
    /// Licence enforcement mechanism; the data stays in the site file.
    Licences {
        #[serde(default = "yes")]
        enabled: bool => ("boolean", "true", false, "Set false to skip the gate."),
        #[serde(default = "yes")]
        enforce: bool => ("boolean", "true", false,
            "Run the gate: every required name needs a group, a threshold or an exemption, and an empty configuration is refused."),
        #[serde(default)]
        groups: BTreeMap<String, String> => ("table of string", "{}", false,
            "Package name to unix group, exported as EB_PIPELINE_LICENCE_GROUP for lines of that package."),
        #[serde(default)]
        below_version: BTreeMap<String, String> => ("table of string", "{}", false,
            "Package name to version; the group applies below that version only."),
        required: Option<LicenceRequired> => ("table", "none", false,
            "Independent second list of licensed packages."),
        #[serde(default)]
        exempt: Vec<String> => ("array of string", "[]", false,
            "Required names that deliberately have no group."),
        #[serde(default)]
        env: BTreeMap<String, BTreeMap<String, String>> => ("table of tables", "{}", false,
            "Environment per package name, `[licences.env.<Name>]`."),
    }
}

doc_struct! {
    /// A site file.
    Site {
        schema_version: u32 => ("integer", "", true, "Schema version; 1."),
        kind: String => ("string", "", true, "File kind; `site`."),
        site: SiteInfo => ("table", "", true, "Site identity and tools."),
        scheduler: Scheduler => ("table", "", true, "Scheduler launch settings."),
        phase_defaults: Option<PhaseArgs> => ("table", "none", false, "Launch arguments per phase role."),
        targets: Vec<Target> => ("array of table", "", true, "Node types; at least one."),
        filesystems: Filesystems => ("table", "", true, "Paths."),
        admission: Option<Admission> => ("table", "none", false, "Inode admission."),
        shards: Option<Shards> => ("table", "none", false, "Shard planning."),
        sources: Option<Sources> => ("table", "none", false, "Source prefetch."),
        modules: Modules => ("table", "", true, "Environment modules."),
        caches: Option<Caches> => ("table", "none", false, "Module caches."),
        logs: Option<Logs> => ("table", "none", false, "Log handling."),
        rebuild: Option<Rebuild> => ("table", "none", false, "Staged rebuild."),
        licences: Option<Licences> => ("table", "none", false, "Licence enforcement."),
    }
}

/// Every documented table, by its path in the file.
pub const SECTIONS: &[(&str, &[FieldDoc])] = &[
    ("", Site::FIELDS),
    ("site", SiteInfo::FIELDS),
    ("site.tools", Tools::FIELDS),
    ("site.tools.<tool>", ToolSpec::FIELDS),
    ("scheduler", Scheduler::FIELDS),
    ("phase_defaults", PhaseArgs::FIELDS),
    ("targets[]", Target::FIELDS),
    ("targets[].phases", PhaseArgs::FIELDS),
    ("targets[].route", Route::FIELDS),
    ("targets[].select", Select::FIELDS),
    ("targets[].hardware", Hardware::FIELDS),
    ("targets[].cpu_hardware", Hardware::FIELDS),
    ("targets[].build_types.<name>", BuildTypeOverride::FIELDS),
    ("filesystems", Filesystems::FIELDS),
    ("admission", Admission::FIELDS),
    ("shards", Shards::FIELDS),
    ("sources", Sources::FIELDS),
    ("modules", Modules::FIELDS),
    ("modules.eessi", Eessi::FIELDS),
    ("caches", Caches::FIELDS),
    ("caches.targets[]", CacheTarget::FIELDS),
    ("logs", Logs::FIELDS),
    ("logs.rotate", Rotate::FIELDS),
    ("rebuild", Rebuild::FIELDS),
    ("licences", Licences::FIELDS),
    ("licences.required", LicenceRequired::FIELDS),
];

/// Render the key reference as plain text.
pub fn reference() -> String {
    let mut out = String::new();
    out.push_str("Site file reference (kind = \"site\", schema_version = 1)\n");
    out.push_str("Unknown keys are errors. Absent optional sections are skipped.\n");
    for (path, fields) in SECTIONS {
        out.push('\n');
        out.push_str(&format!(
            "[{}]\n",
            if path.is_empty() { "top level" } else { path }
        ));
        for f in *fields {
            out.push_str(&format!(
                "  {} ({}, {}{})\n      {}\n",
                f.key,
                f.ty,
                if f.required { "required" } else { "optional" },
                if f.default.is_empty() {
                    String::new()
                } else {
                    format!(", default: {}", f.default)
                },
                f.doc
            ));
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Keys a struct accepts, read from serde's unknown-field error.
    fn accepted<T: serde::de::DeserializeOwned>() -> Vec<String> {
        let err = match toml::from_str::<T>("zz_probe = 1") {
            Ok(_) => panic!("probe key accepted"),
            Err(e) => e.to_string(),
        };
        let line = err
            .lines()
            .find(|l| l.contains("expected one of") || l.contains("expected `"))
            .unwrap_or_else(|| panic!("no key list in: {err}"));
        let rest = line.split("expected").nth(1).unwrap();
        let mut keys: Vec<String> = rest
            .split('`')
            .skip(1)
            .step_by(2)
            .map(str::to_string)
            .collect();
        keys.sort();
        keys
    }

    fn documented(f: &[FieldDoc]) -> Vec<String> {
        let mut k: Vec<String> = f.iter().map(|d| d.key.to_string()).collect();
        k.sort();
        k
    }

    macro_rules! check_all {
        ($($t:ident),* $(,)?) => {{
            let mut seen: Vec<Vec<String>> = Vec::new();
            $(
                assert_eq!(
                    accepted::<$t>(),
                    documented($t::FIELDS),
                    "{} fields and documentation differ",
                    stringify!($t)
                );
                seen.push(documented($t::FIELDS));
            )*
            seen
        }};
    }

    #[test]
    fn every_field_is_documented() {
        let seen = check_all!(
            Site,
            SiteInfo,
            Tools,
            ToolSpec,
            Scheduler,
            PhaseArgs,
            Target,
            Route,
            Select,
            Hardware,
            BuildTypeOverride,
            Filesystems,
            Admission,
            Shards,
            Sources,
            Modules,
            Eessi,
            Caches,
            CacheTarget,
            Logs,
            Rotate,
            Rebuild,
            Licences,
            LicenceRequired,
        );
        // Every documented table appears in the printed reference.
        for (_, fields) in SECTIONS {
            assert!(seen.contains(&documented(fields)));
        }
        let text = reference();
        for (_, fields) in SECTIONS {
            for f in *fields {
                assert!(!f.doc.is_empty() && !f.ty.is_empty(), "{}", f.key);
                assert!(text.contains(f.doc), "{} missing from reference", f.key);
            }
        }
    }

    #[test]
    fn required_keys_match_the_contract() {
        let req: Vec<&str> = Site::FIELDS
            .iter()
            .filter(|f| f.required)
            .map(|f| f.key)
            .collect();
        for k in ["site", "scheduler", "targets", "filesystems", "modules"] {
            assert!(req.contains(&k), "{k} must be required");
        }
        for k in [
            "admission",
            "shards",
            "sources",
            "caches",
            "logs",
            "rebuild",
            "licences",
            "phase_defaults",
        ] {
            assert!(!req.contains(&k), "{k} must be optional");
        }
    }
}
