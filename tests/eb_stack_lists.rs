//! eb-stack's outputs as build lists. The files in `tests/fixtures/eb-stack`
//! are what eb-stack 0.7.4 wrote for the roots pigz and Autoconf:
//! `rebuild-set.yml` by `rebuild-set`, `stack-easystack.yml` by `stack
//! easystack --option pigz-2.8-GCCcore-15.2.0.eb:rebuild=true` from the lock
//! of `stack solve`, `stack-easystack-options.yml` by the same with the
//! options `pigz-2.8-GCCcore-15.2.0.eb:cuda-compute-capabilities=8.0`,
//! `pigz-2.8-GCCcore-15.2.0.eb:parallel=4`,
//! `pigz-2.8-GCCcore-15.2.0.eb:rebuild=true` and
//! `Perl-5.42.0-GCCcore-15.2.0.eb:accept-eula-for=A,B`,
//! `stack-easystack-one.yml` by the same with the options
//! `pigz-2.8-GCCcore-15.2.0.eb:parallel=1` and
//! `pigz-2.8-GCCcore-15.2.0.eb:rebuild=1`, and `solve.build.list` and
//! `solve.jenkins.list` by `stack solve --build-list-out` and
//! `--jenkins-build-list-out`.
#![allow(clippy::disallowed_methods)]

use serde_json::Value;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};

fn fixture(name: &str) -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures/eb-stack")
        .join(name)
}

fn site(dir: &Path, extra: &str) -> PathBuf {
    let r = dir.display();
    let path = dir.join("site.toml");
    let text = format!(
        r#"
schema_version = 1
kind = "site"
[site]
name = "t"
[site.tools.eb_staged_rebuild]
bin = "eb-staged-rebuild"
[scheduler]
kind = "local"
[[targets]]
name = "cpu"
{extra}
[filesystems]
scratch = "{r}/scratch"
tmp = "{r}/tmp"
build_path = "{r}/build"
install_root = "{r}/install"
log_root = "{r}/logs"
[modules]
init = []
[rebuild]
staged = true
"#
    );
    std::fs::write(&path, text).unwrap();
    path
}

fn plan(site: &Path, list: &Path) -> Output {
    Command::new(env!("CARGO_BIN_EXE_eb-pipeline"))
        .args(["plan", "--site"])
        .arg(site)
        .arg("--list")
        .arg(list)
        .args(["--target", "cpu", "--build-type", "production"])
        .output()
        .unwrap()
}

fn plan_of(site: &Path, list: &Path) -> Value {
    let o = plan(site, list);
    assert_eq!(
        o.status.code(),
        Some(0),
        "{}",
        String::from_utf8_lossy(&o.stderr)
    );
    serde_json::from_slice(&o.stdout).unwrap()
}

fn plan_error(site: &Path, list: &Path, bytes: &[u8]) -> String {
    std::fs::write(list, bytes).unwrap();
    let output = plan(site, list);
    assert_eq!(output.status.code(), Some(2));
    assert_eq!(output.stdout, b"");
    String::from_utf8(output.stderr).unwrap()
}

fn phase<'a>(v: &'a Value, name: &str) -> &'a Value {
    v["phases"]
        .as_array()
        .unwrap()
        .iter()
        .find(|p| p["name"] == name)
        .unwrap()
}

fn argv(c: &Value) -> Vec<&str> {
    c["argv"]
        .as_array()
        .unwrap()
        .iter()
        .map(|a| a.as_str().unwrap())
        .collect()
}

/// The build commands of a plan, as their names and the words after the
/// program: the easyconfig and its options.
fn builds(v: &Value) -> Vec<(String, Vec<String>)> {
    phase(v, "build")["commands"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|c| c["name"].as_str().unwrap().starts_with("build:"))
        .map(|c| {
            let a = argv(c);
            let at = a.iter().position(|w| w.ends_with(".eb")).unwrap();
            (
                c["name"].as_str().unwrap().to_string(),
                a[at..].iter().map(|w| w.to_string()).collect(),
            )
        })
        .collect()
}

