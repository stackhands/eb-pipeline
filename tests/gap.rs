//! Opt-in `check --gap-out` delegation to `eb-stack`.

mod common;

use common::{code, eb_pipeline, setup, stderr, stdout, write_exec};

const EB_STACK_GAP: &str = r#"#!/bin/sh
echo "$*" >> "$STUB_DIR/gap.calls"
case "$1 $2" in
  "stack gap")
    while [ "$#" -gt 0 ]; do
      if [ "$1" = "--out-dir" ]; then out=$2; shift; fi
      shift
    done
    mkdir -p "$out"
    printf 'route\tname\nrecipe\tLocal\n' > "$out/gap.tsv"
    printf 'Local-1.0-GCCcore-15.2.0.eb\n' > "$out/roots.txt"
    echo "gap warning" >&2
    echo "site=1 in-stack=0 filtered=0 open-layer=0 recipe=1 open-easyconfigs=0 no-recipe=0 excluded=0 out=$out"
    exit "${STUB_GAP_EXIT:-0}"
    ;;
  "stack shadow")
    printf '{"hits":[],"recipes":1,"base_names":2,"same":0,"different":0,"unversioned":0}\n'
    echo "shadow warning" >&2
    [ -n "${STUB_SHADOW_SIGNAL:-}" ] && kill "-$STUB_SHADOW_SIGNAL" "$$"
    exit "${STUB_SHADOW_EXIT:-0}"
    ;;
esac
exit 2
"#;

fn configured() -> common::T {
    let t = setup(
        r#"
[shards]
enabled = false
easyconfigs = ["/recipes/local"]
installed_modules_dirs = ["/base/cpu"]
installed_modules_dirs_from_env = ["GAP_BASE"]
[watch]
enabled = false
modules_dirs = ["/base/cpu", "/base/gpu"]
easyconfigs = ["/recipes/local", "/recipes/extra"]
"#,
    );
    write_exec(&t.stubs.join("eb-stack"), EB_STACK_GAP);
    let text = std::fs::read_to_string(&t.site)
        .unwrap()
        .replacen(
            "name = \"cpu\"",
            "name = \"cpu\"\nmodules_root = \"/local/cpu/modules\"",
            1,
        )
        .replacen(
            "name = \"gpu\"",
            "name = \"gpu\"\nmodules_root = \"/local/gpu/modules\"",
            1,
        );
    std::fs::write(&t.site, text).unwrap();
    t
}

#[test]
fn gap_and_shadow_use_the_site_paths_and_keep_eb_stack_outputs() {
    let t = configured();
    let out_dir = t.root().join("gap-report");
    let out = eb_pipeline(
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
        &[("GAP_BASE", "/base/accel")],
    );
    assert_eq!(code(&out), 0, "{}", stderr(&out));
    assert_eq!(
        t.calls("gap"),
        format!(
            "\
stack gap --site /local/cpu/modules/all --site /local/gpu/modules/all --installed /base/cpu --installed /base/accel --installed /base/gpu --easyconfigs /recipes/local --easyconfigs /recipes/extra --generation 2026.1 --out-dir {dir}
stack shadow --easyconfigs /recipes/local --easyconfigs /recipes/extra --base /base/cpu --base /base/accel --base /base/gpu --json
",
            dir = out_dir.display()
        )
    );
    assert_eq!(
        stdout(&out),
        format!(
            "site=1 in-stack=0 filtered=0 open-layer=0 recipe=1 open-easyconfigs=0 no-recipe=0 excluded=0 out={}\n",
            out_dir.display()
        )
    );
    assert_eq!(
        std::fs::read_to_string(out_dir.join("shadow.json")).unwrap(),
        "{\"hits\":[],\"recipes\":1,\"base_names\":2,\"same\":0,\"different\":0,\"unversioned\":0}\n"
    );
    assert!(out_dir.join("gap.tsv").is_file());
    assert!(out_dir.join("roots.txt").is_file());
    assert_eq!(
        std::fs::read_to_string(out_dir.join("gap.stderr.log")).unwrap(),
        "gap warning\n"
    );
    assert_eq!(
        std::fs::read_to_string(out_dir.join("shadow.stderr.log")).unwrap(),
        "shadow warning\n"
    );
}

#[test]
fn plain_check_is_byte_identical_and_starts_nothing() {
    let t = configured();
    let plain = eb_pipeline(&t, &["check", "--site", &t.site.display().to_string()], &[]);
    assert_eq!(code(&plain), 0, "{}", stderr(&plain));
    assert_eq!(stdout(&plain), "");
    assert_eq!(stderr(&plain), "INFO [check] t: site file is valid\n");
    assert!(t.calls("gap").is_empty());
}

#[test]
fn gap_options_require_each_other() {
    let t = configured();
    let out_dir = t.root().join("gap-report");
    let no_generation = eb_pipeline(
        &t,
        &[
            "check",
            "--site",
            &t.site.display().to_string(),
            "--gap-out",
            &out_dir.display().to_string(),
        ],
        &[],
    );
    assert_eq!(code(&no_generation), 2, "{}", stderr(&no_generation));
    let no_out = eb_pipeline(
        &t,
        &[
            "check",
            "--site",
            &t.site.display().to_string(),
            "--generation",
            "2026.1",
        ],
        &[],
    );
    assert_eq!(code(&no_out), 2, "{}", stderr(&no_out));
}

#[test]
fn a_missing_eb_stack_is_a_usage_error() {
    let t = configured();
    let mut site = std::fs::read_to_string(&t.site).unwrap();
    site.push_str(&format!(
        "\n[site.tools.eb_stack]\nbin = \"{}/missing-eb-stack\"\n",
        t.root().display()
    ));
    std::fs::write(&t.site, site).unwrap();
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
        ],
        &[("GAP_BASE", "/base/accel")],
    );
    assert_eq!(code(&output), 2, "{}", stderr(&output));
    assert!(
        stderr(&output).contains("cannot run"),
        "{}",
        stderr(&output)
    );
}

#[test]
fn shadow_failure_is_returned_after_its_json_is_written() {
    let t = configured();
    let out_dir = t.root().join("gap-report");
    let out = eb_pipeline(
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
        &[("GAP_BASE", "/base/accel"), ("STUB_SHADOW_EXIT", "1")],
    );
    assert_eq!(code(&out), 1, "{}", stderr(&out));
    assert!(out_dir.join("shadow.json").is_file());
    assert!(
        stderr(&out).contains("stack shadow") && stderr(&out).contains("exited 1"),
        "{}",
        stderr(&out)
    );
}

#[test]
fn a_subprocess_signal_keeps_the_shell_exit_code() {
    let t = configured();
    let out_dir = t.root().join("gap-report");
    let out = eb_pipeline(
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
        &[("GAP_BASE", "/base/accel"), ("STUB_SHADOW_SIGNAL", "TERM")],
    );
    assert_eq!(code(&out), 143, "{}", stderr(&out));
    assert!(out_dir.join("shadow.json").is_file());
}

#[test]
fn a_gap_failure_stops_before_shadow() {
    let t = configured();
    let out_dir = t.root().join("gap-report");
    let out = eb_pipeline(
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
        &[("GAP_BASE", "/base/accel"), ("STUB_GAP_EXIT", "2")],
    );
    assert_eq!(code(&out), 2, "{}", stderr(&out));
    assert_eq!(t.calls("gap").lines().count(), 1);
    assert!(!out_dir.join("shadow.json").exists());
}
