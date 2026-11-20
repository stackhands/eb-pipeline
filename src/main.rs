use clap::{Args, CommandFactory, Parser, Subcommand, ValueEnum};
use clap_complete::{generate_to, shells};
use eb_pipeline::exit::{self, Fail};
use eb_pipeline::package_plan;
use eb_pipeline::plan::{build_plan, read_units, Planner};
use eb_pipeline::resolve::{explain, Scope};
use eb_pipeline::run::{apply_pending_signal, install_handlers, log, run, Options, RunResult};
use eb_pipeline::{
    audit, easystack, eessi, from_commit, metrics, notify, sbom, schema, site, stack_checks,
    summary, templates, tools, util, watch,
};
use serde::Serialize;
use std::path::{Path, PathBuf};
use std::time::Instant;

const ROOT_HELP: &str = "\
Examples:
  eb-pipeline init --template small-cluster --out site.toml
  eb-pipeline check --site site.toml
  eb-pipeline check --list stack.yml --check-from-commit --easyconfigs-repo DIR
  eb-pipeline check --site site.toml --gap-out DIR --generation NAME --shadow
  eb-pipeline targets --site site.toml --format json
  eb-pipeline plan --site site.toml --list list.txt --target cpu --build-type production --out plan.json
  eb-pipeline run --site site.toml --list list.txt --target cpu --build-type production --result result.json
  eb-pipeline watch --site site.toml --target cpu --build-type production --dry-run
  eb-pipeline audit --site site.toml --target cpu --findings findings.csv --out-dir audit
  eb-pipeline metrics --result result.json

Companion commands from this package:
  eb-retry-list
  eb-shard-weights
  eb-test-map
  eb-junit
  eb-cvmfs

Configuration: every flag above falls back to an EB_PIPELINE_<FLAG> variable
(EB_PIPELINE_SITE, EB_PIPELINE_LIST, EB_PIPELINE_PACKAGE_PLAN,
EB_PIPELINE_TARGET, EB_PIPELINE_BUILD_TYPE, EB_PIPELINE_RUN_ID,
EB_PIPELINE_LOG_STAMP, EB_PIPELINE_LISTS_DIR, EB_PIPELINE_RESULT); a flag on
the command line wins.
EB_PIPELINE__SECTION__KEY=value sets one site file value, for example
EB_PIPELINE__FILESYSTEMS__SCRATCH=/scratch/eb; the result is validated like the
file.

Output streams: stdout carries plan, explain, schema, init, status, targets and
metrics, and verify, and run --dry-run or watch --dry-run output only. Events go to stderr
as `LEVEL [phase] target: message`. The result JSON goes to --result, a run's
requested plan JSON to --plan-out, and each run's summary to summary.json and
summary.html in its run directory,
<log_root>/<log-stamp>/. Run --metrics-file atomically replaces a Prometheus
text file when the run ends. `metrics` exits 0 when it can read the file,
including when the run itself failed.

";

const WATCH_EXIT: &str = "\
Exit codes of watch:
  0    nothing to rebuild: no install is recorded, EESSI is as recorded, or no
       recorded install needs a rebuild; --dry-run printed the plan; --adopt
       recorded the installs; or the rebuild succeeded
  1    the rebuild failed, an eb-stack call failed, or the record, state
       file, plan or rebuild list could not be read or written
  2    invalid arguments or site file, or eb-stack below its min_version
  75   another watch of the target holds its lock, the module environment did
       not load, or the EESSI repository or a module tree cannot be read
  78   [watch] is off, the module environment loads no EESSI, recorded installs
       were built against another EESSI version or CPU subdirectory, eb-stack is
       missing, or no easyconfig tree has the easyconfig of a vanished module
  130  SIGINT
  143  SIGTERM
The rebuild's exit code is watch's, so its phases' 75 and 78 pass through.
";

#[derive(Parser)]
#[command(
    name = "eb-pipeline",
    version,
    about = "Site-neutral EasyBuild pipeline driver configured by a per-site TOML file",
    after_long_help = format!("{ROOT_HELP}{}", exit::TABLE),
    after_help = exit::TABLE,
)]
struct Cli {
    #[command(subcommand)]
    command: Cmd,
}

