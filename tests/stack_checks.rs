//! Opt-in `check --stack-checks` delegation to an eb-stack subprocess.

mod common;

use common::{code, eb_pipeline, setup, stderr};

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
            "files,arch,cuda-cc,generation,provided,known-failures",
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
            "stack provided --build-list {list} --baseline-lock /locks/baseline.json --strict"
        ),
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
