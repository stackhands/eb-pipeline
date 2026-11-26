//! What a `local` site needs on the host that runs it.
//!
//! A `local` scheduler runs every phase on the host that runs eb-pipeline,
//! so `check --probe-tools` can look at that host before a build fails on
//! it. EasyBuild needs `eb` and a module tool; the module tool is the one
//! EASYBUILD_MODULES_TOOL names, Lmod when it is unset, and EasyBuild finds
//! it through the variable or the program below.

use crate::schema::Site;

/// Module tools EasyBuild knows, with the variable that holds the
/// command's path and the program looked up on PATH without it.
const MODULE_TOOLS: [(&str, &str, &str); 3] = [
    ("Lmod", "LMOD_CMD", "lmod"),
    ("EnvironmentModules", "MODULES_CMD", "modulecmd.tcl"),
    ("EnvironmentModulesC", "", "modulecmd"),
];

/// Problems a `local` site would hit on this host, one sentence each.
/// Empty for another scheduler kind, and when the site's module wrapper
/// (`modules.shell_init` or `modules.init`) sets up the environment, since
/// what that provides is only known after it runs.
pub fn local_problems(
    site: &Site,
    env: &dyn Fn(&str) -> Option<String>,
    on_path: &dyn Fn(&str) -> bool,
) -> Vec<String> {
    let m = &site.modules;
    if site.scheduler.kind != "local" || m.shell_init.is_some() || !m.init.is_empty() {
        return Vec::new();
    }
    let mut out = Vec::new();
    if !on_path("eb") {
        out.push(
            "`eb` is not on PATH, and the local scheduler runs it on this host; install EasyBuild (`pip install easybuild`) or load it through modules.init".to_string(),
        );
    }
    let set = env("EASYBUILD_MODULES_TOOL").filter(|v| !v.is_empty());
    let name = set.as_deref().unwrap_or("Lmod");
    if let Some((tool, var, prog)) = MODULE_TOOLS.iter().find(|(t, _, _)| *t == name) {
        let by_var = !var.is_empty() && env(var).is_some_and(|v| !v.is_empty());
        if !by_var && !on_path(prog) {
            let how = if var.is_empty() {
                format!("`{prog}` on PATH")
            } else {
                format!("{var} or `{prog}` on PATH")
            };
            let why = if set.is_some() {
                "EASYBUILD_MODULES_TOOL names"
            } else {
                "EASYBUILD_MODULES_TOOL is unset, so EasyBuild uses"
            };
            out.push(format!(
                "{why} {tool} and this host has no {how}; set modules.shell_init to the module tool's init (EESSI's init/lmod/bash provides Lmod), or install one and set EASYBUILD_MODULES_TOOL"
            ));
        }
    }
    out
}

/// The problem `eb --version` shows on this host for a `local` site, if
/// any. `eb` is a launcher that picks the first Python on PATH it accepts,
/// so an `eb` that is on PATH can still fail to start, as when an init
/// script puts a Python without EasyBuild ahead of the one that has it.
/// None for another scheduler kind, with a module wrapper, or with no `eb`
/// on PATH, which [`local_problems`] reports.
pub fn eb_problem(site: &Site) -> Option<String> {
    let m = &site.modules;
    if site.scheduler.kind != "local"
        || m.shell_init.is_some()
        || !m.init.is_empty()
        || !on_path("eb")
    {
        return None;
    }
    let out = std::process::Command::new("eb")
        .arg("--version")
        .stdin(std::process::Stdio::null())
        .output();
    let text = match out {
        Ok(o) if o.status.success() => return None,
        Ok(o) => {
            let all = format!(
                "{}{}",
                String::from_utf8_lossy(&o.stdout),
                String::from_utf8_lossy(&o.stderr)
            );
            all.lines()
                .find(|l| !l.trim().is_empty())
                .unwrap_or("no output")
                .trim()
                .to_string()
        }
        Err(e) => e.to_string(),
    };
    Some(eb_hint(&text))
}

/// The WARN text for an `eb --version` that failed with `first`.
fn eb_hint(first: &str) -> String {
    let hint = if first.contains("No compatible 'python'") {
        "; set EB_PYTHON to the Python that has EasyBuild installed, or put its directory first on PATH"
    } else {
        ""
    };
    format!("`eb --version` fails on this host: {first}{hint}")
}