#[derive(Subcommand)]
enum Cmd {
    /// Parse and validate a site file.
    Check {
        /// Site file.
        #[arg(
            long,
            env = "EB_PIPELINE_SITE",
            required_unless_present = "check_from_commit"
        )]
        site: Option<PathBuf>,
        /// Directory that must hold every build list a target names.
        #[arg(long, env = "EB_PIPELINE_LISTS_DIR")]
        lists_dir: Option<PathBuf>,
        /// Run each needed tool's `--version` and enforce min_version.
        #[arg(long)]
        probe_tools: bool,
        /// Build list or easystack to pass to requested eb-stack checks.
        #[arg(long, env = "EB_PIPELINE_LIST")]
        list: Option<PathBuf>,
        /// Limit target-dependent settings to this target.
        #[arg(long, env = "EB_PIPELINE_TARGET", requires = "stack_checks")]
        target: Option<String>,
        /// Comma-separated eb-stack checks to run before any allocation.
        #[arg(long, value_delimiter = ',', requires = "list", value_enum)]
        stack_checks: Vec<stack_checks::StackCheck>,
        /// Ask `eb-stack stack from-commit` whether pins are already in the
        /// EasyBuild release tag.
        #[arg(long, requires = "list")]
        check_from_commit: bool,
        /// easybuild-easyconfigs checkout for `from-commit` pins.
        #[arg(long, requires = "check_from_commit")]
        easyconfigs_repo: Option<PathBuf>,
        /// easybuild-easyblocks checkout for `include-easyblocks-from-commit`.
        #[arg(long, requires = "check_from_commit")]
        easyblocks_repo: Option<PathBuf>,
        /// EasyBuild release `X.Y.Z`. Overrides `eb-X.Y.Z` in the list file name.
        #[arg(long, requires = "check_from_commit")]
        easybuild_version: Option<String>,
        #[command(flatten)]
        gap: eb_pipeline::gap::GapArgs,
    },
    /// Print the site file reference: every key, type, default and meaning.
    Schema,
    /// Write a starter site file from a built-in template.
    Init {
        /// Template name; `--list` prints the names.
        #[arg(long, required_unless_present = "list")]
        template: Option<String>,
        /// Print the template names and summaries.
        #[arg(long)]
        list: bool,
        /// Write here; absent or `-` prints on stdout. An existing file is refused.
        #[arg(long)]
        out: Option<PathBuf>,
        /// After writing a file, print next steps on stderr (also when `EB_PIPELINE_INIT_HINTS=1`).
        #[arg(long, env = "EB_PIPELINE_INIT_HINTS", value_parser = clap::builder::BoolishValueParser::new())]
        hints: bool,
    },
    /// Print the resolved configuration of one or all targets.
    Explain {
        /// Site file.
        #[arg(long, env = "EB_PIPELINE_SITE")]
        site: PathBuf,
        /// Target name; all targets when absent.
        #[arg(long, env = "EB_PIPELINE_TARGET")]
        target: Option<String>,
        /// Apply this build type's overrides.
        #[arg(long, env = "EB_PIPELINE_BUILD_TYPE")]
        build_type: Option<String>,
        /// Output format.
        #[arg(long, value_enum, default_value_t = Format::Toml)]
        format: Format,
    },
    /// Print the target names in file order, for a CI matrix or a shell loop.
    Targets {
        /// Site file.
        #[arg(long, env = "EB_PIPELINE_SITE")]
        site: PathBuf,
        /// `text`: one name per line; `json`: a JSON array on one line.
        #[arg(long, value_enum, default_value_t = TextOrJson::Text)]
        format: TextOrJson,
    },
    /// Print the last runs under the log root, newest first, from their summaries.
    Status {
        /// Site file.
        #[arg(long, env = "EB_PIPELINE_SITE")]
        site: PathBuf,
        /// Only the runs of this target, with only its entry.
        #[arg(long, env = "EB_PIPELINE_TARGET")]
        target: Option<String>,
        /// How many runs.
        #[arg(long, default_value_t = 10, value_parser = clap::value_parser!(u16).range(1..))]
        last: u16,
        /// Keep only nonzero runs and runs without a readable summary.
        #[arg(long)]
        problems_only: bool,
        /// `text`: a line per run and per target; `json`: one JSON document.
        #[arg(long, value_enum, default_value_t = TextOrJson::Text)]
        format: TextOrJson,
    },
    /// Print Prometheus text, or a short JSON status, for one result file.
    Metrics {
        /// Result file written by `run --result`.
        #[arg(long, env = "EB_PIPELINE_RESULT")]
        result: PathBuf,
        /// `prometheus`, the default, or `json`.
        #[arg(long, value_enum, default_value_t = StatusFormat::Prometheus)]
        format: StatusFormat,
    },
    /// Compute phases, launch lines, environments and tool calls without executing.
    Plan {
        #[command(flatten)]
        common: Common,
        /// Write the plan JSON here; `-` or absent prints it on stdout.
        #[arg(long)]
        out: Option<PathBuf>,
        /// Directory written by `stack shard`; expands its units and build lines.
        #[arg(long)]
        shard_plan: Option<PathBuf>,
    },
    /// Execute the plan's phases as subprocesses.
    #[command(after_long_help = exit::TABLE)]
    Run {
        #[command(flatten)]
        common: Common,
        /// Write the result JSON here.
        #[arg(long)]
        result: Option<PathBuf>,
        /// Write the plan computed by this run before starting any phase command.
        #[arg(long)]
        plan_out: Option<PathBuf>,
        /// Atomically write Prometheus metrics here when the run ends.
        ///
        /// Must differ from `--result` and `--resume-result`.
        #[arg(long)]
        metrics_file: Option<PathBuf>,
        /// Directory written by `stack shard`; reuse its units without replanning.
        #[arg(long)]
        shard_plan: Option<PathBuf>,
        /// Print the commands that would run and start nothing.
        #[arg(long, conflicts_with = "resume_result")]
        dry_run: bool,
        /// Ask `eb-stack stack oom` whether any planned package exceeds this node.
        #[arg(long, requires = "node")]
        oom_preflight: bool,
        /// Node size for `--oom-preflight`, as `CORES:MEMORY_MIB[:TARGET]`.
        #[arg(long, requires = "oom_preflight")]
        node: Option<String>,
        /// Continue from this result when its input digest still matches.
        /// Needs `[resume] enabled`.
        #[arg(long, value_name = "FILE")]
        resume_result: Option<PathBuf>,
    },
    /// Recompute the input record and check its inclusion proof.
    Verify {
        /// Site file.
        #[arg(long, env = "EB_PIPELINE_SITE")]
        site: PathBuf,
        /// Build list file, or an EasyBuild easystack.
        #[arg(
            long,
            env = "EB_PIPELINE_LIST",
            required_unless_present = "recorded_list",
            conflicts_with = "recorded_list"
        )]
        list: Option<PathBuf>,
        /// Read the list path recorded in the plan or result.
        #[arg(long)]
        recorded_list: bool,
        /// Target name.
        #[arg(long, env = "EB_PIPELINE_TARGET")]
        target: String,
        /// Build type name.
        #[arg(long, env = "EB_PIPELINE_BUILD_TYPE")]
        build_type: String,
        /// Plan or result JSON that holds `input_attestation`.
        #[arg(long)]
        from: PathBuf,
    },
    /// Rebuild the recorded installs an EESSI change reaches, in dependency order.
    #[command(after_long_help = WATCH_EXIT)]
    Watch {
        /// Site file.
        #[arg(long, env = "EB_PIPELINE_SITE")]
        site: PathBuf,
        /// Target name.
        #[arg(long, env = "EB_PIPELINE_TARGET")]
        target: String,
        /// Build type of the rebuild (selects per-build-type overrides).
        #[arg(long, env = "EB_PIPELINE_BUILD_TYPE")]
        build_type: String,
        /// Run label of the rebuild. Default: the log stamp.
        #[arg(long, env = "EB_PIPELINE_RUN_ID")]
        run_id: Option<String>,
        /// Timestamp directory of the rebuild under the log root, as for
        /// `run`. Default: the current UTC time.
        #[arg(long, env = "EB_PIPELINE_LOG_STAMP")]
        log_stamp: Option<String>,
        /// Print the plan and rebuild nothing.
        #[arg(long)]
        dry_run: bool,
        /// Record every install in the target's module tree against the EESSI
        /// loaded now, in place of the record, and rebuild nothing.
        #[arg(long)]
        adopt: bool,
        /// Write the plan JSON here.
        #[arg(long)]
        plan_out: Option<PathBuf>,
        /// Write the rebuild's result JSON here.
        #[arg(long)]
        result: Option<PathBuf>,
    },
    /// Run eb-stack vulnerability checks and collect rebuild lists.
    Audit {
        /// Site file.
        #[arg(long, env = "EB_PIPELINE_SITE")]
        site: PathBuf,
        /// Target name.
        #[arg(long, env = "EB_PIPELINE_TARGET")]
        target: String,
        /// Vulnerability scanner CSV for `eb-stack rebuild-set --findings`.
        #[arg(long, required_unless_present = "cve", conflicts_with = "cve")]
        findings: Option<PathBuf>,
        /// CVE id to audit. Repeat for multiple advisories.
        #[arg(long, required_unless_present = "findings")]
        cve: Vec<String>,
        /// Exit non-zero when eb-stack reports backport items.
        #[arg(long)]
        strict: bool,
        /// Output directory for audit artifacts and copied rebuild lists.
        #[arg(long)]
        out_dir: PathBuf,
    },
    /// Generate shell completion scripts from the clap command definition.
    #[command(hide = true)]
    Completions {
        /// Target shell.
        #[arg(long, value_enum)]
        shell: CompletionShell,
        /// Output directory.
        #[arg(long)]
        out_dir: PathBuf,
    },
    /// Generate a section 1 man page from the clap command definition.
    #[command(hide = true)]
    Man {
        /// Output directory.
        #[arg(long)]
        out_dir: PathBuf,
    },
}

#[derive(Args)]
struct Common {
    /// Site file.
    #[arg(long, env = "EB_PIPELINE_SITE")]
    site: PathBuf,
    /// Build list file: one `Name-version-toolchain.eb [eb options]` per line,
    /// or an EasyBuild easystack, a `.yml` or `.yaml` file.
    #[arg(
        long,
        env = "EB_PIPELINE_LIST",
        required_unless_present = "package_plan",
        conflicts_with = "package_plan"
    )]
    list: Option<PathBuf>,
    /// Directory written by `eb-stack package plan`; builds its emitted recipes.
    #[arg(
        long,
        env = "EB_PIPELINE_PACKAGE_PLAN",
        required_unless_present = "list",
        conflicts_with = "list"
    )]
    package_plan: Option<PathBuf>,
    /// Run ecosystem-native checks after a package-plan build.
    #[arg(long, requires = "package_plan")]
    verify_package_plan: bool,
    /// Target name.
    #[arg(long, env = "EB_PIPELINE_TARGET")]
    target: String,
    /// Build type name (selects per-build-type overrides).
    #[arg(long, env = "EB_PIPELINE_BUILD_TYPE")]
    build_type: String,
    /// Run label recorded in the result and passed to tools; a staged rebuild
    /// line passes `<run-id>-<easyconfig stem>`. Default: the log stamp for
    /// `run`, `<run-id>` for `plan`.
    #[arg(long, env = "EB_PIPELINE_RUN_ID")]
    run_id: Option<String>,
    /// Timestamp directory under the log root: a UTC time as
    /// `YYYYMMDD_HHMMSS`, the name `eb-keep-logs rotate` ages. Default: the
    /// current UTC time, taken once per run; `<log-stamp>` for `plan`.
    #[arg(long, env = "EB_PIPELINE_LOG_STAMP")]
    log_stamp: Option<String>,
    /// Build a checksum-indexed bundle of prefetched and configured assets.
    #[arg(long)]
    offline_bundle: Option<PathBuf>,
    /// Run phases after source prefetch through sources.offline_wrapper.
    #[arg(long, requires = "offline_bundle")]
    require_offline: bool,
}

