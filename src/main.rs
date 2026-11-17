use clap::{Args, Parser, Subcommand, ValueEnum};
use eb_pipeline::exit::{self, Fail};
use eb_pipeline::plan::{build_plan, read_units, Planner};
use eb_pipeline::resolve::{explain, Scope};
use eb_pipeline::run::{install_handlers, log, run, Options, RunResult};
use eb_pipeline::{
    easystack, eessi, metrics, notify, sbom, schema, site, summary, templates, tools, util, watch,
};
use serde::Serialize;
use std::path::{Path, PathBuf};

const ROOT_HELP: &str = "\
Examples:
  eb-pipeline init --template small-cluster --out site.toml
  eb-pipeline check --site site.toml
  eb-pipeline targets --site site.toml --format json
  eb-pipeline plan --site site.toml --list list.txt --target cpu --build-type production --out plan.json
  eb-pipeline run --site site.toml --list list.txt --target cpu --build-type production --result result.json
  eb-pipeline watch --site site.toml --target cpu --build-type production --dry-run
  eb-pipeline metrics --result result.json

Configuration: every flag above falls back to an EB_PIPELINE_<FLAG> variable
(EB_PIPELINE_SITE, EB_PIPELINE_LIST, EB_PIPELINE_TARGET, EB_PIPELINE_BUILD_TYPE,
EB_PIPELINE_RUN_ID, EB_PIPELINE_LOG_STAMP, EB_PIPELINE_LISTS_DIR,
EB_PIPELINE_RESULT); a flag on the command line wins.
EB_PIPELINE__SECTION__KEY=value sets one site file value, for example
EB_PIPELINE__FILESYSTEMS__SCRATCH=/scratch/eb; the result is validated like the
file.

Output streams: stdout carries plan, explain, schema, init, status, targets and
metrics, and run --dry-run or watch --dry-run output only. Events go to stderr
as `LEVEL [phase] target: message`. The result JSON goes to --result, and each
run's summary to summary.json and summary.html in its run directory,
<log_root>/<log-stamp>/. `metrics` exits 0 when it can read the file, including
when the run itself failed.

";

const WATCH_EXIT: &str = "\
Exit codes of watch:
  0    nothing to rebuild: no install is recorded, EESSI is as recorded, or no
       recorded install needs a rebuild; --dry-run printed the plan; --adopt
       recorded the installs; or the rebuild succeeded
  1    the rebuild failed, or an eb-stack call, the record or the state file did
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
        #[arg(long, env = "EB_PIPELINE_SITE")]
        site: PathBuf,
        /// Directory that must hold every build list a target names.
        #[arg(long, env = "EB_PIPELINE_LISTS_DIR")]
        lists_dir: Option<PathBuf>,
        /// Run each needed tool's `--version` and enforce min_version.
        #[arg(long)]
        probe_tools: bool,
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
        /// Directory written by `stack shard`; reuse its units without replanning.
        #[arg(long)]
        shard_plan: Option<PathBuf>,
        /// Print the commands that would run and start nothing.
        #[arg(long)]
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
}

