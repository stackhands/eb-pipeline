//! A site file with targets `cpu` and `gpu`, and stub `eb` and
//! `eb-keep-logs` binaries, for the tests of the run summary and what reads
//! it. Each test file uses a part of it.
#![allow(dead_code)]

use serde_json::Value;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::process::{Command, Output, Stdio};

pub const STAMP: &str = "20260101_000000";

/// Fails the easyconfig stems listed in STUB_EB_FAIL.
const EB: &str = r#"#!/bin/sh
D="${STUB_DIR:?}"
ec="$1"; stem="${ec##*/}"; stem="${stem%.eb}"
echo "$*" >> "$D/eb.calls"
case ",${STUB_EB_FAIL:-}," in *",$stem,"*) echo "ERROR: $stem failed"; exit 1;; esac
exit 0
"#;

/// `rotate` exits STUB_ROTATE_EXIT, saying what eb-keep-logs says when it
/// exits 75 because another rotate holds its lock.
const KEEP_LOGS: &str = r#"#!/bin/sh
case "$1" in
  --version) echo "eb-keep-logs 0.2.0"; exit 0;;
  -h|--help) echo "  eb-keep-logs rotate ROOT --compress-after DAYS [--delete-after DAYS] [--lock-timeout SECONDS]"; exit 0;;
esac
echo "$*" >> "$STUB_DIR/keeplogs.calls"
if [ "$1" = rotate ] && [ "${STUB_ROTATE_EXIT:-0}" = 75 ]; then
  echo "eb-keep-logs: another rotate holds the lock $2/.eb-keep-logs-rotate.lock; gave up after 0 s (--lock-timeout)" >&2
  exit 75
fi
exit 0
"#;

pub struct T {
    pub dir: tempfile::TempDir,
    pub site: PathBuf,
    pub stubs: PathBuf,
}

impl T {
    pub fn root(&self) -> &Path {
        self.dir.path()
    }

    pub fn run_dir(&self, stamp: &str) -> PathBuf {
        self.root().join("logs").join(stamp)
    }

    pub fn summary(&self, stamp: &str) -> Value {
        let p = self.run_dir(stamp).join("summary.json");
        let text = std::fs::read_to_string(&p).unwrap_or_else(|e| panic!("{}: {e}", p.display()));
        serde_json::from_str(&text).unwrap()
    }
}

pub fn write_exec(path: &Path, text: &str) {
    std::fs::write(path, text).unwrap();
    std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o755)).unwrap();
}

/// A site file whose tables end with `extra`.
pub fn setup(extra: &str) -> T {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path();
    let stubs = root.join("stubs");
    std::fs::create_dir(&stubs).unwrap();
    write_exec(&stubs.join("eb"), EB);
    write_exec(&stubs.join("eb-keep-logs"), KEEP_LOGS);
    let r = root.display();
    let s = stubs.display();
    let site = format!(
        r#"
schema_version = 1
kind = "site"
[site]
name = "t"
[site.tools.eb_keep_logs]
bin = "{s}/eb-keep-logs"
min_version = "0.2"
[scheduler]
kind = "local"
[[targets]]
name = "cpu"
[[targets]]
name = "gpu"
[filesystems]
scratch = "{r}/scratch"
tmp = "{r}/tmp"
build_path = "{r}/build"
install_root = "{r}/install"
log_root = "{r}/logs"
[modules]
init = []
[logs]
keep_failed = true
[logs.rotate]
compress_after_days = 3
{extra}
"#
    );
    let site_path = root.join("site.toml");
    std::fs::write(&site_path, site).unwrap();
    T {
        dir,
        site: site_path,
        stubs,
    }
}

/// `eb-pipeline ARGS`, with the stubs first on PATH.
pub fn eb_pipeline(t: &T, args: &[&str], vars: &[(&str, &str)]) -> Output {
    let path = format!(
        "{}:{}",
        t.stubs.display(),
        std::env::var("PATH").unwrap_or_default()
    );
    let mut c = Command::new(env!("CARGO_BIN_EXE_eb-pipeline"));
    c.args(args)
        .env("PATH", path)
        .env("STUB_DIR", t.root())
        .env_remove("EB_PIPELINE_LOG_STAMP")
        .env_remove("EB_PIPELINE_RUN_ID")
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    for (k, v) in vars {
        c.env(k, v);
    }
    c.output().unwrap()
}

/// `run` of `list` for `target` in the run directory `stamp`.
pub fn run(t: &T, target: &str, stamp: &str, list: &str, vars: &[(&str, &str)]) -> Output {
    let list_path = t.root().join(format!("list-{target}.txt"));
    std::fs::write(&list_path, list).unwrap();
    let site = t.site.display().to_string();
    let list_arg = list_path.display().to_string();
    let result = t.root().join(format!("result-{target}.json"));
    let result_arg = result.display().to_string();
    eb_pipeline(
        t,
        &[
            "run",
            "--site",
            &site,
            "--list",
            &list_arg,
            "--target",
            target,
            "--build-type",
            "production",
            "--log-stamp",
            stamp,
            "--result",
            &result_arg,
        ],
        vars,
    )
}

pub fn stderr(o: &Output) -> String {
    String::from_utf8_lossy(&o.stderr).into_owned()
}

pub fn stdout(o: &Output) -> String {
    String::from_utf8_lossy(&o.stdout).into_owned()
}

pub fn code(o: &Output) -> i32 {
    o.status.code().unwrap_or(-1)
}