#[derive(Clone, Copy, ValueEnum)]
enum Format {
    Toml,
    Json,
}

#[derive(Clone, Copy, ValueEnum)]
enum TextOrJson {
    Text,
    Json,
}

/// What `metrics` prints.
#[derive(Clone, Copy, ValueEnum)]
enum StatusFormat {
    Prometheus,
    Json,
}

#[derive(Clone, Copy, ValueEnum)]
enum CompletionShell {
    Bash,
    Zsh,
    Fish,
}

fn to_sorted<T: Serialize>(v: &T) -> serde_json::Value {
    serde_json::to_value(v).expect("serializable")
}

fn write_stdout_text(text: &str) -> Result<(), Fail> {
    util::write_stdout(text.as_bytes())
        .map_err(|e| Fail::new(exit::FAILED, format!("cannot write stdout: {e}")))
}

fn write_stderr_text(text: &str) -> Result<(), Fail> {
    use std::io::Write;
    std::io::stderr()
        .lock()
        .write_all(text.as_bytes())
        .map_err(|e| Fail::new(exit::FAILED, format!("cannot write stderr: {e}")))
}

fn write_json(v: &serde_json::Value, path: Option<&Path>) -> Result<(), Fail> {
    let text = serde_json::to_string_pretty(v).expect("serializable") + "\n";
    match path {
        Some(p) if p != Path::new("-") => util::write_atomic(p, text.as_bytes())
            .map_err(|e| Fail::new(exit::FAILED, format!("cannot write {}: {e}", p.display()))),
        _ => write_stdout_text(&text),
    }
}

fn load_valid(path: &Path, lists_dir: Option<&Path>) -> Result<schema::Site, Fail> {
    let s = site::load(path)?;
    let errs = site::validate(&s, lists_dir);
    if errs.is_empty() {
        Ok(s)
    } else {
        for e in &errs {
            log("ERROR", "check", &s.site.name, e);
        }
        Err(Fail::usage(format!(
            "{} problem(s) in the site file",
            errs.len()
        )))
    }
}

/// The site file as the commands of a run get it in `EB_PIPELINE_SITE`:
/// absolute, so that it names the same file from any directory.
fn site_file(path: &Path) -> Option<String> {
    std::path::absolute(path)
        .ok()
        .and_then(|p| p.to_str().map(str::to_string))
}

struct OptInChecks<'a> {
    from_commit: Option<(&'a Path, from_commit::Options<'a>)>,
    gap: &'a eb_pipeline::gap::GapArgs,
}

fn cmd_check(
    site_path: Option<&Path>,
    lists_dir: Option<&Path>,
    probe: bool,
    list: Option<&Path>,
    target: Option<&str>,
    stack_checks: &[stack_checks::StackCheck],
    opt_in: OptInChecks<'_>,
) -> Result<i32, Fail> {
    validate_option_values(&[("--generation", opt_in.gap.generation.as_deref())])?;
    if let Some((_, opts)) = &opt_in.from_commit {
        validate_option_values(&[("--easybuild-version", opts.easybuild_version)])?;
    }
    let site = match site_path {
        Some(path) => Some(load_valid(path, lists_dir)?),
        None if opt_in.from_commit.is_none() || !stack_checks.is_empty() => {
            return Err(Fail::usage(
                "pass --site or --check-from-commit with --list",
            ));
        }
        None => None,
    };
    let mut code = exit::OK;
    if let Some(s) = &site {
        let m = &s.modules;
        if let Some(first) = m
            .init
            .first()
            .filter(|f| m.unuse_path && !f.starts_with('/'))
        {
            log(
                "WARN",
                "check",
                &s.site.name,
                &format!(
                    "modules.unuse_path clears MODULEPATH after shell_init, so `{first}` has no path to load from; load it by absolute path or drop unuse_path"
                ),
            );
        }
        if let Some(a) = s.admission_on() {
            if a.required && (a.quota_cmd.is_none() || a.reservations.is_none()) {
                log(
                    "WARN",
                    "check",
                    &s.site.name,
                    "admission.required is set without quota_cmd and reservations; `run` will refuse with exit 78",
                );
            }
            if a.live_jobs_cmd.is_none() && a.quota_cmd.is_some() && a.reservations.is_some() {
                log(
                    "WARN",
                    "check",
                    &s.site.name,
                    "admission.live_jobs_cmd is unset; stack admit then counts every other job's reservation, including those of jobs that ended without removing theirs",
                );
            }
        }
        if probe {
            tools::check_versions(s, true)?;
        }
        if !stack_checks.is_empty() {
            stack_checks::run(
                s,
                list.expect("clap requires --list with --stack-checks"),
                target,
                stack_checks,
            )?;
        }
        eb_pipeline::gap::run(s, opt_in.gap)?;
        log("INFO", "check", &s.site.name, "site file is valid");
    }
    if let Some((list, opts)) = opt_in.from_commit {
        let fc = from_commit::run(site.as_ref(), list, &opts)?;
        if fc != exit::OK {
            code = fc;
        }
    }
    Ok(code)
}

fn cmd_explain(
    site_path: &Path,
    target: Option<&str>,
    build_type: Option<&str>,
    format: Format,
) -> Result<i32, Fail> {
    validate_option_values(&[("--build-type", build_type)])?;
    let s = load_valid(site_path, None)?;
    let v = explain(&s, site_file(site_path).as_deref(), target, build_type)?;
    match format {
        Format::Json => write_json(&v, None)?,
        Format::Toml => {
            let rendered = toml::to_string_pretty(&v)
                .map_err(|e| Fail::new(exit::FAILED, format!("cannot render TOML: {e}")))?;
            write_stdout_text(&rendered)?;
        }
    }
    Ok(exit::OK)
}

fn cmd_targets(site_path: &Path, format: TextOrJson) -> Result<i32, Fail> {
    let s = load_valid(site_path, None)?;
    let names: Vec<&str> = s.targets.iter().map(|t| t.name.as_str()).collect();
    match format {
        TextOrJson::Text => {
            let mut out = String::new();
            for n in &names {
                out.push_str(n);
                out.push('\n');
            }
            write_stdout_text(&out)?;
        }
        // Compact, unlike write_json: GITHUB_OUTPUT and the like take one `name=value` line.
        TextOrJson::Json => {
            let line = format!("{}\n", serde_json::to_string(&names).expect("serializable"));
            write_stdout_text(&line)?;
        }
    }
    Ok(exit::OK)
}

