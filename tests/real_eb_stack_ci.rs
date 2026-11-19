//! A test that reads `EB_STACK_REAL` runs only where the variable is set.
//! The `ci` workflow has to set it, for every such test, to a pinned release.

use std::path::{Path, PathBuf};

fn root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
}

fn job(workflow: &str) -> String {
    let mut lines = workflow.lines().skip_while(|l| *l != "  real-eb-stack:");
    assert!(lines.next().is_some(), "ci.yml has no real-eb-stack job");
    let is_job_key = |l: &str| {
        l.strip_prefix("  ")
            .and_then(|k| k.strip_suffix(':'))
            .is_some_and(|k| !k.is_empty() && !k.contains(' '))
    };
    lines
        .take_while(|l| !is_job_key(l))
        .collect::<Vec<_>>()
        .join("\n")
}

fn tests_reading_eb_stack_real() -> Vec<String> {
    let mut found = Vec::new();
    for entry in std::fs::read_dir(root().join("tests")).unwrap() {
        let path = entry.unwrap().path();
        if path.extension().is_some_and(|e| e == "rs")
            && path.file_name().is_some_and(|n| n != "real_eb_stack_ci.rs")
            && std::fs::read_to_string(&path)
                .unwrap()
                .contains("EB_STACK_REAL")
        {
            found.push(stem(&path));
        }
    }
    found.sort();
    found
}

fn stem(path: &Path) -> String {
    path.file_stem().unwrap().to_str().unwrap().to_string()
}

#[test]
fn ci_runs_every_test_that_needs_the_real_eb_stack() {
    let workflow = std::fs::read_to_string(root().join(".github/workflows/ci.yml")).unwrap();
    let job = job(&workflow);
    let job = job.as_str();
    let tests = tests_reading_eb_stack_real();
    assert!(!tests.is_empty());
    for name in &tests {
        assert!(
            job.contains(&format!("--test {name}")),
            "the real-eb-stack job does not run tests/{name}.rs"
        );
    }
    for line in job.lines().filter(|l| l.contains("--test ")) {
        let name = line
            .trim()
            .trim_end_matches('\\')
            .trim()
            .trim_start_matches("cargo test --locked")
            .trim()
            .trim_start_matches("--test ")
            .trim()
            .to_string();
        assert!(
            tests.contains(&name),
            "the real-eb-stack job runs tests/{name}.rs, which does not read EB_STACK_REAL"
        );
    }
}

#[test]
fn ci_installs_a_pinned_release_and_exports_it() {
    let workflow = std::fs::read_to_string(root().join(".github/workflows/ci.yml")).unwrap();
    let job = job(&workflow);
    let job = job.as_str();
    assert!(
        job.contains("https://github.com/stackhands/eb-stack/releases/download/v0.8.0/"),
        "the job does not download a tagged eb-stack release"
    );
    let sum = job
        .lines()
        .find(|l| l.contains("sha256sum --check"))
        .expect("the job does not check the archive");
    assert!(
        sum.split_whitespace()
            .any(|w| w.len() == 64 && w.bytes().all(|b| b.is_ascii_hexdigit())),
        "the checksum is not a SHA-256: {sum}"
    );
    let export = job
        .find("EB_STACK_REAL=")
        .expect("the job does not export EB_STACK_REAL");
    assert!(job.contains(">> \"$GITHUB_ENV\""));
    let tests = job.find("cargo test").expect("the job runs no tests");
    assert!(export < tests, "EB_STACK_REAL is set after the tests ran");
    assert!(
        job.contains("grep -Fx \"eb-stack 0.8.0\""),
        "the job does not check the version it installed"
    );
}
