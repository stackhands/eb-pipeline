//! Opt-in site-layer gap and shadow reports delegated to `eb-stack`.

use crate::exit::{self, Fail};
use crate::resolve::Scope;
use crate::schema::Site;
use crate::tools::Tool;
use crate::util;
use clap::Args;
use std::collections::BTreeSet;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::{Command, Output, Stdio};

/// Options that enable `eb-stack stack gap` from `eb-pipeline check`.
#[derive(Args, Debug)]
pub struct GapArgs {
    /// Write eb-stack's gap report files into this directory.
    #[arg(long, requires = "generation")]
    pub gap_out: Option<PathBuf>,
    /// Toolchain generation passed to `eb-stack stack gap`.
    #[arg(long, requires = "gap_out")]
    pub generation: Option<String>,
    /// Also write `stack shadow` JavaScript Object Notation (JSON) output to `DIR/shadow.json`.
    #[arg(long, requires = "gap_out")]
    pub shadow: bool,
}

fn push_unique(out: &mut Vec<String>, seen: &mut BTreeSet<String>, value: &str) {
    if !value.is_empty() && seen.insert(value.to_string()) {
        out.push(value.to_string());
    }
}

fn local_modules(site: &Site) -> Vec<String> {
    let mut paths = Vec::new();
    let mut seen = BTreeSet::new();
    for target in &site.targets {
        let scope = Scope {
            site,
            site_file: None,
            target,
            build_type: None,
            run_id: String::new(),
            log_stamp: String::new(),
        };
        push_unique(
            &mut paths,
            &mut seen,
            &format!("{}/all", scope.modules_root().trim_end_matches('/')),
        );
    }
    paths
}

fn base_modules(site: &Site) -> Result<Vec<String>, Fail> {
    let mut paths = Vec::new();
    let mut seen = BTreeSet::new();
    if let Some(shards) = &site.shards {
        for path in &shards.installed_modules_dirs {
            push_unique(&mut paths, &mut seen, path);
        }
        for name in &shards.installed_modules_dirs_from_env {
            let value = std::env::var_os(name).ok_or_else(|| {
                Fail::usage(format!(
                    "shards.installed_modules_dirs_from_env: {name} is unset"
                ))
            })?;
            for path in std::env::split_paths(&value) {
                push_unique(&mut paths, &mut seen, &path.as_os_str().to_string_lossy());
            }
        }
    }
    if let Some(watch) = &site.watch {
        for path in &watch.modules_dirs {
            push_unique(&mut paths, &mut seen, path);
        }
    }
    if paths.is_empty() {
        return Err(Fail::usage(
            "--gap-out needs a European Environment for Scientific Software Installations (EESSI) module tree in shards.installed_modules_dirs, \
             shards.installed_modules_dirs_from_env, or watch.modules_dirs",
        ));
    }
    Ok(paths)
}

fn easyconfigs(site: &Site) -> Result<Vec<String>, Fail> {
    let mut paths = Vec::new();
    let mut seen = BTreeSet::new();
    if let Some(shards) = &site.shards {
        for path in &shards.easyconfigs {
            push_unique(&mut paths, &mut seen, path);
        }
    }
    if let Some(watch) = &site.watch {
        for path in &watch.easyconfigs {
            push_unique(&mut paths, &mut seen, path);
        }
    }
    if paths.is_empty() {
        return Err(Fail::usage(
            "--gap-out needs a recipe tree in shards.easyconfigs or watch.easyconfigs",
        ));
    }
    Ok(paths)
}

fn pairs(argv: &mut Vec<String>, option: &str, values: &[String]) {
    for value in values {
        argv.push(option.to_string());
        argv.push(value.clone());
    }
}