fn cmd_status(
    site_path: &Path,
    target: Option<&str>,
    last: u16,
    problems_only: bool,
    format: TextOrJson,
) -> Result<i32, Fail> {
    let s = load_valid(site_path, None)?;
    if let Some(t) = target {
        s.target(t)?;
    }
    let root = Path::new(&s.filesystems.log_root);
    let runs = if problems_only {
        summary::problem_runs(root, target, last.into())
    } else {
        summary::runs(root, target, last.into())
    }
    .map_err(|e| Fail::new(exit::FAILED, e))?;
    if runs.is_empty() {
        let what = if problems_only {
            "no problematic runs under"
        } else {
            "no runs under"
        };
        log(
            "INFO",
            "status",
            target.unwrap_or("-"),
            &format!("{what} {}", root.display()),
        );
    }
    match format {
        TextOrJson::Text => write_stdout_text(&summary::status_text(&runs))?,
        TextOrJson::Json => write_json(&summary::status_json(root, &runs), None)?,
    }
    Ok(exit::OK)
}

fn cmd_metrics(path: &Path, format: StatusFormat) -> Result<i32, Fail> {
    let (_, value) = exit::read_json_output(path, "eb-pipeline run", exit::USAGE)?;
    let status = metrics::from_result(&value)?;
    match format {
        StatusFormat::Prometheus => write_stdout_text(&metrics::prometheus(&status))?,
        StatusFormat::Json => write_stdout_text(&metrics::status_json(&status))?,
    }
    Ok(exit::OK)
}

fn cmd_audit(
    site_path: &Path,
    target_name: &str,
    findings: Option<&Path>,
    cves: &[String],
    strict: bool,
    out_dir: &Path,
) -> Result<i32, Fail> {
    for cve in cves {
        validate_option_values(&[("--cve", Some(cve))])?;
    }
    let s = load_valid(site_path, None)?;
    let target = s.target(target_name)?;
    audit::run(audit::Args {
        site: &s,
        site_file: site_file(site_path),
        target,
        findings,
        cves,
        strict,
        out_dir,
    })
}

fn cmd_init(
    template: Option<&str>,
    list: bool,
    out: Option<&Path>,
    hints: bool,
) -> Result<i32, Fail> {
    if list {
        let mut out = String::new();
        for t in templates::TEMPLATES {
            out.push_str(&format!("{:<16} {}\n", t.name, t.summary));
        }
        write_stdout_text(&out)?;
        return Ok(exit::OK);
    }
    let name = template.unwrap_or_default();
    let t = templates::get(name).ok_or_else(|| {
        Fail::usage(format!(
            "unknown template `{name}`; known: {}",
            templates::TEMPLATES
                .iter()
                .map(|t| t.name)
                .collect::<Vec<_>>()
                .join(", ")
        ))
    })?;
    let wrote_path = match out {
        None => {
            write_stdout_text(t.text)?;
            None
        }
        Some(p) if p == Path::new("-") => {
            write_stdout_text(t.text)?;
            None
        }
        Some(p) => {
            use std::io::Write;
            let mut f = std::fs::OpenOptions::new()
                .write(true)
                .create_new(true)
                .open(p)
                .map_err(|e| {
                    let message = format!("cannot create {}: {e}", p.display());
                    if e.kind() == std::io::ErrorKind::AlreadyExists {
                        Fail::usage(message)
                    } else {
                        Fail::new(exit::FAILED, message)
                    }
                })?;
            let written = f.write_all(t.text.as_bytes()).and_then(|()| f.sync_all());
            drop(f);
            if let Err(e) = written {
                let _ = std::fs::remove_file(p);
                return Err(Fail::new(
                    exit::FAILED,
                    format!("cannot write {}: {e}", p.display()),
                ));
            }
            write_stderr_text(&format!(
                "INFO [init] -: wrote template {} to {}\n",
                t.name,
                p.display()
            ))?;
            Some(p)
        }
    };
    if hints {
        if let Some(p) = wrote_path {
            write_stderr_text(&format!(
                "INFO [init] hint: check the site file with `eb-pipeline check --site {}`\n",
                p.display()
            ))?;
            if t.name == "eessi-site" || t.name == "small-cluster" {
                write_stderr_text(&format!(
                    "INFO [init] hint: list target names with `eb-pipeline targets --site {}` (needs a build from the main branch; release 0.3.0 has no targets command)\n",
                    p.display()
                ))?;
                write_stderr_text(
                    "INFO [init] hint: see docs/orgmode/howto/install.org to install from source\n",
                )?;
            }
        } else {
            write_stderr_text(
                "INFO [init] hint: write the template with --out site.toml to print next steps\n",
            )?;
        }
    }
    Ok(exit::OK)
}

fn cmd_completions(shell: CompletionShell, out_dir: &Path) -> Result<i32, Fail> {
    let mut cmd = Cli::command();
    std::fs::create_dir_all(out_dir).map_err(|e| {
        Fail::new(
            exit::FAILED,
            format!("cannot create {}: {e}", out_dir.display()),
        )
    })?;
    match shell {
        CompletionShell::Bash => generate_to(shells::Bash, &mut cmd, "eb-pipeline", out_dir)
            .map_err(|e| {
                Fail::new(
                    exit::FAILED,
                    format!("cannot write completions in {}: {e}", out_dir.display()),
                )
            })?,
        CompletionShell::Zsh => generate_to(shells::Zsh, &mut cmd, "eb-pipeline", out_dir)
            .map_err(|e| {
                Fail::new(
                    exit::FAILED,
                    format!("cannot write completions in {}: {e}", out_dir.display()),
                )
            })?,
        CompletionShell::Fish => generate_to(shells::Fish, &mut cmd, "eb-pipeline", out_dir)
            .map_err(|e| {
                Fail::new(
                    exit::FAILED,
                    format!("cannot write completions in {}: {e}", out_dir.display()),
                )
            })?,
    };
    Ok(exit::OK)
}

fn cmd_man(out_dir: &Path) -> Result<i32, Fail> {
    std::fs::create_dir_all(out_dir).map_err(|e| {
        Fail::new(
            exit::FAILED,
            format!("cannot create {}: {e}", out_dir.display()),
        )
    })?;
    let cmd = Cli::command();
    let man = clap_mangen::Man::new(cmd.clone());
    let mut out = Vec::new();
    man.render(&mut out).map_err(|e| {
        Fail::new(
            exit::FAILED,
            format!("cannot render man page for {}: {e}", cmd.get_name()),
        )
    })?;
    let path = out_dir.join("eb-pipeline.1");
    std::fs::write(&path, out).map_err(|e| {
        Fail::new(
            exit::FAILED,
            format!("cannot write {}: {e}", path.display()),
        )
    })?;
    Ok(exit::OK)
}

fn timestamp_id() -> String {
    chrono::Utc::now().format("%Y%m%d_%H%M%S").to_string()
}

struct Mode<'a> {
    out: Option<&'a Path>,
    result: Option<&'a Path>,
    plan_out: Option<&'a Path>,
    metrics_file: Option<&'a Path>,
    shard_plan: Option<&'a Path>,
    dry: bool,
    is_run: bool,
    oom_node: Option<&'a str>,
    resume_result: Option<&'a Path>,
}

fn validate_option_values(values: &[(&str, Option<&str>)]) -> Result<(), Fail> {
    for &(option, value) in values {
        let Some(value) = value else {
            continue;
        };
        if value.trim().is_empty() || value.chars().any(char::is_control) {
            return Err(Fail::usage(format!(
                "{option} must be non-empty and contain no control characters"
            )));
        }
    }
    Ok(())
}