#[derive(Args)]
struct Common {
    /// Site file.
    #[arg(long, env = "EB_PIPELINE_SITE")]
    site: PathBuf,
    /// Build list file: one `Name-version-toolchain.eb [eb options]` per line,
    /// or an EasyBuild easystack, a `.yml` or `.yaml` file.
    #[arg(long, env = "EB_PIPELINE_LIST")]
    list: PathBuf,
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

fn to_sorted<T: Serialize>(v: &T) -> serde_json::Value {
    serde_json::to_value(v).expect("serializable")
}

fn write_json(v: &serde_json::Value, path: Option<&Path>) -> Result<(), Fail> {
    let text = serde_json::to_string_pretty(v).expect("serializable") + "\n";
    match path {
        Some(p) if p != Path::new("-") => std::fs::write(p, text)
            .map_err(|e| Fail::new(exit::FAILED, format!("cannot write {}: {e}", p.display()))),
        _ => {
            print!("{text}");
            Ok(())
        }
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

fn cmd_check(site_path: &Path, lists_dir: Option<&Path>, probe: bool) -> Result<i32, Fail> {
    let s = load_valid(site_path, lists_dir)?;
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
        tools::check_versions(&s, true)?;
    }
    log("INFO", "check", &s.site.name, "site file is valid");
    Ok(exit::OK)
}

fn cmd_explain(
    site_path: &Path,
    target: Option<&str>,
    build_type: Option<&str>,
    format: Format,
) -> Result<i32, Fail> {
    let s = load_valid(site_path, None)?;
    let v = explain(&s, site_file(site_path).as_deref(), target, build_type)?;
    match format {
        Format::Json => write_json(&v, None)?,
        Format::Toml => print!(
            "{}",
            toml::to_string_pretty(&v)
                .map_err(|e| Fail::new(exit::FAILED, format!("cannot render TOML: {e}")))?
        ),
    }
    Ok(exit::OK)
}

fn cmd_targets(site_path: &Path, format: TextOrJson) -> Result<i32, Fail> {
    let s = load_valid(site_path, None)?;
    let names: Vec<&str> = s.targets.iter().map(|t| t.name.as_str()).collect();
    match format {
        TextOrJson::Text => {
            for n in &names {
                println!("{n}");
            }
        }
        // Compact, unlike write_json: GITHUB_OUTPUT and the like take one `name=value` line.
        TextOrJson::Json => {
            println!("{}", serde_json::to_string(&names).expect("serializable"))
        }
    }
    Ok(exit::OK)
}

fn cmd_status(
    site_path: &Path,
    target: Option<&str>,
    last: u16,
    format: TextOrJson,
) -> Result<i32, Fail> {
    let s = load_valid(site_path, None)?;
    if let Some(t) = target {
        s.target(t)?;
    }
    let root = Path::new(&s.filesystems.log_root);
    let runs = summary::runs(root, target, last.into()).map_err(|e| Fail::new(exit::FAILED, e))?;
    if runs.is_empty() {
        log(
            "INFO",
            "status",
            target.unwrap_or("-"),
            &format!("no runs under {}", root.display()),
        );
    }
    match format {
        TextOrJson::Text => print!("{}", summary::status_text(&runs)),
        TextOrJson::Json => write_json(&summary::status_json(root, &runs), None)?,
    }
    Ok(exit::OK)
}

fn cmd_metrics(path: &Path, format: StatusFormat) -> Result<i32, Fail> {
    let text = std::fs::read_to_string(path)
        .map_err(|e| Fail::usage(format!("cannot read {}: {e}", path.display())))?;
    let value: serde_json::Value = serde_json::from_str(&text)
        .map_err(|e| Fail::usage(format!("cannot parse {}: {e}", path.display())))?;
    let status = metrics::from_result(&value)?;
    match format {
        StatusFormat::Prometheus => print!("{}", metrics::prometheus(&status)),
        StatusFormat::Json => print!("{}", metrics::status_json(&status)),
    }
    Ok(exit::OK)
}

fn cmd_init(
    template: Option<&str>,
    list: bool,
    out: Option<&Path>,
    hints: bool,
) -> Result<i32, Fail> {
    if list {
        for t in templates::TEMPLATES {
            println!("{:<16} {}", t.name, t.summary);
        }
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
            print!("{}", t.text);
            None
        }
        Some(p) if p == Path::new("-") => {
            print!("{}", t.text);
            None
        }
        Some(p) => {
            use std::io::Write;
            let mut f = std::fs::OpenOptions::new()
                .write(true)
                .create_new(true)
                .open(p)
                .map_err(|e| Fail::usage(format!("cannot create {}: {e}", p.display())))?;
            f.write_all(t.text.as_bytes()).map_err(|e| {
                Fail::new(exit::FAILED, format!("cannot write {}: {e}", p.display()))
            })?;
            log(
                "INFO",
                "init",
                "-",
                &format!("wrote template {} to {}", t.name, p.display()),
            );
            Some(p)
        }
    };
    if hints {
        if let Some(p) = wrote_path {
            eprintln!(
                "INFO [init] hint: check the site file with `eb-pipeline check --site {}`",
                p.display()
            );
            if t.name == "eessi-site" || t.name == "small-cluster" {
                eprintln!(
                    "INFO [init] hint: list target names with `eb-pipeline targets --site {}` (needs a build from the main branch; release 0.3.0 has no targets command)",
                    p.display()
                );
                eprintln!(
                    "INFO [init] hint: see docs/orgmode/howto/install.org to install from source"
                );
            }
        } else {
            eprintln!(
                "INFO [init] hint: write the template with --out site.toml to print next steps"
            );
        }
    }
    Ok(exit::OK)
}

fn timestamp_id() -> String {
    chrono::Utc::now().format("%Y%m%d_%H%M%S").to_string()
}

struct Mode<'a> {
    out: Option<&'a Path>,
    result: Option<&'a Path>,
    shard_plan: Option<&'a Path>,
    dry: bool,
    is_run: bool,
    oom_node: Option<&'a str>,
    resume_result: Option<&'a Path>,
}

