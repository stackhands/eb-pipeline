//! `targets`: the site file's target names, for a CI matrix or a shell loop.

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

#[test]
fn the_readme_lists_targets_among_the_subcommands_and_streams() {
    let readme = std::fs::read_to_string(root().join("README.md")).unwrap();
    assert!(
        readme
            .lines()
            .any(|l| l.starts_with("| `targets --site FILE [--format text\\|json]` |")),
        "README.md has no `targets` row in its subcommand table"
    );
    let streams = readme
        .lines()
        .find(|l| l.starts_with("Streams: stdout carries "))
        .unwrap_or_default();
    assert!(streams.contains("`targets`"), "{streams:?}");
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

/// Every target gets one EASYBUILD_BUILDPATH, and EasyBuild removes the build
/// directory of an easyconfig when a build of it starts.
#[test]
fn the_ci_example_builds_one_target_at_a_time() {
    let path = root().join("examples/ci/github-actions.yml");
    let wf = std::fs::read_to_string(&path).unwrap();
    assert!(
        wf.lines().any(|l| l.trim() == "max-parallel: 1"),
        "{} lacks `max-parallel: 1`",
        path.display()
    );
}