fn cmd_plan_or_run(c: &Common, m: Mode) -> Result<i32, Fail> {
    let Mode {
        out,
        result,
        plan_out,
        metrics_file,
        shard_plan,
        dry,
        is_run,
        oom_node,
        resume_result,
    } = m;
    validate_option_values(&[
        ("--build-type", Some(c.build_type.as_str())),
        ("--run-id", c.run_id.as_deref()),
        ("--node", oom_node),
    ])?;
    let source_inputs = [
        ("--site", Some(c.site.as_path())),
        ("--list", c.list.as_deref()),
    ];
    let refuse_alias = |output_option, output, inputs: &[(&str, Option<&Path>)]| {
        util::refuse_output_alias(output_option, output, inputs).map_err(Fail::usage)
    };
    refuse_alias("--out", out, &source_inputs)?;
    refuse_alias("--result", result, &source_inputs)?;
    if let Some(plan_out) = plan_out {
        if plan_out == Path::new("-") {
            return Err(Fail::usage("--plan-out must name a file"));
        }
        refuse_alias(
            "--plan-out",
            Some(plan_out),
            &[
                ("--site", Some(c.site.as_path())),
                ("--list", c.list.as_deref()),
                ("--result", result),
                ("--metrics-file", metrics_file),
                ("--resume-result", resume_result),
            ],
        )?;
    }
    if let Some(metrics_file) = metrics_file {
        refuse_alias(
            "--metrics-file",
            Some(metrics_file),
            &[
                ("--site", Some(c.site.as_path())),
                ("--list", c.list.as_deref()),
                ("--result", result),
                ("--resume-result", resume_result),
            ],
        )?;
    }
    if let Some(stamp) = c.log_stamp.as_deref().filter(|s| !util::is_log_stamp(s)) {
        return Err(Fail::usage(format!(
            "--log-stamp `{stamp}` is not a UTC time as YYYYMMDD_HHMMSS"
        )));
    }
    let (s, errs) = {
        let s = site::load(&c.site)?;
        let e = site::validate(&s, None);
        (s, e)
    };
    if !errs.is_empty() {
        for e in &errs {
            log("ERROR", "check", &s.site.name, e);
        }
        // A licence gate failure refuses the run; every other problem is invalid input.
        let licence_only = errs.iter().all(|e| e.starts_with("licences"));
        let code = if is_run && licence_only {
            exit::REFUSED
        } else {
            exit::USAGE
        };
        return Err(Fail::new(
            code,
            format!("{} problem(s) in the site file", errs.len()),
        ));
    }
    let target = s.target(&c.target)?;
    let sources = s.sources_on();
    if c.offline_bundle.is_some() {
        if is_run && c.offline_bundle.as_ref().is_some_and(|path| path.exists()) {
            return Err(Fail::usage(
                "--offline-bundle must name an absent path for run",
            ));
        }
        let Some(conf) = sources else {
            return Err(Fail::usage(
                "--offline-bundle needs an enabled [sources] table with prefetch = true",
            ));
        };
        let easyconfigs = if conf.easyconfigs.is_empty() {
            s.shards
                .as_ref()
                .map(|shards| shards.easyconfigs.as_slice())
                .unwrap_or_default()
        } else {
            conf.easyconfigs.as_slice()
        };
        if easyconfigs.is_empty() {
            return Err(Fail::usage(
                "--offline-bundle needs sources.easyconfigs or shards.easyconfigs",
            ));
        }
        if s.filesystems.source_paths.is_empty() {
            return Err(Fail::usage(
                "--offline-bundle needs filesystems.source_paths",
            ));
        }
        if c.require_offline && conf.offline_wrapper.is_empty() {
            return Err(Fail::usage(
                "--require-offline needs sources.offline_wrapper",
            ));
        }
    }
    let package_input = c
        .package_plan
        .as_deref()
        .map(package_plan::load)
        .transpose()?;
    let (input_name, lines) = if let Some(input) = &package_input {
        let name = input.bundle.to_str().ok_or_else(|| {
            Fail::usage("--package-plan path is not valid UTF-8 and cannot be represented in JSON")
        })?;
        (name.to_string(), input.lines.clone())
    } else {
        let list = c.list.as_deref().expect("clap requires one input");
        let lines = if easystack::is_easystack(list) {
            easystack::read(list).map_err(Fail::usage)?
        } else {
            let list_text = util::read_text_file(list)
                .map_err(|e| Fail::usage(format!("cannot read list {}: {e}", list.display())))?;
            util::parse_list(&list_text)?
        };
        let name = list.to_str().ok_or_else(|| {
            Fail::usage("--list path is not valid UTF-8 and cannot be represented in JSON")
        })?;
        (name.to_string(), lines)
    };
    if lines.is_empty() {
        return Err(Fail::usage(format!("input {input_name} holds no lines")));
    }
    let log_stamp = c.log_stamp.clone().unwrap_or_else(|| {
        if is_run {
            timestamp_id()
        } else {
            "<log-stamp>".into()
        }
    });
    let run_id = c.run_id.clone().unwrap_or_else(|| {
        if is_run {
            log_stamp.clone()
        } else {
            "<run-id>".into()
        }
    });
    let scope = Scope {
        site: &s,
        site_file: site_file(&c.site),
        target,
        build_type: Some(&c.build_type),
        run_id,
        log_stamp,
    };
    let mut planner = Planner::new(scope, input_name);
    planner.offline_bundle = c.offline_bundle.clone();
    if c.require_offline {
        planner.offline_wrapper = sources
            .expect("offline bundle checked sources")
            .offline_wrapper
            .clone();
    }
    planner.package_plan = package_input;
    planner.verify_package_plan = c.verify_package_plan;
    if !is_run {
        let units = match shard_plan {
            Some(d) => Some(read_units(d)?),
            None => None,
        };
        let mut plan = build_plan(&planner, &lines, units.as_deref())?;
        if let Some(conf) = s.attest_on() {
            let Some(list_path) = c.list.as_deref() else {
                return Err(Fail::usage("attest needs --list"));
            };
            plan.input_attestation = Some(eb_pipeline::attest::append(
                &eb_pipeline::attest::Request {
                    site: &s,
                    attest: conf,
                    site_path: &c.site,
                    list_path,
                    lines: &lines,
                    target: &c.target,
                },
            )?);
        }
        write_json(&to_sorted(&plan), out)?;
        return Ok(exit::OK);
    }
    if !dry {
        tools::check_versions(&s, false)?;
        if c.offline_bundle.is_some() {
            tools::check_one(&s, tools::Tool::EbStack, true).map_err(Fail::usage)?;
        }
    }
    install_handlers();
    let before = (!dry && s.watch_on().is_some()).then(|| watch::listings(&planner, &lines));
    let saved_units = match shard_plan {
        Some(d) => Some(read_units(d)?),
        None => None,
    };
    let (prior, resume_refused) = match resume_result {
        Some(path) if !dry => {
            if s.resume_on().is_none() {
                return Err(Fail::usage("--resume-result needs [resume] enabled = true"));
            }
            match eb_pipeline::resume::load(path) {
                Ok(p) => (Some(p), None),
                Err(f) if f.code == exit::REFUSED => (None, Some(f.msg)),
                Err(f) => return Err(f),
            }
        }
        _ => (None, None),
    };
    let started_at = chrono::Utc::now();
    let began = Instant::now();
    let mut res = run(
        &planner,
        &lines,
        &Options {
            dry_run: dry,
            shard_units: saved_units.as_deref(),
            oom_node,
            resume: prior.as_ref(),
            resume_refused: resume_refused.as_deref(),
            plan_out,
        },
    );
    if let Some(m) = &res.message {
        log("ERROR", "run", &c.target, m);
    }
    if !dry && res.exit_code == 0 {
        if let Some(conf) = s.sbom_on() {
            match sbom::store(&s, conf, &summary::run_dir(&res)) {
                Ok(path) => log(
                    "INFO",
                    "sbom",
                    &c.target,
                    &format!("wrote {}", path.display()),
                ),
                Err(f) => {
                    log("ERROR", "sbom", &c.target, &f.msg);
                    res.message = Some(f.msg);
                    res.exit_code = f.code;
                }
            }
        }
    }
    if !dry {
        res.started_at = started_at.to_rfc3339();
        res.ended_at = chrono::Utc::now().to_rfc3339();
        res.duration_seconds = eb_pipeline::run::seconds(began.elapsed());
        if let Some(before) = &before {
            watch::record_run(&planner, &lines, &res, before);
        }
        write_summary_with_pending_signal(
            &s,
            &mut res,
            "run",
            result,
            None,
            &planner.offline_wrapper,
        );
        let value = to_sorted(&res);
        let mut output_errors = Vec::new();
        if let Some(p) = result {
            if let Err(f) = write_json(&value, Some(p)) {
                output_errors.push(f.msg);
            }
        }
        if let Some(p) = metrics_file {
            match metrics::from_result(&value) {
                Ok(status) => {
                    if let Err(e) = metrics::write_atomic(p, &metrics::prometheus(&status)) {
                        output_errors.push(format!("cannot write metrics {}: {e}", p.display()));
                    }
                }
                Err(f) => output_errors.push(f.msg),
            }
        }
        if !output_errors.is_empty() {
            return Err(signal_outranks(
                res.exit_code,
                Fail::new(exit::FAILED, output_errors.join("; ")),
            ));
        }
    }
    Ok(res.exit_code)
}

