//! `run` with sharding, admission and resume against an unmodified eb-stack
//! binary. The stub programs of the other run tests answer in the shape their
//! authors expect; this file reads what eb-stack prints and writes.
#![allow(clippy::disallowed_methods)]

#[path = "common/exec_stub.rs"]
mod exec_stub;

use serde_json::Value;
use std::path::{Path, PathBuf};
use std::process::Command;

const EB: &str = r#"#!/bin/sh
D="${STUB_DIR:?}"
case " $* " in *" --fetch "*) exit 0;; esac
echo "$*" >> "$D/eb.calls"
stem=$(basename "$1" .eb)
name=${stem%%-[0-9]*}
ver=${stem#"$name"-}
dir="${EASYBUILD_INSTALLPATH:?}/modules/all/$name"
mkdir -p "$dir"
printf 'help([[%s]])\n' "$stem" > "$dir/$ver.lua"
exit 0
"#;

const RECIPES: [(&str, &str, &str); 3] = [
    ("Foo", "1.0", ""),
    ("Bar", "2.0", "dependencies = [('Foo', '1.0')]\n"),
    ("Baz", "3.0", ""),
];

struct World {
    root: PathBuf,
    site: PathBuf,
    stubs: PathBuf,
    list: PathBuf,
    _keep: tempfile::TempDir,
}

fn world(eb_stack: &Path) -> World {
    let keep = tempfile::tempdir().unwrap();
    let root = keep.path().to_path_buf();
    let stubs = root.join("stubs");
    std::fs::create_dir(&stubs).unwrap();
    exec_stub::write_exec(&stubs.join("eb"), EB);
    let mut list = String::new();
    for (name, version, extra) in RECIPES {
        let dir = root
            .join("easyconfigs")
            .join(name[..1].to_lowercase())
            .join(name);
        std::fs::create_dir_all(&dir).unwrap();
        let file = format!("{name}-{version}-GCCcore-13.2.0.eb");
        std::fs::write(
            dir.join(&file),
            format!(
                "easyblock = 'ConfigureMake'\nname = '{name}'\nversion = '{version}'\n\
                 homepage = 'https://example.invalid'\ndescription = 'fixture'\n\
                 toolchain = {{'name': 'GCCcore', 'version': '13.2.0'}}\n{extra}"
            ),
        )
        .unwrap();
        list.push_str(&file);
        list.push('\n');
    }
    let list_path = root.join("list.txt");
    std::fs::write(&list_path, list).unwrap();
    let site = root.join("site.toml");
    std::fs::write(
        &site,
        format!(
            r#"schema_version = 1
kind = "site"
[site]
name = "example"
[site.tools.eb_stack]
bin = "{bin}"
[scheduler]
kind = "local"
[[targets]]
name = "cpu"
[filesystems]
scratch = "{root}/scratch"
tmp = "{root}/tmp"
build_path = "{root}/build"
install_root = "{root}/install"
log_root = "{root}/logs"
[modules]
init = []
[shards]
count = 2
easyconfigs = ["{root}/easyconfigs"]
[admission]
quota_cmd = "echo '{{\"inodes_available\": 5000000}}'"
live_jobs_cmd = "echo 4411"
lock = "{root}/admission.lock"
reservations = "{root}/res"
default_footprint = 60000
[resume]
enabled = true
easyconfigs = ["{root}/easyconfigs"]
"#,
            bin = eb_stack.display(),
            root = root.display(),
        ),
    )
    .unwrap();
    World {
        root,
        site,
        stubs,
        list: list_path,
        _keep: keep,
    }
}

fn run(w: &World, run_id: &str, extra: &[&str]) -> (i32, String, Value) {
    let result = w.root.join(format!("{run_id}.json"));
    let output = Command::new(env!("CARGO_BIN_EXE_eb-pipeline"))
        .args(["run", "--site"])
        .arg(&w.site)
        .arg("--list")
        .arg(&w.list)
        .args(["--target", "cpu", "--build-type", "production"])
        .args(["--run-id", run_id, "--result"])
        .arg(&result)
        .args(extra)
        .env(
            "PATH",
            format!(
                "{}:{}",
                w.stubs.display(),
                std::env::var("PATH").unwrap_or_default()
            ),
        )
        .env("STUB_DIR", &w.stubs)
        .env("SLURM_JOB_ID", "4411")
        .output()
        .unwrap();
    let stderr = String::from_utf8_lossy(&output.stderr).into_owned();
    let value = std::fs::read_to_string(&result)
        .ok()
        .and_then(|text| serde_json::from_str(&text).ok())
        .unwrap_or(Value::Null);
    (output.status.code().unwrap_or(127), stderr, value)
}

fn eb_calls(w: &World) -> usize {
    std::fs::read_to_string(w.stubs.join("eb.calls"))
        .map(|text| text.lines().count())
        .unwrap_or(0)
}

#[test]
fn real_eb_stack_shards_admits_and_resumes_a_run() {
    let Some(eb_stack) = std::env::var_os("EB_STACK_REAL") else {
        return;
    };
    let w = world(Path::new(&eb_stack));

    let (code, stderr, first) = run(&w, "first", &[]);
    assert_eq!(code, 0, "{stderr}");
    assert_eq!(first["counts"]["success"], 3, "{first}");
    assert_eq!(first["sharded"], true, "{first}");
    let admission = first["admission"].as_array().expect("admission records");
    assert_eq!(admission.len(), 2, "{first}");
    for record in admission {
        assert_eq!(record["outcome"], "admitted", "{record}");
        let verdict: Value = serde_json::from_str(record["last_json"].as_str().unwrap()).unwrap();
        assert_eq!(verdict["admitted"], true, "{verdict}");
        assert_eq!(verdict["job"], "4411", "{verdict}");
    }
    assert_eq!(eb_calls(&w), 3);
    let reservations: Vec<_> = std::fs::read_dir(w.root.join("res"))
        .unwrap()
        .map(|entry| entry.unwrap().file_name().into_string().unwrap())
        .filter(|name| name.ends_with(".json"))
        .collect();
    assert!(reservations.is_empty(), "{reservations:?}");

    let (code, stderr, second) = run(
        &w,
        "second",
        &[
            "--resume-result",
            w.root.join("first.json").to_str().unwrap(),
        ],
    );
    assert_eq!(code, 0, "{stderr}");
    assert_eq!(
        stderr.matches("resumed; outputs still present").count(),
        3,
        "{stderr}"
    );
    assert_eq!(second["counts"]["skipped"], 3, "{second}");
    assert_eq!(second["input_digest"], first["input_digest"], "{second}");
    assert_eq!(
        first["input_digest"].as_str().map(str::len),
        Some(64),
        "{first}"
    );
    assert_eq!(eb_calls(&w), 3, "the second run built nothing");
}
