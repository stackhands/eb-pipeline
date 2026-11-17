use clap::{Args, Parser, Subcommand, ValueEnum};
use eb_pipeline::exit::{self, Fail};
use eb_pipeline::plan::{build_plan, read_units, Planner};
use eb_pipeline::resolve::{explain, Scope};
use eb_pipeline::run::{install_handlers, log, run, Options};
use eb_pipeline::{schema, site, templates, tools, util};
use serde::Serialize;
use std::path::{Path, PathBuf};

const ROOT_HELP: &str = "\
Examples:
  eb-pipeline init --template small-cluster --out site.toml
  eb-pipeline check --site site.toml
  eb-pipeline targets --site site.toml --format json
  eb-pipeline plan --site site.toml --list list.txt --target cpu --build-type production --out plan.json
  eb-pipeline run --site site.toml --list list.txt --target cpu --build-type production --result result.json

Configuration: every flag above falls back to an EB_PIPELINE_<FLAG> variable
(EB_PIPELINE_SITE, EB_PIPELINE_LIST, EB_PIPELINE_TARGET, EB_PIPELINE_BUILD_TYPE,
EB_PIPELINE_RUN_ID, EB_PIPELINE_LOG_STAMP, EB_PIPELINE_LISTS_DIR); a flag on the
command line wins. EB_PIPELINE__SECTION__KEY=value sets one site file value, for
example EB_PIPELINE__FILESYSTEMS__SCRATCH=/scratch/eb; the result is validated
like the file.

Output streams: stdout carries plan, explain, schema, init, targets and run --dry-run
output only. Events go to stderr as `LEVEL [phase] target: message`. The result JSON
goes to --result.

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
        #[arg(long, value_enum, default_value_t = TargetsFormat::Text)]
        format: TargetsFormat,
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
        /// Print the commands that would run and start nothing.
        #[arg(long)]
        dry_run: bool,
    },
}

#[derive(Args)]
struct Common {
    /// Site file.
    #[arg(long, env = "EB_PIPELINE_SITE")]
    site: PathBuf,
    /// Build list file: one `Name-version-toolchain.eb [eb options]` per line.
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
}

#[derive(Clone, Copy, ValueEnum)]
enum Format {
    Toml,
    Json,
}

#[derive(Clone, Copy, ValueEnum)]
enum TargetsFormat {
    Text,
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

fn cmd_targets(site_path: &Path, format: TargetsFormat) -> Result<i32, Fail> {
    let s = load_valid(site_path, None)?;
    let names: Vec<&str> = s.targets.iter().map(|t| t.name.as_str()).collect();
    match format {
        TargetsFormat::Text => {
            for n in &names {
                println!("{n}");
            }
        }
        // Compact, unlike write_json: GITHUB_OUTPUT and the like take one `name=value` line.
        TargetsFormat::Json => {
            println!("{}", serde_json::to_string(&names).expect("serializable"))
        }
    }
    Ok(exit::OK)
}

fn cmd_init(template: Option<&str>, list: bool, out: Option<&Path>) -> Result<i32, Fail> {
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
    match out {
        None => print!("{}", t.text),
        Some(p) if p == Path::new("-") => print!("{}", t.text),
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
}

fn cmd_plan_or_run(c: &Common, m: Mode) -> Result<i32, Fail> {
    let Mode {
        out,
        result,
        shard_plan,
        dry,
        is_run,
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
    let list_text = std::fs::read_to_string(&c.list)
        .map_err(|e| Fail::usage(format!("cannot read list {}: {e}", c.list.display())))?;
    let lines = util::parse_list(&list_text)?;
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
    let planner = Planner::new(scope, c.list.display().to_string());
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
    }
    install_handlers();
    let res = run(&planner, &lines, &Options { dry_run: dry });
    if let Some(m) = &res.message {
        log("ERROR", "run", &c.target, m);
    }
    if !dry {
        if let Some(p) = result {
            write_json(&to_sorted(&res), Some(p))?;
        }
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
        } => cmd_init(template.as_deref(), *list, out.as_deref()),
        Cmd::Explain {
            site,
            target,
            build_type,
            format,
        } => cmd_explain(site, target.as_deref(), build_type.as_deref(), *format),
        Cmd::Targets { site, format } => cmd_targets(site, *format),
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
            },
        ),
        Cmd::Run {
            common,
            result,
            dry_run,
        } => cmd_plan_or_run(
            common,
            Mode {
                out: None,
                result: result.as_deref(),
                shard_plan: None,
                dry: *dry_run,
                is_run: true,
            },
        ),
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