/// A signal ends a run with 130 or 143 whatever else went wrong, an output
/// file that could not be written included; the message is still printed.
fn signal_outranks(run_code: i32, failure: Fail) -> Fail {
    match run_code {
        exit::SIGINT | exit::SIGTERM => Fail::new(run_code, failure.msg),
        _ => failure,
    }
}

fn write_summary_with_pending_signal(
    s: &schema::Site,
    res: &mut RunResult,
    command: &str,
    result: Option<&Path>,
    watch: Option<serde_json::Value>,
    offline_wrapper: &[String],
) {
    apply_pending_signal(&mut res.exit_code, &mut res.interrupted_by);
    write_summary(s, res, command, result, watch, offline_wrapper);
    apply_pending_signal(&mut res.exit_code, &mut res.interrupted_by);
}

/// Merge a target's run into its run directory's summary; a problem is a
/// warning and leaves the run's exit code as it is.
fn write_summary(
    s: &schema::Site,
    res: &RunResult,
    command: &str,
    result: Option<&Path>,
    watch: Option<serde_json::Value>,
    offline_wrapper: &[String],
) {
    let dir = summary::run_dir(res);
    let entry = summary::entry(res, command, result, watch);
    match summary::record(&s.site.name, entry, &dir) {
        Ok(_) => {
            log(
                "INFO",
                "summary",
                &res.target,
                &format!(
                    "{} and {} written in {}",
                    summary::JSON,
                    summary::HTML,
                    dir.display()
                ),
            );
            notify::send(
                s,
                &res.target,
                res.exit_code,
                &dir.join(summary::JSON),
                offline_wrapper,
            );
        }
        Err(e) => log(
            "WARN",
            "summary",
            &res.target,
            &format!("no summary written: {e}"),
        ),
    }
}

struct WatchArgs<'a> {
    site: &'a Path,
    target: &'a str,
    build_type: &'a str,
    run_id: Option<&'a str>,
    log_stamp: Option<&'a str>,
    dry_run: bool,
    adopt: bool,
    plan_out: Option<&'a Path>,
    result: Option<&'a Path>,
}

