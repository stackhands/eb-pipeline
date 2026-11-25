//! `targets`: the site file's target names, for a CI matrix or a shell loop.
#![allow(clippy::disallowed_methods)]

use std::path::{Path, PathBuf};
use std::process::{Command, Output};

fn bin() -> Command {
    let mut c = Command::new(env!("CARGO_BIN_EXE_eb-pipeline"));
    // Only what a test sets reaches the binary, whatever the caller exported.
    c.env_clear();
    c
}

fn root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
}

fn template(name: &str) -> PathBuf {
    root().join("templates").join(format!("{name}.toml"))
}

fn text(o: &Output) -> String {
    format!(
        "{}{}",
        String::from_utf8_lossy(&o.stdout),
        String::from_utf8_lossy(&o.stderr)
    )
}

/// The `[[targets]]` names of a site file, read without eb-pipeline.
fn names_in(path: &Path) -> Vec<String> {
    let v: toml::Value = toml::from_str(&std::fs::read_to_string(path).unwrap()).unwrap();
    v["targets"]
        .as_array()
        .unwrap()
        .iter()
        .map(|t| t["name"].as_str().unwrap().to_owned())
        .collect()
}

#[test]
fn targets_prints_one_name_per_line_in_file_order() {
    for t in eb_pipeline::templates::TEMPLATES {
        let path = template(t.name);
        let o = bin()
            .args(["targets", "--site"])
            .arg(&path)
            .output()
            .unwrap();
        assert_eq!(o.status.code(), Some(0), "{}: {}", t.name, text(&o));
        let want: String = names_in(&path).iter().map(|n| format!("{n}\n")).collect();
        assert_eq!(String::from_utf8_lossy(&o.stdout), want, "{}", t.name);
        assert!(o.stderr.is_empty(), "{}", text(&o));
    }
}

#[test]
fn targets_json_is_one_array_on_one_line() {
    let path = template("eessi-site");
    let o = bin()
        .args(["targets", "--format", "json", "--site"])
        .arg(&path)
        .output()
        .unwrap();
    assert_eq!(o.status.code(), Some(0), "{}", text(&o));
    let out = String::from_utf8(o.stdout).unwrap();
    // A CI output takes `name=value` on one line, and a matrix reads the array.
    assert_eq!(out.lines().count(), 1, "{out:?}");
    let names: Vec<String> = serde_json::from_str(&out).unwrap();
    assert_eq!(names, names_in(&path));
    assert!(names.len() > 1, "{names:?}");
}

#[test]
fn an_invalid_site_file_exits_2_and_prints_no_names() {
    let dir = tempfile::tempdir().unwrap();
    let site = dir.path().join("site.toml");
    let valid = std::fs::read_to_string(template("small-cluster")).unwrap();
    std::fs::write(&site, format!("{valid}\n[[targets]]\nname = \"cpu\"\n")).unwrap();
    for format in ["text", "json"] {
        let o = bin()
            .args(["targets", "--format", format, "--site"])
            .arg(&site)
            .output()
            .unwrap();
        assert_eq!(o.status.code(), Some(2), "{}", text(&o));
        assert!(o.stdout.is_empty(), "{}", text(&o));
        assert!(
            String::from_utf8_lossy(&o.stderr).contains("targets: duplicate name `cpu`"),
            "{}",
            text(&o)
        );
    }
}

#[test]
fn targets_reads_the_site_file_from_the_environment() {
    let path = template("eessi-site");
    let o = bin()
        .arg("targets")
        .env("EB_PIPELINE_SITE", &path)
        .output()
        .unwrap();
    assert_eq!(o.status.code(), Some(0), "{}", text(&o));
    let want: String = names_in(&path).iter().map(|n| format!("{n}\n")).collect();
    assert_eq!(String::from_utf8_lossy(&o.stdout), want);
}

/// The command reference shows `--help` as the binary prints it.
#[test]
fn the_cli_reference_lists_targets_among_the_commands_and_streams() {
    let path = root().join("docs/orgmode/reference/cli.org");
    let page = std::fs::read_to_string(&path).unwrap();
    assert!(
        page.lines().any(|l| l.starts_with("| =targets= |")),
        "{} has no `targets` row in its table of commands",
        path.display()
    );
    let help: String = page
        .lines()
        .skip_while(|l| *l != "=eb-pipeline --help= prints:")
        .skip_while(|l| *l != "#+begin_example")
        .skip(1)
        .take_while(|l| *l != "#+end_example")
        .map(|l| format!("{l}\n"))
        .collect();
    let o = bin().arg("--help").output().unwrap();
    assert_eq!(o.status.code(), Some(0), "{}", text(&o));
    assert_eq!(
        help,
        String::from_utf8_lossy(&o.stdout),
        "{}: the `eb-pipeline --help` block",
        path.display()
    );
    let streams = help
        .lines()
        .find(|l| l.starts_with("Output streams: stdout carries "))
        .unwrap_or_default();
    assert!(streams.contains(" targets "), "{streams:?}");
}

