//! Opt-in `check --stack-checks` delegation to an eb-stack subprocess.

mod common;

use common::{code, eb_pipeline, setup, stderr, write_exec};

fn configured() -> common::T {
    let t = setup(
        r#"
[shards]
enabled = false
easyconfigs = ["/robot/easyconfigs"]
[sbom]
enabled = false
lock = "/locks/baseline.json"
"#,
    );
    let text = std::fs::read_to_string(&t.site)
        .unwrap()
        .replacen(
            "name = \"cpu\"",
            "name = \"cpu\"\ncpu_target = \"x86_64/amd/zen4\"",
            1,
        )
        .replacen(
            "[filesystems]",
            &format!(
                "[filesystems]\nsource_paths = [\"{}/sources/{{target}}\"]\nconfigs_repo = \"/repos/easyconfigs\"",
                t.root().display()
            ),
            1,
        )
        .replacen(
            "kind = \"local\"",
            "kind = \"slurm\"\nlaunch = \"/bin/false\"",
            1,
        );
    std::fs::write(&t.site, text).unwrap();
    t
}

#[test]
fn requested_checks_use_site_paths_and_selected_target_without_allocating() {
    let t = configured();
    let list = t.root().join("stack.yml");
    std::fs::write(&list, "easyconfigs: []\n").unwrap();
    let site = t.site.display().to_string();
    let list = list.display().to_string();
    let out = eb_pipeline(
        &t,
        &[
            "check",
            "--site",
            &site,
            "--list",
            &list,
            "--target",
            "cpu",
            "--stack-checks",
            "files,arch,cuda-cc,generation,known-failures",
        ],
        &[],
    );
    assert_eq!(code(&out), 0, "{}", stderr(&out));
    let calls = t.calls("ebstack");
    let expected = [
        format!(
            "stack files --easystack {list} --easyconfigs /robot/easyconfigs --easyconfigs-repo /repos/easyconfigs --sourcepath {}/sources/cpu",
            t.root().display()
        ),
        format!(
            "stack arch --arch x86_64/amd/zen4 --easystack {list} --easyconfigs /robot/easyconfigs"
        ),
        format!("stack cuda-cc --easystack {list} --easyconfigs /robot/easyconfigs"),
        format!("stack generation --easystack {list}"),
        format!(
            "stack known-failures --arch x86_64/amd/zen4 --easystack {list} --easyconfigs /robot/easyconfigs --strict"
        ),
    ]
    .join("\n")
        + "\n";
    assert_eq!(calls, expected);
    assert!(!t.root().join("srun.jobs").exists());
}

#[test]
fn no_stack_checks_preserves_plain_check_and_a_failure_fails_check() {
    let t = configured();
    let site = t.site.display().to_string();
    let plain = eb_pipeline(&t, &["check", "--site", &site], &[]);
    assert_eq!(code(&plain), 0, "{}", stderr(&plain));
    assert!(t.calls("ebstack").is_empty());

    let list = t.root().join("stack.yml");
    std::fs::write(&list, "easyconfigs: []\n").unwrap();
    let list = list.display().to_string();
    let failed = eb_pipeline(
        &t,
        &[
            "check",
            "--site",
            &site,
            "--list",
            &list,
            "--stack-checks",
            "files,generation",
        ],
        &[("STUB_EB_STACK_FAIL", "1")],
    );
    assert_eq!(code(&failed), 1, "{}", stderr(&failed));
    let err = stderr(&failed);
    assert!(err.contains("files failed with status 1"), "{err}");
    assert!(err.contains("generation failed with status 1"), "{err}");
    assert!(
        err.contains("eb-stack checks failed: files, generation"),
        "{err}"
    );
}

#[test]
fn a_stack_check_usage_exit_is_preserved() {
    let t = configured();
    let list = t.root().join("stack.yml");
    std::fs::write(&list, "easyconfigs: []\n").unwrap();
    let output = eb_pipeline(
        &t,
        &[
            "check",
            "--site",
            &t.site.display().to_string(),
            "--list",
            &list.display().to_string(),
            "--stack-checks",
            "files",
        ],
        &[("STUB_EB_STACK_FAIL", "2")],
    );
    assert_eq!(code(&output), 2, "{}", stderr(&output));
    assert!(
        stderr(&output).contains("files failed with status 2"),
        "{}",
        stderr(&output)
    );
}

#[test]
fn stack_checks_require_a_list_and_validate_the_target_before_starting() {
    let t = configured();
    let site = t.site.display().to_string();
    let missing = eb_pipeline(
        &t,
        &["check", "--site", &site, "--stack-checks", "files"],
        &[],
    );
    assert_eq!(code(&missing), 2, "{}", stderr(&missing));

    let list = t.root().join("stack.yml");
    std::fs::write(&list, "easyconfigs: []\n").unwrap();
    let list = list.display().to_string();
    let unknown = eb_pipeline(
        &t,
        &[
            "check",
            "--site",
            &site,
            "--list",
            &list,
            "--target",
            "missing",
            "--stack-checks",
            "arch",
        ],
        &[],
    );
    assert_eq!(code(&unknown), 2, "{}", stderr(&unknown));
    assert!(t.calls("ebstack").is_empty());
}