/// Whether `prog` names an executable file in a PATH entry.
pub fn on_path(prog: &str) -> bool {
    use std::os::unix::fs::PermissionsExt;
    let Some(path) = std::env::var_os("PATH") else {
        return false;
    };
    std::env::split_paths(&path).any(|d| {
        std::fs::metadata(d.join(prog))
            .is_ok_and(|m| m.is_file() && m.permissions().mode() & 0o111 != 0)
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn site(kind: &str, init: &[&str], shell_init: Option<&str>) -> Site {
        let modules = match shell_init {
            Some(f) => format!("init = {init:?}\nshell_init = \"{f}\""),
            None => format!("init = {init:?}"),
        };
        let text = format!(
            "schema_version = 1\nkind = \"site\"\n[site]\nname = \"w\"\n[scheduler]\nkind = \"{kind}\"\n\
             [[targets]]\nname = \"cpu\"\nbuild_lists = [\"software.list\"]\ndefault = true\n\
             [filesystems]\nscratch = \"/srv/s\"\ntmp = \"/srv/s/tmp\"\nbuild_path = \"/srv/s/b\"\n\
             install_root = \"/srv/sw\"\nlog_root = \"/srv/logs\"\n[modules]\n{modules}\n"
        );
        toml::from_str(&text).expect("test site parses")
    }

    fn env<'a>(pairs: &'a [(&'a str, &'a str)]) -> impl Fn(&str) -> Option<String> + 'a {
        move |k| {
            pairs
                .iter()
                .find(|(n, _)| *n == k)
                .map(|(_, v)| v.to_string())
        }
    }

    #[test]
    fn bare_host_names_eb_and_lmod() {
        let p = local_problems(&site("local", &[], None), &env(&[]), &|_| false);
        assert_eq!(p.len(), 2, "{p:?}");
        assert!(p[0].contains("`eb` is not on PATH"), "{p:?}");
        assert!(p[1].contains("EASYBUILD_MODULES_TOOL is unset"), "{p:?}");
        assert!(p[1].contains("LMOD_CMD or `lmod` on PATH"), "{p:?}");
    }

    #[test]
    fn lmod_cmd_or_program_satisfies_lmod() {
        let s = site("local", &[], None);
        assert!(local_problems(&s, &env(&[("LMOD_CMD", "/x/lmod")]), &|p| p == "eb").is_empty());
        assert!(local_problems(&s, &env(&[]), &|p| p == "eb" || p == "lmod").is_empty());
    }

    #[test]
    fn named_tool_is_the_one_checked() {
        let s = site("local", &[], None);
        let e = [("EASYBUILD_MODULES_TOOL", "EnvironmentModulesC")];
        let p = local_problems(&s, &env(&e), &|p| p == "eb" || p == "lmod");
        assert_eq!(p.len(), 1, "{p:?}");
        assert!(p[0].contains("names EnvironmentModulesC"), "{p:?}");
        assert!(local_problems(&s, &env(&e), &|p| p == "eb" || p == "modulecmd").is_empty());
    }

    #[test]
    fn unknown_tool_is_left_to_easybuild() {
        let s = site("local", &[], None);
        let e = [("EASYBUILD_MODULES_TOOL", "SomethingNew")];
        assert!(local_problems(&s, &env(&e), &|p| p == "eb").is_empty());
    }

    #[test]
    fn python_launcher_failure_names_eb_python() {
        let w = eb_hint("ERROR: No compatible 'python' command found via $PATH (EasyBuild requires Python 3.6+)");
        assert!(
            w.starts_with("`eb --version` fails on this host: ERROR"),
            "{w}"
        );
        assert!(w.contains("EB_PYTHON"), "{w}");
        assert!(!eb_hint("Traceback").contains("EB_PYTHON"));
    }

    #[test]
    fn wrapper_or_slurm_skips_the_host() {
        let none = |_: &str| false;
        assert!(local_problems(&site("slurm", &[], None), &env(&[]), &none).is_empty());
        assert!(local_problems(&site("local", &["EasyBuild"], None), &env(&[]), &none).is_empty());
        let s = site(
            "local",
            &[],
            Some("/cvmfs/software.eessi.io/versions/2023.06/init/lmod/bash"),
        );
        assert!(local_problems(&s, &env(&[]), &none).is_empty());
    }
}