#[test]
fn a_rebuild_set_easystack_rebuilds_its_easyconfig_through_eb_staged_rebuild() {
    let d = tempfile::tempdir().unwrap();
    let module = d
        .path()
        .join("install/modules/all/pigz/2.8-GCCcore-15.2.0.lua");
    std::fs::create_dir_all(module.parent().unwrap()).unwrap();
    std::fs::write(&module, "-- module\n").unwrap();
    let v = plan_of(&site(d.path(), ""), &fixture("rebuild-set.yml"));
    assert_eq!(v["lines"], 1);
    assert_eq!(
        builds(&v),
        [(
            "build:pigz-2.8-GCCcore-15.2.0".to_string(),
            vec!["pigz-2.8-GCCcore-15.2.0.eb".to_string(), "--rebuild".into()]
        )]
    );
    let a = argv(&phase(&v, "build")["commands"][0]);
    assert_eq!(a[..2], ["eb-staged-rebuild", "run"], "{a:?}");
}

#[test]
fn a_lock_easystack_keeps_its_entries_in_order_with_their_options() {
    let d = tempfile::tempdir().unwrap();
    let v = plan_of(&site(d.path(), ""), &fixture("stack-easystack.yml"));
    let want = [
        ("Autoconf-2.72-GCCcore-15.2.0", ""),
        ("M4-1.4.20-GCCcore-15.2.0", ""),
        ("Perl-5.42.0-GCCcore-15.2.0", ""),
        ("pigz-2.8-GCCcore-15.2.0", "--rebuild"),
    ];
    let got = builds(&v);
    assert_eq!(got.len(), want.len(), "{got:?}");
    for ((name, words), (stem, option)) in got.iter().zip(want) {
        assert_eq!(*name, format!("build:{stem}"));
        let mut w = vec![format!("{stem}.eb")];
        w.extend((!option.is_empty()).then(|| option.to_string()));
        assert_eq!(*words, w, "{name}");
    }
    // pigz is not installed, so its rebuild builds in place with plain eb.
    let pigz = phase(&v, "build")["commands"]
        .as_array()
        .unwrap()
        .iter()
        .find(|c| c["name"] == "build:pigz-2.8-GCCcore-15.2.0")
        .unwrap();
    assert_eq!(argv(pigz)[0], "eb");
}

#[test]
fn the_options_eb_stack_writes_reach_eb_as_it_was_given_them() {
    let d = tempfile::tempdir().unwrap();
    let v = plan_of(&site(d.path(), ""), &fixture("stack-easystack-options.yml"));
    let got: Vec<String> = builds(&v).into_iter().map(|(_, w)| w.join(" ")).collect();
    assert_eq!(
        got,
        [
            "Autoconf-2.72-GCCcore-15.2.0.eb",
            "M4-1.4.20-GCCcore-15.2.0.eb",
            "Perl-5.42.0-GCCcore-15.2.0.eb --accept-eula-for=A,B",
            "pigz-2.8-GCCcore-15.2.0.eb --cuda-compute-capabilities=8.0 --parallel=4 --rebuild",
        ]
    );
}

#[test]
fn a_plain_one_that_eb_stack_writes_gives_the_bare_option_as_in_easybuild() {
    let d = tempfile::tempdir().unwrap();
    let module = d
        .path()
        .join("install/modules/all/pigz/2.8-GCCcore-15.2.0.lua");
    std::fs::create_dir_all(module.parent().unwrap()).unwrap();
    std::fs::write(&module, "-- module\n").unwrap();
    let v = plan_of(&site(d.path(), ""), &fixture("stack-easystack-one.yml"));
    let pigz = phase(&v, "build")["commands"]
        .as_array()
        .unwrap()
        .iter()
        .find(|c| c["name"] == "build:pigz-2.8-GCCcore-15.2.0")
        .unwrap();
    let a = argv(pigz);
    assert_eq!(a[..2], ["eb-staged-rebuild", "run"], "{a:?}");
    // eb refuses `--parallel` without a value, as with `eb --easystack`.
    assert_eq!(
        a[a.len() - 3..],
        ["pigz-2.8-GCCcore-15.2.0.eb", "--parallel", "--rebuild"]
    );
}