fn cmd_plan_or_run(c: &Common, m: Mode) -> Result<i32, Fail> {
    let Mode {
        out,
        result,
        shard_plan,
        dry,
        is_run,
        oom_node,
        resume_result,
    } = m;
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
    let list_text = std::fs::read_to_string(&c.list)
        .map_err(|e| Fail::usage(format!("cannot read list {}: {e}", c.list.display())))?;
    let lines = if easystack::is_easystack(&c.list) {
        easystack::parse(&list_text)
            .map_err(|e| Fail::usage(format!("easystack {}: {e}", c.list.display())))?
    } else {
        util::parse_list(&list_text)?
    };
    if lines.is_empty() {
        return Err(Fail::usage(format!(
            "list {} holds no lines",
            c.list.display()
        )));
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
    let mut planner = Planner::new(scope, c.list.display().to_string());
    planner.offline_bundle = c.offline_bundle.clone();
    if c.require_offline {
        planner.offline_wrapper = sources
            .expect("offline bundle checked sources")
            .offline_wrapper
            .clone();
    }
    if !is_run {
        let units = match shard_plan {
            Some(d) => Some(read_units(d)?),
            None => None,
        };
        let plan = build_plan(&planner, &lines, units.as_deref())?;
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
    let mut res = run(
        &planner,
        &lines,
        &Options {
            dry_run: dry,
            shard_units: saved_units.as_deref(),
            oom_node,
            resume: prior.as_ref(),
            resume_refused: resume_refused.as_deref(),
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
        if let Some(before) = &before {
            watch::record_run(&planner, &lines, &res, before);
        }
        write_summary(&s, &res, "run", result, None, &planner.offline_wrapper);
        if let Some(p) = result {
            write_json(&to_sorted(&res), Some(p))?;
        }
    }
    Ok(res.exit_code)
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
    if let Some(stamp) = w.log_stamp.filter(|s| !util::is_log_stamp(s)) {
        return Err(Fail::usage(format!(
            "--log-stamp `{stamp}` is not a UTC time as YYYYMMDD_HHMMSS"
        )));
    }
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
            for i in &installs {
                println!("{}", i.module);
            }
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
            for (m, i) in &other {
                println!(
                    "{m:<width$}  built against EESSI {} {}",
                    i.eessi_version, i.software_subdir
                );
            }
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
    let print = |doc: &watch::PlanDoc| {
        if w.dry_run {
            print!("{}", doc.text());
        } else {
            doc.text().lines().for_each(say);
        }
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
    if watch::unchanged_at(&state, &probe, revision) && abi_fields.is_empty() && !abi_needs_update {
        drop_work();
        let doc = watch::PlanDoc::unchanged(name, &probe, revision, &state);
        if let Some(p) = w.plan_out {
            write_json(&to_sorted(&doc), Some(p))?;
        }
        print(&doc);
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
            print(&watch::PlanDoc::new(name, &probe, revision, verdicts));
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
            print(&watch::PlanDoc::new(name, &probe, revision, verdicts));
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
    if let Some(p) = w.plan_out {
        write_json(&to_sorted(&doc), Some(p))?;
    }
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
    print(&doc);
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
    let res = run(
        &planner,
        &lines,
        &Options {
            dry_run: false,
            shard_units: None,
            oom_node: None,
            resume: None,
            resume_refused: None,
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
    write_summary(&s, &res, "watch", w.result, Some(to_sorted(&doc)), &[]);
    if let Some(p) = w.result {
        write_json(&to_sorted(&res), Some(p))?;
    }
    Ok(res.exit_code)
}

fn main() {
    let cli = Cli::parse();
    let r = match &cli.command {
        Cmd::Check {
            site,
            lists_dir,
            probe_tools,
        } => cmd_check(site, lists_dir.as_deref(), *probe_tools),
        Cmd::Schema => {
            print!("{}", schema::reference());
            Ok(exit::OK)
        }
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
            format,
        } => cmd_status(site, target.as_deref(), *last, *format),
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
                shard_plan: shard_plan.as_deref(),
                dry: *dry_run,
                is_run: true,
                oom_node: node.as_deref(),
                resume_result: resume_result.as_deref(),
            },
        ),
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