fn cmd_watch(w: &WatchArgs) -> Result<i32, Fail> {
    validate_option_values(&[("--build-type", Some(w.build_type)), ("--run-id", w.run_id)])?;
    if let Some(stamp) = w.log_stamp.filter(|s| !util::is_log_stamp(s)) {
        return Err(Fail::usage(format!(
            "--log-stamp `{stamp}` is not a UTC time as YYYYMMDD_HHMMSS"
        )));
    }
    util::refuse_output_alias(
        "--plan-out",
        w.plan_out,
        &[("--site", Some(w.site)), ("--result", w.result)],
    )
    .map_err(Fail::usage)?;
    util::refuse_output_alias("--result", w.result, &[("--site", Some(w.site))])
        .map_err(Fail::usage)?;
    let s = load_valid(w.site, None)?;
    let target = s.target(w.target)?;
    let Some(conf) = s.watch_on() else {
        return Err(Fail::new(
            exit::REFUSED,
            "the site has no [watch] table, or it is disabled, so no install is recorded to watch",
        ));
    };
    let log_stamp = w.log_stamp.map_or_else(timestamp_id, str::to_string);
    // The selector chose the lines that built the recorded installs, and the
    // rebuild list holds lines without its option, such as a vanished EESSI
    // module's, which it would leave out.
    let target = &schema::Target {
        select: None,
        ..target.clone()
    };
    let scope = Scope {
        site: &s,
        site_file: site_file(w.site),
        target,
        build_type: Some(w.build_type),
        run_id: w.run_id.map_or_else(|| log_stamp.clone(), str::to_string),
        log_stamp,
    };
    let list = format!("{}/watch.list", scope.log_dir());
    let planner = Planner::new(scope, list.clone());
    let name = target.name.as_str();
    let say = |m: &str| log("INFO", "watch", name, m);
    let state_dir = planner.scope.watch_state_dir().expect("[watch] is on");
    install_handlers();
    let _lock = watch::hold(&state_dir, name)?;
    let work = watch::work_dir(&planner, "plan")?;
    let drop_work = || {
        let _ = std::fs::remove_dir_all(&work);
    };
    let (probe, env) = watch::probe(&planner, &work)?;
    let revision = watch::revision_of(&probe)?;
    watch::check_dirs(&probe)?;
    // A newer versions/YYYY.MM directory, while this module environment
    // still loads the older generation, is only detected. Choosing what to
    // rebuild for the new generation is eb-stack's, and watch does not ask.
    if !w.adopt {
        if let Some(repo) = &probe.repository {
            let versions = eessi::versions_dir(repo);
            match eessi::newer_generation(&versions, &probe.version) {
                Ok(Some(newer)) => {
                    drop_work();
                    return Err(Fail::new(
                        exit::TEMPFAIL,
                        format!(
                            "EESSI {newer} is a directory under {}, and the module environment of target {name} loads {}; watch detects that and does not ask eb-stack for a rebuild set",
                            versions.display(),
                            probe.version
                        ),
                    ));
                }
                Ok(None) => {}
                Err(e) => {
                    drop_work();
                    return Err(Fail::new(
                        exit::TEMPFAIL,
                        format!("cannot list {}: {e}", versions.display()),
                    ));
                }
            }
        }
    }
    let at = revision
        .map(|r| format!(" at revision {r}"))
        .unwrap_or_default();
    let eessi_now = format!("EESSI {} {}{at}", probe.version, probe.software_subdir);
    let local_dir = watch::local_modules_dir(&planner);
    if w.adopt {
        let found = eessi::scan(std::slice::from_ref(&local_dir));
        let installs: Vec<watch::ToRecord> = found
            .values()
            .filter_map(|m| {
                let ec = m.easyconfig.as_deref()?;
                Some(watch::ToRecord {
                    module: m.name.clone(),
                    easyconfig: ec.rsplit('/').next().unwrap_or(ec).to_string(),
                    args: Vec::new(),
                    how: "adopted",
                })
            })
            .collect();
        if found.len() > installs.len() {
            say(&format!(
                "{} module(s) in {local_dir} have no easyconfig in their install and are not adopted",
                found.len() - installs.len()
            ));
        }
        if w.dry_run {
            let mut out = String::new();
            for i in &installs {
                out.push_str(&i.module);
                out.push('\n');
            }
            write_stdout_text(&out)?;
            drop_work();
            say(&format!(
                "dry run: would adopt {} against {eessi_now}",
                installs.len()
            ));
            return Ok(exit::OK);
        }
        watch::need_eb_stack(&s)?;
        let names = watch::record(&planner, &work, &probe, revision, &installs, true)?;
        drop_work();
        say(&format!(
            "adopted {} against {eessi_now}: {}",
            names.len(),
            names.join(", ")
        ));
        return Ok(exit::OK);
    }
    let state = watch::load(&state_dir, name).map_err(|e| Fail::new(exit::FAILED, e))?;
    if state.installs.is_empty() {
        drop_work();
        log(
            "WARN",
            "watch",
            name,
            &format!(
                "no install is recorded in {}: a run with [watch] records what it builds, and watch --adopt what is installed",
                watch::state_path(&state_dir, name).display()
            ),
        );
        return Ok(exit::OK);
    }
    let other = watch::other_eessi(&state, &probe);
    if let Some((_, first)) = other.first() {
        if w.dry_run {
            let width = other.iter().map(|(m, _)| m.len()).max().unwrap_or(0);
            let mut out = String::new();
            for (m, i) in &other {
                out.push_str(&format!(
                    "{m:<width$}  built against EESSI {} {}\n",
                    i.eessi_version, i.software_subdir
                ));
            }
            let _ = write_stdout_text(&out);
        }
        drop_work();
        return Err(Fail::new(
            exit::REFUSED,
            format!(
                "the module environment of target {name} loads EESSI {} {}, and {} of {} recorded installs were built against EESSI {} {}; a rebuild in place would mix the two: build the stack for the new EESSI (eb-stack stack gap lists what it lacks), then record it with watch --adopt",
                probe.version,
                probe.software_subdir,
                other.len(),
                state.installs.len(),
                first.eessi_version,
                first.software_subdir
            ),
        ));
    }
    let print = |doc: &watch::PlanDoc| -> Result<(), Fail> {
        if w.dry_run {
            write_stdout_text(&doc.text())?;
        } else {
            doc.text().lines().for_each(say);
        }
        Ok(())
    };
    let abi_conf = conf.abi_drift.as_ref().filter(|a| a.enabled);
    let (abi_now, abi_fields) = if let Some(a) = abi_conf {
        let now = watch::abi_baseline(&planner, &work, a)?;
        let changed = watch::abi_changes(a, &state, &now);
        (Some(now), changed)
    } else {
        (None, Vec::new())
    };
    let abi_needs_update = abi_now
        .as_ref()
        .is_some_and(|a| state.abi_baseline.as_ref() != Some(a));
    let write_plan = |doc: &watch::PlanDoc| match w.plan_out {
        Some(path) => write_json(&to_sorted(doc), Some(path)),
        None => Ok(()),
    };
    if watch::unchanged_at(&state, &probe, revision) && abi_fields.is_empty() && !abi_needs_update {
        drop_work();
        let doc = watch::PlanDoc::unchanged(name, &probe, revision, &state);
        write_plan(&doc)?;
        print(&doc)?;
        return Ok(exit::OK);
    }
    let mut verdicts = watch::assess(&state, &probe.modules_dirs, &local_dir, conf.rebuild_review);
    let mut staged: Option<watch::Universe> = None;
    if !abi_fields.is_empty() {
        watch::need_eb_stack(&s)?;
        say(&format!(
            "host ABI changed in {} field(s): {}",
            abi_fields.len(),
            abi_fields
                .iter()
                .map(|f| f.field.as_str())
                .collect::<Vec<_>>()
                .join(", ")
        ));
        let u = watch::stage(
            &work,
            &eessi::scan(&probe.modules_dirs),
            &eessi::scan(std::slice::from_ref(&local_dir)),
        )?;
        watch::apply_abi_changes(
            &planner,
            &work,
            &u,
            &mut verdicts,
            &abi_fields,
            conf.rebuild_review,
            &probe.software_subdir,
        )?;
        staged = Some(u);
    }
    let mut doc;
    if verdicts.iter().any(|v| v.action == "rebuild") {
        if let Err(f) = watch::need_eb_stack(&s) {
            let doc = watch::PlanDoc::new(name, &probe, revision, verdicts);
            write_plan(&doc)?;
            print(&doc)?;
            return Err(Fail::new(
                f.code,
                format!(
                    "{}; without it neither the order of the rebuild nor the installs that depend on it are known, and nothing is rebuilt",
                    f.msg
                ),
            ));
        }
        let u = staged.take().unwrap_or(watch::stage(
            &work,
            &eessi::scan(&probe.modules_dirs),
            &eessi::scan(std::slice::from_ref(&local_dir)),
        )?);
        watch::cascade(
            &planner,
            &work,
            &u,
            &mut verdicts,
            conf.rebuild_review,
            &probe.software_subdir,
        )?;
        let vanished = verdicts
            .iter()
            .any(|v| v.action == "rebuild" && v.changes.iter().any(|c| c.change == "vanished"));
        let robot = if vanished {
            watch::robot_trees(conf, &env)
        } else {
            Vec::new()
        };
        if vanished && robot.is_empty() {
            let doc = watch::PlanDoc::new(name, &probe, revision, verdicts);
            write_plan(&doc)?;
            print(&doc)?;
            return Err(Fail::new(
                exit::REFUSED,
                "a dependency vanished from EESSI, and eb-stack has no easyconfig tree to plan its local build from: set [watch] easyconfigs, or load EasyBuild in [modules] init",
            ));
        }
        let ordered = watch::order(&planner, &work, &u, &verdicts, &robot)?;
        doc = watch::PlanDoc::new(name, &probe, revision, verdicts);
        doc.rebuild = ordered;
    } else {
        doc = watch::PlanDoc::new(name, &probe, revision, verdicts);
    }
    write_plan(&doc)?;
    if !conf.rebuild_review {
        for v in doc.installs.iter().filter(|v| v.action == "keep") {
            if let Some(c) = v.changes.iter().find(|c| c.class == "review") {
                log(
                    "WARN",
                    "watch",
                    name,
                    &format!(
                        "{} kept although {} {} ({}), as rebuild_review = false",
                        v.module, c.module, c.change, c.class
                    ),
                );
            }
        }
    }
    print(&doc)?;
    if w.dry_run {
        if doc.rebuild.is_empty() {
            drop_work();
        } else {
            say(&format!(
                "dry run: nothing rebuilt; eb-stack's files are in {}",
                work.display()
            ));
        }
        return Ok(exit::OK);
    }
    if doc.rebuild.is_empty() {
        if let Err(f) = watch::settle(
            &planner,
            &probe,
            revision,
            &doc.installs,
            None,
            abi_now.as_ref(),
        ) {
            log(
                "WARN",
                "watch",
                name,
                &format!("record not updated: {}", f.msg),
            );
        }
        drop_work();
        return Ok(exit::OK);
    }
    let log_dir = PathBuf::from(planner.scope.log_dir());
    std::fs::create_dir_all(&log_dir).map_err(|e| {
        Fail::new(
            exit::FAILED,
            format!("cannot create {}: {e}", log_dir.display()),
        )
    })?;
    std::fs::write(&list, doc.rebuild.join("\n") + "\n")
        .map_err(|e| Fail::new(exit::FAILED, format!("cannot write {list}: {e}")))?;
    write_json(&to_sorted(&doc), Some(&log_dir.join("watch-plan.json")))?;
    let lines = util::parse_list(&(doc.rebuild.join("\n") + "\n"))?;
    tools::check_versions(&s, false)?;
    let before = watch::listings(&planner, &lines);
    let mut res = run(
        &planner,
        &lines,
        &Options {
            dry_run: false,
            shard_units: None,
            oom_node: None,
            resume: None,
            resume_refused: None,
            plan_out: None,
        },
    );
    if let Some(m) = &res.message {
        log("ERROR", "run", name, m);
    }
    if res.interrupted_by.is_none() {
        let rebuild = Some((lines.as_slice(), &res, &before));
        match watch::settle(
            &planner,
            &probe,
            revision,
            &doc.installs,
            rebuild,
            abi_now.as_ref(),
        ) {
            Ok(names) if !names.is_empty() => say(&format!(
                "recorded {} against {eessi_now}: {}",
                names.len(),
                names.join(", ")
            )),
            Ok(_) => {}
            Err(f) => log(
                "WARN",
                "watch",
                name,
                &format!(
                    "record not updated: {}; the next watch plans from the record as it was",
                    f.msg
                ),
            ),
        }
        drop_work();
    }
    write_summary_with_pending_signal(&s, &mut res, "watch", w.result, Some(to_sorted(&doc)), &[]);
    if let Some(p) = w.result {
        write_json(&to_sorted(&res), Some(p)).map_err(|f| signal_outranks(res.exit_code, f))?;
    }
    Ok(res.exit_code)
}