#[test]
fn watch_help_describes_exit_one_completely() {
    let o = bin().args(["watch", "--help"]).output().unwrap();
    assert_eq!(o.status.code(), Some(0), "{}", text(&o));
    let out = String::from_utf8_lossy(&o.stdout);
    assert!(
        out.contains("plan or rebuild list could not be read or written"),
        "{out}"
    );
    assert!(!out.contains("state file did\n"), "{out}");
}

#[test]
fn the_ci_example_runs_one_job_per_target() {
    let path = root().join("examples/ci/github-actions.yml");
    let wf = std::fs::read_to_string(&path).unwrap();
    for needed in [
        "names=$(eb-pipeline targets --format json)",
        "target: ${{ fromJSON(needs.targets.outputs.names) }}",
        "fail-fast: false",
        "EB_PIPELINE_TARGET: ${{ matrix.target }}",
        "EB_PIPELINE_RUN_ID: ${{ github.run_id }}-${{ github.run_attempt }}-${{ matrix.target }}",
        "eb-pipeline run --result",
    ] {
        assert!(wf.contains(needed), "{} lacks `{needed}`", path.display());
    }
    // EB_PIPELINE_LINT_DENY adds an operator's own site names to refuse.
    let denied: Vec<String> = eb_pipeline::lint::deny_from_env()
        .into_iter()
        .filter(|d| wf.contains(d.as_str()))
        .collect();
    assert!(denied.is_empty(), "{}: {denied:?}", path.display());
}

/// Without `{target}` in the build path every target gets one
/// EASYBUILD_BUILDPATH, and EasyBuild removes the build directory of an
/// easyconfig when a build of it starts.
#[test]
fn the_ci_example_builds_one_target_at_a_time() {
    let path = root().join("examples/ci/github-actions.yml");
    let wf = std::fs::read_to_string(&path).unwrap();
    assert!(
        wf.lines().any(|l| l.trim() == "max-parallel: 1"),
        "{} lacks `max-parallel: 1`",
        path.display()
    );
    let comment: String = wf
        .lines()
        .map(str::trim)
        .filter_map(|l| l.strip_prefix("# "))
        .collect::<Vec<_>>()
        .join(" ");
    for needed in [
        "Raise max-parallel only when build_path has {target}",
        "its own first source path and, with staged rebuilds, its own state root",
    ] {
        assert!(
            comment.contains(needed),
            "{} lacks `{needed}`",
            path.display()
        );
    }
}

/// `max-parallel` holds within one run of the workflow; two dispatches are
/// two runs. The block has no `queue` key: actionlint 1.7.12 refuses it.
#[test]
fn the_ci_example_builds_a_target_for_one_dispatch_at_a_time() {
    let path = root().join("examples/ci/github-actions.yml");
    let wf = std::fs::read_to_string(&path).unwrap();
    assert!(
        !wf.lines().any(|l| l.starts_with("concurrency:")),
        "{}: a workflow-wide group runs one dispatch at a time",
        path.display()
    );
    let group: Vec<&str> = wf
        .lines()
        .skip_while(|l| *l != "  build:")
        .skip(1)
        .take_while(|l| l.is_empty() || l.starts_with("    "))
        .skip_while(|l| *l != "    concurrency:")
        .skip(1)
        .take_while(|l| l.starts_with("      "))
        .map(str::trim)
        .collect();
    assert_eq!(
        group,
        [
            "group: eb-pipeline-${{ matrix.target }}",
            "cancel-in-progress: false",
        ],
        "{}: the build job's concurrency",
        path.display()
    );
    let guide = root().join("docs/orgmode/howto/build-from-ci.org");
    let page = std::fs::read_to_string(&guide).unwrap();
    let text = page.split_whitespace().collect::<Vec<_>>().join(" ");
    for needed in [
        "The build job's =concurrency= group, =eb-pipeline-<target>=,",
        "=cancel-in-progress: false= leaves the running build alone",
        "GitHub keeps only one pending job per group, so a third dispatch replaces the waiting one",
        "It keeps two dispatches off one target, not off one path",
    ] {
        assert!(
            text.contains(needed),
            "{} lacks `{needed}`",
            guide.display()
        );
    }
}
