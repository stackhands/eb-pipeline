//! `eb-pipeline audit`: ask eb-stack for vulnerability rebuild lists.
//!
//! The command shells out to eb-stack for every policy decision. It only
//! stages module metadata, passes through the requested inputs, and copies
//! eb-stack's generated rebuild lists.

use crate::eessi::{self, Probe};
use crate::exit::{self, Fail};
use crate::plan::Planner;
use crate::resolve::Scope;
use crate::run::{exec_logged, log};
use crate::schema::{Site, Target};
use crate::tools::Tool;
use crate::watch;
use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};

const PHASE: &str = "audit";

/// Inputs for one opt-in audit run.
pub struct Args<'a> {
    /// Validated site configuration.
    pub site: &'a Site,
    /// Absolute site-file path passed to subprocess environments.
    pub site_file: Option<String>,
    /// Target whose module environment is audited.
    pub target: &'a Target,
    /// Optional vulnerability-scanner findings file.
    pub findings: Option<&'a Path>,
    /// CVE identifiers to inspect when no findings file is supplied.
    pub cves: &'a [String],
    /// Whether reported backport items make the command fail.
    pub strict: bool,
    /// Directory for logs, generated data, and copied rebuild lists.
    pub out_dir: &'a Path,
}

/// Run the requested `eb-stack` audit subprocesses and collect rebuild lists.
pub fn run(args: Args<'_>) -> Result<i32, Fail> {
    let scope = Scope {
        site: args.site,
        site_file: args.site_file,
        target: args.target,
        build_type: None,
        run_id: "audit".into(),
        log_stamp: "audit".into(),
    };
    let planner = Planner::new(scope, "<audit>".into());
    std::fs::create_dir_all(args.out_dir).map_err(|e| {
        Fail::new(
            exit::FAILED,
            format!("cannot create {}: {e}", args.out_dir.display()),
        )
    })?;
    watch::need_eb_stack(args.site)?;
    let work = args.out_dir.join("work");
    let _ = std::fs::remove_dir_all(&work);
    std::fs::create_dir_all(&work).map_err(|e| {
        Fail::new(
            exit::FAILED,
            format!("cannot create {}: {e}", work.display()),
        )
    })?;

    let (probe, env) = probe_target_env(&planner, &work)?;
    watch::check_dirs(&probe)?;
    let eessi = eessi::scan(&probe.modules_dirs);
    let local = eessi::scan(&[watch::local_modules_dir(&planner)]);
    let universe = watch::stage(&work, &eessi, &local)?;
    let easyconfigs = easyconfig_trees(args.site, &env, &universe);
    let mut strict_backports = 0usize;

    if let Some(findings) = args.findings {
        let out = work.join("rebuild-set");
        let mut argv = vec![
            "rebuild-set".into(),
            "--findings".into(),
            findings.display().to_string(),
        ];
        push_easyconfigs(&mut argv, &easyconfigs);
        argv.extend([
            "--installed".into(),
            universe.installed.display().to_string(),
            "--installed-label".into(),
            probe.software_subdir.clone(),
            "--out-dir".into(),
            out.display().to_string(),
        ]);
        let summary = run_eb_stack(&planner, &argv, &work.join("rebuild-set.log"))?;
        strict_backports += parse_metric(&summary, "backport").unwrap_or(0);
        copy_rebuilds(&out.join("rebuilds"), &args.out_dir.join("rebuilds"))?;
    }

    if !args.cves.is_empty() {
        let lock = args
            .site
            .sbom_on()
            .and_then(|sb| sb.lock.as_deref())
            .filter(|path| !path.is_empty())
            .ok_or_else(|| {
                Fail::usage(
                    "--cve needs sbom.lock in the site file so `eb-stack stack sbom` can read the lock",
                )
            })?;
        let backport = work.join("backport.md");
        let mut argv = vec![
            "stack".into(),
            "sbom".into(),
            "--lock".into(),
            lock.to_string(),
            "--backport-list".into(),
            backport.display().to_string(),
            "--out".into(),
            work.join("stack.cdx.json").display().to_string(),
        ];
        for cve in args.cves {
            argv.push("--cve".into());
            argv.push(cve.clone());
        }
        let summary = run_eb_stack(&planner, &argv, &work.join("stack-sbom.log"))?;
        strict_backports += parse_metric(&summary, "backport").unwrap_or(0);

        for cve in args.cves {
            let tag = cve.replace(['/', '\\', ' '], "_");
            let out = work.join(format!("component-reach-{tag}"));
            let mut argv = vec![
                "component-reach".into(),
                "--cve".into(),
                cve.clone(),
                "--generation".into(),
                probe.version.clone(),
            ];
            push_easyconfigs(&mut argv, &easyconfigs);
            argv.extend([
                "--installed".into(),
                universe.installed.display().to_string(),
                "--out-dir".into(),
                out.display().to_string(),
            ]);
            let _ = run_eb_stack(
                &planner,
                &argv,
                &work.join(format!("component-reach-{tag}.log")),
            )?;
        }
    }

    if args.strict && strict_backports > 0 {
        return Err(Fail::new(
            exit::FAILED,
            format!(
                "strict mode: eb-stack reported {strict_backports} backport item(s); see {}",
                work.display()
            ),
        ));
    }
    log(
        "INFO",
        PHASE,
        &args.target.name,
        &format!("wrote audit artifacts under {}", args.out_dir.display()),
    );
    Ok(exit::OK)
}