fn cmd_verify(
    site_path: &Path,
    list_path: Option<&Path>,
    recorded_list: bool,
    target: &str,
    from: &Path,
) -> Result<i32, Fail> {
    let (s, errs) = {
        let s = site::load(site_path)?;
        let e = site::validate(&s, None);
        (s, e)
    };
    if !errs.is_empty() {
        for e in &errs {
            log("ERROR", "check", &s.site.name, e);
        }
        return Err(Fail::usage(format!(
            "{} problem(s) in the site file",
            errs.len()
        )));
    }
    let Some(conf) = s.attest_on() else {
        return Err(Fail::usage("verify needs [attest] enabled = true"));
    };
    let (_, doc) = exit::read_json_output(from, "eb-pipeline plan", exit::USAGE)?;
    let stored = match doc.get("input_attestation") {
        Some(v) => serde_json::from_value(v.clone()).map_err(|e| {
            Fail::usage(format!(
                "{} input_attestation is not a record and a proof: {e}",
                from.display()
            ))
        })?,
        None => {
            return Err(Fail::new(
                exit::REFUSED,
                format!("{} has no input_attestation", from.display()),
            ))
        }
    };
    let list = if recorded_list {
        doc.get("list")
            .and_then(serde_json::Value::as_str)
            .map(Path::new)
            .ok_or_else(|| {
                Fail::usage(format!(
                    "{} has no string `list` for --recorded-list",
                    from.display()
                ))
            })?
    } else {
        list_path.expect("clap requires --list or --recorded-list")
    };
    let lines = if easystack::is_easystack(list) {
        easystack::read(list).map_err(Fail::usage)?
    } else {
        let list_text = util::read_text_file(list)
            .map_err(|e| Fail::usage(format!("cannot read list {}: {e}", list.display())))?;
        util::parse_list(&list_text)?
    };
    if lines.is_empty() {
        return Err(Fail::usage(format!(
            "list {} holds no lines",
            list.display()
        )));
    }
    if s.target(target).is_err() {
        return Err(Fail::usage(format!("no target {target}")));
    }
    eb_pipeline::attest::verify(
        &eb_pipeline::attest::Request {
            site: &s,
            attest: conf,
            site_path,
            list_path: list,
            lines: &lines,
            target,
        },
        &stored,
    )?;
    write_stdout_text(&format!("verified input record {}\n", stored.record_sha256))?;
    Ok(exit::OK)
}

fn main() {
    let cli = match Cli::try_parse() {
        Ok(cli) => cli,
        Err(error) => match error.kind() {
            clap::error::ErrorKind::DisplayHelp | clap::error::ErrorKind::DisplayVersion => {
                let text = error.to_string();
                if let Err(write_error) = util::write_stdout(text.as_bytes()) {
                    eprintln!("ERROR [main] -: cannot write stdout: {write_error}");
                    std::process::exit(exit::FAILED);
                }
                std::process::exit(exit::OK);
            }
            _ => error.exit(),
        },
    };
    let r = match &cli.command {
        Cmd::Check {
            site,
            lists_dir,
            probe_tools,
            list,
            target,
            stack_checks,
            check_from_commit,
            easyconfigs_repo,
            easyblocks_repo,
            easybuild_version,
            gap,
        } => {
            let fc = check_from_commit.then(|| {
                (
                    list.as_deref().expect("--list with --check-from-commit"),
                    from_commit::Options {
                        easyconfigs_repo: easyconfigs_repo.as_deref(),
                        easyblocks_repo: easyblocks_repo.as_deref(),
                        easybuild_version: easybuild_version.as_deref(),
                    },
                )
            });
            cmd_check(
                site.as_deref(),
                lists_dir.as_deref(),
                *probe_tools,
                list.as_deref(),
                target.as_deref(),
                stack_checks,
                OptInChecks {
                    from_commit: fc,
                    gap,
                },
            )
        }
        Cmd::Schema => write_stdout_text(&schema::reference()).map(|()| exit::OK),
        Cmd::Init {
            template,
            list,
            out,
            hints,
        } => cmd_init(template.as_deref(), *list, out.as_deref(), *hints),
        Cmd::Explain {
            site,
            target,
            build_type,
            format,
        } => cmd_explain(site, target.as_deref(), build_type.as_deref(), *format),
        Cmd::Targets { site, format } => cmd_targets(site, *format),
        Cmd::Status {
            site,
            target,
            last,
            problems_only,
            format,
        } => cmd_status(site, target.as_deref(), *last, *problems_only, *format),
        Cmd::Metrics { result, format } => cmd_metrics(result, *format),
        Cmd::Plan {
            common,
            out,
            shard_plan,
        } => cmd_plan_or_run(
            common,
            Mode {
                out: out.as_deref(),
                result: None,
                plan_out: None,
                metrics_file: None,
                shard_plan: shard_plan.as_deref(),
                dry: false,
                is_run: false,
                oom_node: None,
                resume_result: None,
            },
        ),
        Cmd::Run {
            common,
            result,
            plan_out,
            metrics_file,
            shard_plan,
            dry_run,
            oom_preflight: _,
            node,
            resume_result,
        } => cmd_plan_or_run(
            common,
            Mode {
                out: None,
                result: result.as_deref(),
                plan_out: plan_out.as_deref(),
                metrics_file: metrics_file.as_deref(),
                shard_plan: shard_plan.as_deref(),
                dry: *dry_run,
                is_run: true,
                oom_node: node.as_deref(),
                resume_result: resume_result.as_deref(),
            },
        ),
        Cmd::Verify {
            site,
            list,
            recorded_list,
            target,
            build_type: _,
            from,
        } => cmd_verify(site, list.as_deref(), *recorded_list, target, from),
        Cmd::Watch {
            site,
            target,
            build_type,
            run_id,
            log_stamp,
            dry_run,
            adopt,
            plan_out,
            result,
        } => cmd_watch(&WatchArgs {
            site,
            target,
            build_type,
            run_id: run_id.as_deref(),
            log_stamp: log_stamp.as_deref(),
            dry_run: *dry_run,
            adopt: *adopt,
            plan_out: plan_out.as_deref(),
            result: result.as_deref(),
        }),
        Cmd::Audit {
            site,
            target,
            findings,
            cve,
            strict,
            out_dir,
        } => cmd_audit(site, target, findings.as_deref(), cve, *strict, out_dir),
        Cmd::Completions { shell, out_dir } => cmd_completions(*shell, out_dir),
        Cmd::Man { out_dir } => cmd_man(out_dir),
    };
    let code = match r {
        Ok(c) => c,
        Err(f) => {
            log("ERROR", "main", "-", &f.msg);
            f.code
        }
    };
    std::process::exit(code);
}