#[test]
fn the_build_lists_of_solve_are_build_lists_already() {
    let d = tempfile::tempdir().unwrap();
    let s = site(d.path(), "");
    let paths = builds(&plan_of(&s, &fixture("solve.build.list")));
    let names = builds(&plan_of(&s, &fixture("solve.jenkins.list")));
    let stems = |b: &[(String, Vec<String>)]| -> Vec<String> {
        b.iter().map(|(n, w)| format!("{n} {}", w.len())).collect()
    };
    assert_eq!(
        stems(&paths),
        [
            "build:M4-1.4.20-GCCcore-15.2.0 1",
            "build:Perl-5.42.0-GCCcore-15.2.0 1",
            "build:Autoconf-2.72-GCCcore-15.2.0 1",
            "build:pigz-2.8-GCCcore-15.2.0 1",
        ]
    );
    assert_eq!(stems(&names), stems(&paths));
    assert!(paths[0].1[0].starts_with("/cvmfs/"), "{paths:?}");
}

#[test]
fn stack_shard_reads_the_lines_of_an_easystack() {
    let d = tempfile::tempdir().unwrap();
    let shards = "[shards]\ncount = 2\neasyconfigs = [\"/opt/ec\"]\n";
    let s = site(d.path(), "");
    std::fs::write(&s, std::fs::read_to_string(&s).unwrap() + shards).unwrap();
    let v = plan_of(&s, &fixture("stack-easystack.yml"));
    let a = argv(&phase(&v, "shard")["commands"][0]);
    let list = a[a.iter().position(|w| *w == "--build-list").unwrap() + 1];
    assert_eq!(
        list,
        format!(
            "{}/logs/<log-stamp>/cpu/easystack-cpu.list",
            d.path().display()
        )
    );

    // With a selector, the selected list holds the lines, as for any list.
    let s = site(
        d.path(),
        "[targets.select]\noption = \"--only-targets\"\nvalue = \"cpu\"\n",
    );
    std::fs::write(&s, std::fs::read_to_string(&s).unwrap() + shards).unwrap();
    let v = plan_of(&s, &fixture("stack-easystack.yml"));
    let a = argv(&phase(&v, "shard")["commands"][0]);
    let list = a[a.iter().position(|w| *w == "--build-list").unwrap() + 1];
    assert!(list.ends_with("/selected-cpu.list"), "{list}");
}

#[test]
fn the_how_to_shows_what_each_entry_becomes() {
    let page = std::fs::read_to_string(
        PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("docs/orgmode/howto/from-eb-stack.org"),
    )
    .unwrap();
    let rows: Vec<Vec<&str>> = page
        .lines()
        .map(str::trim)
        .skip_while(|l| !l.starts_with("| In the easystack"))
        .skip(2)
        .take_while(|l| l.starts_with('|'))
        .map(|l| l.trim_matches('|').split(" | ").map(str::trim).collect())
        .collect();
    assert!(rows.len() >= 8, "{rows:?}");
    for row in rows {
        let [given, line] = [row[0], row[1]].map(|c| c.trim_matches('='));
        let yaml = match given.strip_prefix("- ") {
            Some(name) => format!("easyconfigs:\n  - {name}\n"),
            None => format!("easyconfigs:\n  - x.eb:\n      options:\n        {given}\n"),
        };
        let got = &eb_pipeline::easystack::parse(&yaml).unwrap()[0];
        let got = match given.starts_with("- ") {
            true => got.raw.clone(),
            false => got.args.join(" "),
        };
        assert_eq!(got, line, "{given}");
    }
}