/// Build the exact `stack gap` argument vector for this site.
pub fn gap_argv(site: &Site, generation: &str, out_dir: &Path) -> Result<Vec<String>, Fail> {
    let mut argv = vec!["stack".into(), "gap".into()];
    pairs(&mut argv, "--site", &local_modules(site));
    pairs(&mut argv, "--installed", &base_modules(site)?);
    pairs(&mut argv, "--easyconfigs", &easyconfigs(site)?);
    argv.extend([
        "--generation".into(),
        generation.into(),
        "--out-dir".into(),
        out_dir.display().to_string(),
    ]);
    Ok(argv)
}

/// Build the exact `stack shadow` JavaScript Object Notation (JSON) argument vector for this site.
pub fn shadow_argv(site: &Site) -> Result<Vec<String>, Fail> {
    let mut argv = vec!["stack".into(), "shadow".into()];
    pairs(&mut argv, "--easyconfigs", &easyconfigs(site)?);
    pairs(&mut argv, "--base", &base_modules(site)?);
    argv.push("--json".into());
    Ok(argv)
}

fn execute(bin: &str, argv: &[String]) -> Result<Output, Fail> {
    Command::new(bin)
        .args(argv)
        .stdin(Stdio::null())
        .output()
        .map_err(|error| Fail::usage(format!("cannot run `{bin} {}`: {error}", argv.join(" "))))
}

fn write(path: &Path, bytes: &[u8]) -> Result<(), Fail> {
    util::write_atomic(path, bytes).map_err(|error| {
        Fail::new(
            exit::FAILED,
            format!("cannot write {}: {error}", path.display()),
        )
    })
}

fn emit(bytes: &[u8], stderr: bool) -> Result<(), Fail> {
    let result = if stderr {
        std::io::stderr().write_all(bytes)
    } else {
        util::write_stdout(bytes)
    };
    result.map_err(|error| {
        Fail::new(
            exit::FAILED,
            format!("cannot print eb-stack output: {error}"),
        )
    })
}

fn failed(bin: &str, argv: &[String], output: &Output, log: &Path) -> Fail {
    use std::os::unix::process::ExitStatusExt;
    let code = output
        .status
        .code()
        .or_else(|| output.status.signal().map(|signal| 128 + signal))
        .unwrap_or(exit::FAILED);
    Fail::new(
        code,
        format!(
            "`{bin} {}` exited {code}; its stderr is in {}",
            argv.join(" "),
            log.display()
        ),
    )
}

/// Run the requested gap report, and optionally the shadow report.
pub fn run(site: &Site, args: &GapArgs) -> Result<(), Fail> {
    let Some(out_dir) = args.gap_out.as_deref() else {
        return Ok(());
    };
    let generation = args
        .generation
        .as_deref()
        .expect("clap requires --generation with --gap-out");
    std::fs::create_dir_all(out_dir).map_err(|error| {
        Fail::new(
            exit::FAILED,
            format!("cannot create {}: {error}", out_dir.display()),
        )
    })?;

    let bin = Tool::EbStack.bin(site);
    let gap = gap_argv(site, generation, out_dir)?;
    let output = execute(&bin, &gap)?;
    let gap_stdout = out_dir.join("gap.stdout.log");
    let gap_stderr = out_dir.join("gap.stderr.log");
    write(&gap_stdout, &output.stdout)?;
    write(&gap_stderr, &output.stderr)?;
    emit(&output.stdout, false)?;
    emit(&output.stderr, true)?;
    if !output.status.success() {
        return Err(failed(&bin, &gap, &output, &gap_stderr));
    }

    if args.shadow {
        let shadow = shadow_argv(site)?;
        let output = execute(&bin, &shadow)?;
        let shadow_json = out_dir.join("shadow.json");
        let shadow_stderr = out_dir.join("shadow.stderr.log");
        write(&shadow_json, &output.stdout)?;
        write(&shadow_stderr, &output.stderr)?;
        emit(&output.stderr, true)?;
        if !output.status.success() {
            return Err(failed(&bin, &shadow, &output, &shadow_stderr));
        }
    }
    Ok(())
}
