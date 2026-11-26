//! End-to-end from-commit check against an unmodified eb-stack build.
#![allow(clippy::disallowed_methods)]

use std::path::Path;
use std::process::Command;

fn git(dir: &Path, args: &[&str]) -> String {
    let output = Command::new("git")
        .arg("-C")
        .arg(dir)
        .arg("-c")
        .arg("user.name=Rohit Goswami")
        .arg("-c")
        .arg("user.email=rgoswami@ieee.org")
        .args(args)
        .output()
        .expect("spawn git");
    assert!(
        output.status.success(),
        "git {args:?} in {}: {}",
        dir.display(),
        String::from_utf8_lossy(&output.stderr)
    );
    String::from_utf8(output.stdout)
        .expect("utf8")
        .trim()
        .to_string()
}

fn write(dir: &Path, rel: &str, text: &str) {
    let path = dir.join(rel);
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(path, text).unwrap();
}

fn commit_all(dir: &Path, message: &str) -> String {
    git(dir, &["add", "."]);
    git(dir, &["commit", "-m", message]);
    git(dir, &["rev-parse", "HEAD"])
}

#[test]
fn real_eb_stack_reports_a_released_pin() {
    let Some(eb_stack) = std::env::var_os("EB_STACK_REAL") else {
        return;
    };
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path();
    let easyconfigs = root.join("easyconfigs");
    std::fs::create_dir_all(&easyconfigs).unwrap();
    git(&easyconfigs, &["init", "-b", "main"]);
    write(&easyconfigs, "README", "root\n");
    let _ = commit_all(&easyconfigs, "root");
    git(&easyconfigs, &["tag", "easybuild-easyconfigs-v5.0.0"]);
    write(
        &easyconfigs,
        "easybuild/easyconfigs/f/Foo/Foo-1.0.eb",
        "one\n",
    );
    let c1 = commit_all(&easyconfigs, "add Foo");
    git(&easyconfigs, &["tag", "easybuild-easyconfigs-v5.4.0"]);
    let list = root.join("20261011-eb-5.4.0-UCX.yml");
    std::fs::write(
        &list,
        format!("easyconfigs:\n  - Foo-1.0.eb:\n      options:\n        from-commit: {c1}\n"),
    )
    .unwrap();
    let log = root.join("real-from-commit.log");
    let eb_stack = Path::new(&eb_stack);
    let out = Command::new(env!("CARGO_BIN_EXE_eb-pipeline"))
        .args(["check", "--list"])
        .arg(&list)
        .args(["--check-from-commit", "--easyconfigs-repo"])
        .arg(&easyconfigs)
        .env(
            "PATH",
            format!(
                "{}:{}",
                eb_stack.parent().unwrap().display(),
                std::env::var("PATH").unwrap_or_default()
            ),
        )
        .output()
        .unwrap();
    let text = format!(
        "eb_stack={}\nlist={}\neasyconfigs_repo={}\nexit: {}\nstdout:\n{}\nstderr:\n{}\n",
        eb_stack.display(),
        list.display(),
        easyconfigs.display(),
        out.status,
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    );
    std::fs::write(&log, &text).unwrap();
    eprintln!("wrote {}", log.display());
    assert_eq!(out.status.code(), Some(1), "{text}");
    assert!(
        String::from_utf8_lossy(&out.stderr).contains("from-commit failed"),
        "{text}"
    );
}