#[test]
fn an_easystack_that_cannot_be_read_names_the_file_and_the_entry() {
    let d = tempfile::tempdir().unwrap();
    let list = d.path().join("rebuilds.yml");
    std::fs::write(
        &list,
        "easyconfigs:\n  - pigz-2.8-GCCcore-15.2.0.eb\n  - zlib-1.3.1-GCCcore-15.2.0.eb:\n      option:\n        rebuild: true\n",
    )
    .unwrap();
    let o = plan(&site(d.path(), ""), &list);
    assert_eq!(o.status.code(), Some(2));
    let err = String::from_utf8_lossy(&o.stderr);
    assert!(
        err.contains(&format!(
            "easystack {}: entry 2: zlib-1.3.1-GCCcore-15.2.0.eb: only `options` can follow the name, not option",
            list.display()
        )),
        "{err}"
    );
}

#[test]
fn easystack_parse_errors_name_the_source_line_and_column() {
    let dir = tempfile::tempdir().unwrap();
    let site = site(dir.path(), "");
    let list = dir.path().join("input.yml");
    let prefix = format!("ERROR [main] -: easystack {}:", list.display());

    let malformed = plan_error(
        &site,
        &list,
        b"easyconfigs:\n  - x.eb:\n      options:\n        rebuild: [\n",
    );
    assert!(
        malformed.starts_with(&format!("{prefix}5:1:")),
        "{malformed}"
    );

    let tab = plan_error(&site, &list, b"easyconfigs:\n\t- x.eb\n");
    assert!(tab.starts_with(&format!("{prefix}2:1:")), "{tab}");

    let duplicate = plan_error(
        &site,
        &list,
        b"easyconfigs:\n  - x.eb\neasyconfigs:\n  - y.eb\n",
    );
    assert!(
        duplicate.starts_with(&format!("{prefix}3:1: duplicate entry")),
        "{duplicate}"
    );

    let nested_duplicate = plan_error(
        &site,
        &list,
        b"easyconfigs:\n  - First-1.0.eb:\n      options:\n        rebuild: true\n  - Second-2.0.eb:\n      options:\n        rebuild: true\n        rebuild: false\n",
    );
    assert_eq!(
        nested_duplicate,
        format!(
            "{prefix}8:9: easyconfigs[1].Second-2.0.eb.options: duplicate entry with key \"rebuild\"\n"
        )
    );

    let malformed_crlf = plan_error(
        &site,
        &list,
        b"easyconfigs:\r\n  - x.eb:\r\n      options:\r\n        rebuild: [\r\n",
    );
    assert!(
        malformed_crlf.starts_with(&format!("{prefix}5:1:")),
        "{malformed_crlf}"
    );

    let non_utf8 = plan_error(&site, &list, b"easyconfigs:\n  - bad-\xff.eb\n");
    assert_eq!(non_utf8, format!("{prefix}2:9: input is not valid UTF-8\n"));
}

#[test]
fn valid_easystack_encoding_and_newline_variants_keep_plan_bytes() {
    let dir = tempfile::tempdir().unwrap();
    let site = site(dir.path(), "");
    let list = dir.path().join("input.yml");
    let mut outputs = Vec::new();
    for bytes in [
        b"easyconfigs:\n  - x.eb\n".as_slice(),
        b"easyconfigs:\r\n  - x.eb\r\n".as_slice(),
        b"\xef\xbb\xbfeasyconfigs:\n  - x.eb\n".as_slice(),
    ] {
        std::fs::write(&list, bytes).unwrap();
        let output = plan(&site, &list);
        assert_eq!(output.status.code(), Some(0));
        assert_eq!(output.stderr, b"");
        outputs.push(output.stdout);
    }
    assert_eq!(outputs[1], outputs[0]);
    assert_eq!(outputs[2], outputs[0]);
}