fn probe_target_env(
    planner: &Planner<'_>,
    work: &Path,
) -> Result<(Probe, BTreeMap<String, String>), Fail> {
    let out = work.join("target.env");
    let inner = ["sh", "-c", "env -0 > \"$1\"", "sh"]
        .into_iter()
        .map(String::from)
        .chain([out.display().to_string()])
        .collect();
    let cmd = planner.plan_role_cmd(PHASE, inner)?;
    let log_path = work.join("probe.log");
    let code = exec_logged(&cmd, &planner.scope.env(), &log_path)?;
    let bytes = std::fs::read(&out).unwrap_or_default();
    let _ = std::fs::remove_file(&out);
    if code != 0 || bytes.is_empty() {
        return Err(Fail::new(
            exit::TEMPFAIL,
            format!(
                "the module environment of target {} did not load (exit {code}); its output is in {}",
                planner.scope.target.name,
                log_path.display()
            ),
        ));
    }
    let env = eessi::parse_env0(&bytes);
    let watch = planner.scope.site.watch_on();
    let probe = Probe::from_env(
        &env,
        watch.and_then(|w| w.repository.as_deref()),
        watch.map_or(&[] as &[String], |w| w.modules_dirs.as_slice()),
    )
    .map_err(|why| {
        Fail::new(
            exit::REFUSED,
            format!(
                "no EESSI in the module environment of target {}: {why}",
                planner.scope.target.name
            ),
        )
    })?;
    Ok((probe, env))
}

fn easyconfig_trees(
    site: &Site,
    env: &BTreeMap<String, String>,
    u: &watch::Universe,
) -> Vec<PathBuf> {
    let mut out: Vec<PathBuf> = site
        .watch_on()
        .map(|w| watch::robot_trees(w, env))
        .unwrap_or_default()
        .into_iter()
        .map(PathBuf::from)
        .collect();
    out.push(u.eessi_dir.clone());
    out.push(u.local_dir.clone());
    let mut seen = BTreeSet::new();
    out.retain(|path| seen.insert(path.display().to_string()));
    out
}

fn push_easyconfigs(argv: &mut Vec<String>, trees: &[PathBuf]) {
    for tree in trees {
        argv.push("--easyconfigs".into());
        argv.push(tree.display().to_string());
    }
}

fn run_eb_stack(planner: &Planner<'_>, tail: &[String], log_path: &Path) -> Result<String, Fail> {
    let mut argv = vec![Tool::EbStack.bin(planner.scope.site)];
    argv.extend(tail.iter().cloned());
    let cmd = planner.plan_role_cmd(PHASE, argv)?;
    let code = exec_logged(&cmd, &planner.scope.env(), log_path)?;
    if code != 0 {
        return Err(Fail::new(
            exit::FAILED,
            format!(
                "eb-stack {} exited {code}; its output is in {}",
                tail.join(" "),
                log_path.display()
            ),
        ));
    }
    std::fs::read_to_string(log_path).map_err(|e| {
        Fail::new(
            exit::FAILED,
            format!("cannot read {}: {e}", log_path.display()),
        )
    })
}

fn parse_metric(text: &str, key: &str) -> Option<usize> {
    let needle = format!("{key}=");
    text.lines().rev().find_map(|line| {
        line.split_whitespace().find_map(|word| {
            let value = word.strip_prefix(&needle)?;
            value.parse::<usize>().ok()
        })
    })
}

fn copy_rebuilds(src: &Path, dst: &Path) -> Result<(), Fail> {
    if !src.is_dir() {
        return Ok(());
    }
    std::fs::create_dir_all(dst).map_err(|e| {
        Fail::new(
            exit::FAILED,
            format!("cannot create {}: {e}", dst.display()),
        )
    })?;
    for entry in std::fs::read_dir(src)
        .map_err(|e| Fail::new(exit::FAILED, format!("cannot read {}: {e}", src.display())))?
    {
        let entry = entry
            .map_err(|e| Fail::new(exit::FAILED, format!("cannot read {}: {e}", src.display())))?;
        let path = entry.path();
        if path.extension().is_some_and(|ext| ext == "yml") {
            let to = dst.join(entry.file_name());
            std::fs::copy(&path, &to).map_err(|e| {
                Fail::new(
                    exit::FAILED,
                    format!("cannot copy {} to {}: {e}", path.display(), to.display()),
                )
            })?;
        }
    }
    Ok(())
}