#[test]
fn easystack_checks_refuse_a_plain_build_list() {
    let t = configured();
    let list = t.root().join("build.list");
    std::fs::write(&list, "Pkg-1.0.eb\n").unwrap();
    let output = eb_pipeline(
        &t,
        &[
            "check",
            "--site",
            &t.site.display().to_string(),
            "--list",
            &list.display().to_string(),
            "--stack-checks",
            "generation",
        ],
        &[],
    );
    assert_eq!(code(&output), 2, "{}", stderr(&output));
    assert!(t.calls("ebstack").is_empty());
    assert!(
        stderr(&output).contains("needs an easystack .yml or .yaml path"),
        "{}",
        stderr(&output)
    );
}

#[test]
fn provided_cannot_share_an_input_with_easystack_checks() {
    let t = configured();
    let list = t.root().join("stack.yml");
    std::fs::write(&list, "easyconfigs: []\n").unwrap();
    let output = eb_pipeline(
        &t,
        &[
            "check",
            "--site",
            &t.site.display().to_string(),
            "--list",
            &list.display().to_string(),
            "--stack-checks",
            "provided,files",
        ],
        &[],
    );
    assert_eq!(code(&output), 2, "{}", stderr(&output));
    assert!(t.calls("ebstack").is_empty());
    assert!(
        stderr(&output).contains("cannot combine provided with easystack checks"),
        "{}",
        stderr(&output)
    );
}

#[test]
fn check_from_commit_refuses_invalid_json_from_a_successful_eb_stack() {
    let t = setup("");
    write_exec(
        &t.stubs.join("eb-stack"),
        "#!/bin/sh\necho \"$*\" >> \"$STUB_DIR/ebstack.calls\"\nprintf '{'\n",
    );
    let list = t.root().join("stack.yml");
    std::fs::write(&list, "easyconfigs: []\n").unwrap();
    let list = list.display().to_string();

    let output = eb_pipeline(&t, &["check", "--list", &list, "--check-from-commit"], &[]);

    assert_eq!(code(&output), 1, "{}", stderr(&output));
    assert!(
        stderr(&output).contains("eb-stack stack from-commit wrote invalid JSON"),
        "{}",
        stderr(&output)
    );
    assert_eq!(
        t.calls("ebstack"),
        format!("stack from-commit --easystack {list} --json\n")
    );
}

#[test]
fn shadow_refuses_invalid_json_from_a_successful_eb_stack() {
    let t = setup(
        r#"
[shards]
enabled = false
easyconfigs = ["/recipes/local"]
installed_modules_dirs = ["/base/cpu"]
"#,
    );
    write_exec(
        &t.stubs.join("eb-stack"),
        r#"#!/bin/sh
echo "$*" >> "$STUB_DIR/gap.calls"
case "$1 $2" in
  "stack gap")
    while [ "$#" -gt 0 ]; do
      if [ "$1" = "--out-dir" ]; then out=$2; shift; fi
      shift
    done
    mkdir -p "$out"
    exit 0;;
  "stack shadow")
    printf '{'
    exit 0;;
esac
exit 2
"#,
    );
    let out_dir = t.root().join("gap-report");
    let output = eb_pipeline(
        &t,
        &[
            "check",
            "--site",
            &t.site.display().to_string(),
            "--gap-out",
            &out_dir.display().to_string(),
            "--generation",
            "2026.1",
            "--shadow",
        ],
        &[],
    );

    assert_eq!(code(&output), 1, "{}", stderr(&output));
    assert!(
        stderr(&output).contains("eb-stack stack shadow wrote invalid JSON"),
        "{}",
        stderr(&output)
    );
    assert!(!out_dir.join("shadow.json").exists());
    assert!(t
        .calls("gap")
        .contains("stack shadow --easyconfigs /recipes/local --base /base/cpu --json\n"));
}

#[test]
fn plain_list_checks_run_together_on_one_build_list() {
    let t = configured();
    let list = t.root().join("build.list");
    std::fs::write(&list, "Pkg-1.0-foss-2026.1.eb\n").unwrap();
    let list = list.display().to_string();
    let out = eb_pipeline(
        &t,
        &[
            "check",
            "--site",
            &t.site.display().to_string(),
            "--list",
            &list,
            "--stack-checks",
            "provided,buildenv",
        ],
        &[],
    );
    assert_eq!(code(&out), 0, "{}", stderr(&out));
    let expected = [
        format!("stack provided --build-list {list} --baseline-lock /locks/baseline.json --strict"),
        format!("stack buildenv --build-list {list} --strict"),
    ]
    .join("\n")
        + "\n";
    assert_eq!(t.calls("ebstack"), expected);
}

#[test]
fn buildenv_cannot_share_an_input_with_easystack_checks() {
    let t = configured();
    let list = t.root().join("stack.yml");
    std::fs::write(&list, "easyconfigs: []\n").unwrap();
    let output = eb_pipeline(
        &t,
        &[
            "check",
            "--site",
            &t.site.display().to_string(),
            "--list",
            &list.display().to_string(),
            "--stack-checks",
            "generation,buildenv,provided",
        ],
        &[],
    );
    assert_eq!(code(&output), 2, "{}", stderr(&output));
    assert!(t.calls("ebstack").is_empty());
    assert!(
        stderr(&output).contains("cannot combine buildenv and provided with easystack checks"),
        "{}",
        stderr(&output)
    );
}
