//! Opt-in delegation of build-list checks to `eb-stack`.

use crate::exit::{self, Fail};
use crate::resolve::Scope;
use crate::run::log;
use crate::schema::{Site, Target};
use crate::tools::Tool;
use clap::ValueEnum;
use std::collections::BTreeSet;
use std::os::unix::process::ExitStatusExt;
use std::path::Path;
use std::process::{Command, Stdio};

/// A check implemented by an `eb-stack stack` subcommand.
#[derive(Clone, Copy, Debug, PartialEq, Eq, ValueEnum)]
pub enum StackCheck {
    /// `stack files`, with robot trees, the easyconfig repository and source paths.
    Files,
    /// `stack arch`, with each selected target's `cpu_target` as `--arch`.
    Arch,
    /// `stack cuda-cc`, with the robot trees. `eb-stack` checks its accelerator targets.
    CudaCc,
    /// `stack generation` on the easystack.
    Generation,
    /// `stack provided` on a plain build list, with `[sbom].lock` as `--baseline-lock`.
    Provided,
    /// `stack known-failures`, with robot trees and each selected `cpu_target`.
    KnownFailures,
}

impl StackCheck {
    fn subcommand(self) -> &'static str {
        match self {
            Self::Files => "files",
            Self::Arch => "arch",
            Self::CudaCc => "cuda-cc",
            Self::Generation => "generation",
            Self::Provided => "provided",
            Self::KnownFailures => "known-failures",
        }
    }
}

fn selected_targets<'a>(site: &'a Site, target: Option<&str>) -> Result<Vec<&'a Target>, Fail> {
    match target {
        Some(name) => Ok(vec![site.target(name)?]),
        None => Ok(site.targets.iter().collect()),
    }
}

fn push_pairs(argv: &mut Vec<String>, option: &str, values: impl IntoIterator<Item = String>) {
    let mut seen = BTreeSet::new();
    for value in values {
        if seen.insert(value.clone()) {
            argv.push(option.into());
            argv.push(value);
        }
    }
}

fn easyconfig_paths(site: &Site) -> impl Iterator<Item = String> + '_ {
    site.shards
        .iter()
        .flat_map(|shards| shards.easyconfigs.iter().cloned())
}

fn source_paths(site: &Site, targets: &[&Target]) -> Vec<String> {
    targets
        .iter()
        .flat_map(|target| {
            let scope = Scope {
                site,
                site_file: None,
                target,
                build_type: None,
                run_id: String::new(),
                log_stamp: String::new(),
            };
            scope.source_paths()
        })
        .collect()
}

fn validate_list_path(list: &Path, checks: &[StackCheck]) -> Result<(), Fail> {
    let provided = checks.contains(&StackCheck::Provided);
    if provided && checks.iter().any(|check| *check != StackCheck::Provided) {
        return Err(Fail::usage(
            "--stack-checks cannot combine provided with easystack checks because they require different input formats",
        ));
    }
    if provided {
        return Ok(());
    }
    match list
        .extension()
        .and_then(|extension| extension.to_str())
        .map(str::to_ascii_lowercase)
    {
        Some(extension) if extension == "yml" || extension == "yaml" => Ok(()),
        _ => Err(Fail::usage(format!(
            "--stack-checks needs an easystack .yml or .yaml path, not {}",
            list.display()
        ))),
    }
}

/// Build the real `eb-stack` argv for one requested check.
pub fn argv(site: &Site, list: &Path, targets: &[&Target], check: StackCheck) -> Vec<String> {
    let mut argv = vec!["stack".into(), check.subcommand().into()];
    let list = list.display().to_string();
    match check {
        StackCheck::Files => {
            argv.extend(["--easystack".into(), list]);
            push_pairs(&mut argv, "--easyconfigs", easyconfig_paths(site));
            if let Some(repo) = &site.filesystems.configs_repo {
                argv.extend(["--easyconfigs-repo".into(), repo.clone()]);
            }
            push_pairs(&mut argv, "--sourcepath", source_paths(site, targets));
        }
        StackCheck::Arch => {
            push_pairs(
                &mut argv,
                "--arch",
                targets.iter().filter_map(|t| t.cpu_target.clone()),
            );
            argv.extend(["--easystack".into(), list]);
            push_pairs(&mut argv, "--easyconfigs", easyconfig_paths(site));
        }
        StackCheck::CudaCc => {
            argv.extend(["--easystack".into(), list]);
            push_pairs(&mut argv, "--easyconfigs", easyconfig_paths(site));
        }
        StackCheck::Generation => {
            argv.extend(["--easystack".into(), list]);
        }
        StackCheck::Provided => {
            argv.extend(["--build-list".into(), list]);
            if let Some(lock) = site.sbom.as_ref().and_then(|sbom| sbom.lock.as_ref()) {
                argv.extend(["--baseline-lock".into(), lock.clone()]);
            }
            argv.push("--strict".into());
        }
        StackCheck::KnownFailures => {
            push_pairs(
                &mut argv,
                "--arch",
                targets.iter().filter_map(|t| t.cpu_target.clone()),
            );
            argv.extend(["--easystack".into(), list]);
            push_pairs(&mut argv, "--easyconfigs", easyconfig_paths(site));
            argv.push("--strict".into());
        }
    }
    argv
}

/// Run requested checks directly, without a scheduler launch.
pub fn run(
    site: &Site,
    list: &Path,
    target: Option<&str>,
    checks: &[StackCheck],
) -> Result<(), Fail> {
    validate_list_path(list, checks)?;
    let targets = selected_targets(site, target)?;
    let bin = Tool::EbStack.bin(site);
    let label = target.unwrap_or("-");
    let mut failed = Vec::new();
    let mut codes = Vec::new();
    for check in checks {
        let name = check.subcommand();
        log("INFO", "stack-check", label, &format!("running {name}"));
        let status = Command::new(&bin)
            .args(argv(site, list, &targets, *check))
            .stdin(Stdio::null())
            .status();
        match status {
            Ok(status) if status.success() => {
                log("INFO", "stack-check", label, &format!("{name} passed"));
            }
            Ok(status) => {
                let code = status
                    .code()
                    .or_else(|| status.signal().map(|signal| 128 + signal))
                    .unwrap_or(exit::FAILED);
                log(
                    "ERROR",
                    "stack-check",
                    label,
                    &format!("{name} failed with status {code}"),
                );
                failed.push(name);
                codes.push(code);
            }
            Err(err) => {
                log(
                    "ERROR",
                    "stack-check",
                    label,
                    &format!("cannot start {bin} for {name}: {err}"),
                );
                failed.push(name);
                codes.push(exit::FAILED);
            }
        }
    }
    if failed.is_empty() {
        Ok(())
    } else {
        Err(Fail::new(
            crate::summary::combined_code(&codes),
            format!("eb-stack checks failed: {}", failed.join(", ")),
        ))
    }
}
