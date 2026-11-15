use clap::{Args, Parser, Subcommand, ValueEnum};
use eb_pipeline::exit::{self, Fail};
use eb_pipeline::plan::{build_plan, Planner};
use eb_pipeline::resolve::{explain, Scope};
use eb_pipeline::run::{install_handlers, log, run, Options};
use eb_pipeline::{schema, site, tools, util};
use serde::Serialize;
use std::path::{Path, PathBuf};

const ROOT_HELP: &str = "\
Examples:
  eb-pipeline check --site site.toml
  eb-pipeline plan --site site.toml --list list.txt --target cpu --build-type production --out plan.json
  eb-pipeline run --site site.toml --list list.txt --target cpu --build-type production --result result.json

Output streams: stdout carries plan, explain and schema output only. Events go to
stderr as `LEVEL [phase] target: message`. The result JSON goes to --result.

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
        #[arg(long)]
        site: PathBuf,
        /// Directory that must hold every build list a target names.
        #[arg(long)]
        lists_dir: Option<PathBuf>,
        /// Run each needed tool's `--version` and enforce min_version.
        #[arg(long)]
        probe_tools: bool,
    },
    /// Print the site file reference: every key, type, default and meaning.
    Schema,
    /// Print the resolved configuration of one or all targets.
    Explain {
        /// Site file.
        #[arg(long)]
        site: PathBuf,
        /// Target name; all targets when absent.
        #[arg(long)]
        target: Option<String>,
        /// Apply this build type's overrides.
        #[arg(long)]
        build_type: Option<String>,
        /// Output format.
        #[arg(long, value_enum, default_value_t = Format::Toml)]
        format: Format,
    },
    /// Compute phases, launch lines, environments and tool calls without executing.
    Plan {
        #[command(flatten)]
        common: Common,
        /// Write the plan JSON here; `-` or absent prints it on stdout.
        #[arg(long)]
        out: Option<PathBuf>,
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
    #[arg(long)]
    site: PathBuf,
    /// Build list file: one `Name-version-toolchain.eb [eb options]` per line.
    #[arg(long)]
    list: PathBuf,
    /// Target name.
    #[arg(long)]
    target: String,
    /// Build type name (selects per-build-type overrides).
    #[arg(long)]
    build_type: String,
    /// Run identifier; names the log directory. Default: a UTC timestamp for
    /// `run`, `<run-id>` for `plan`.
    #[arg(long)]
    run_id: Option<String>,
}

#[derive(Clone, Copy, ValueEnum)]
enum Format {
    Toml,
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
        Err(Fail::usage(format!("{} problem(s) in the site file", errs.len())))
    }
}

fn cmd_check(site_path: &Path, lists_dir: Option<&Path>, probe: bool) -> Result<i32, Fail> {
    let s = load_valid(site_path, lists_dir)?;
    if let Some(a) = s.admission_on() {
        if a.required && (a.quota_cmd.is_none() || a.reservations.is_none()) {
            log(
                "WARN",
                "check",
                &s.site.name,
                "admission.required is set without quota_cmd and reservations; `run` will refuse with exit 78",
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
    let v = explain(&s, target, build_type)?;
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

fn timestamp_id() -> String {
    chrono::Utc::now().format("%Y%m%d_%H%M%S").to_string()
}

fn cmd_plan_or_run(c: &Common, out: Option<&Path>, result: Option<&Path>, dry: bool, is_run: bool) -> Result<i32, Fail> {
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
        let code = if is_run && licence_only { exit::REFUSED } else { exit::USAGE };
        return Err(Fail::new(code, format!("{} problem(s) in the site file", errs.len())));
    }
    let target = s.target(&c.target)?;
    let list_text = std::fs::read_to_string(&c.list)
        .map_err(|e| Fail::usage(format!("cannot read list {}: {e}", c.list.display())))?;
    let lines = util::parse_list(&list_text)?;
    if lines.is_empty() {
        return Err(Fail::usage(format!("list {} holds no lines", c.list.display())));
    }
    let run_id = c.run_id.clone().unwrap_or_else(|| {
        if is_run {
            timestamp_id()
        } else {
            "<run-id>".into()
        }
    });
    let scope = Scope {
        site: &s,
        target,
        build_type: Some(&c.build_type),
        run_id,
    };
    let planner = Planner::new(scope, c.list.display().to_string());
    if !is_run {
        let plan = build_plan(&planner, &lines)?;
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
        Cmd::Check { site, lists_dir, probe_tools } => cmd_check(site, lists_dir.as_deref(), *probe_tools),
        Cmd::Schema => {
            print!("{}", schema::reference());
            Ok(exit::OK)
        }
        Cmd::Explain { site, target, build_type, format } => {
            cmd_explain(site, target.as_deref(), build_type.as_deref(), *format)
        }
        Cmd::Plan { common, out } => cmd_plan_or_run(common, out.as_deref(), None, false, false),
        Cmd::Run { common, result, dry_run } => {
            cmd_plan_or_run(common, None, result.as_deref(), *dry_run, true)
        }
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
