//! `run --resume-result` against stub `eb` and `eb-stack` programs.

use serde_json::Value;
use std::os::unix::fs::{MetadataExt, PermissionsExt};
use std::path::{Path, PathBuf};
use std::process::Command;

const EB_STACK: &str = r#"#!/bin/sh
D="${STUB_DIR:?}"
case "$1" in
  --version) echo "eb-stack 0.7.4"; exit 0;;
esac
[ "$1" = stack ] || exit 64
sub="$2"
shift 2
echo "$sub $*" >> "$D/eb-stack.calls"
case "$sub" in
  order)
    hashes=""
    while [ $# -gt 0 ]; do
      case "$1" in
        --hashes-out) hashes="$2"; shift 2;;
        *) shift;;
      esac
    done
    [ -n "$hashes" ] || { echo "missing --hashes-out" >&2; exit 2; }
    mkdir -p "$(dirname "$hashes")"
    if [ -n "${STUB_ORDER_EMPTY:-}" ]; then
      : > "$hashes"
    else
      printf '%s\n' "${STUB_ORDER_HASHES:-deadbeefdeadbeefdeadbeefdeadbeefdeadbeefdeadbeefdeadbeefdeadbeef  Foo/1.0-foss-2025a}" > "$hashes"
    fi
    echo "hashes=$hashes modules=1 incomplete=${STUB_ORDER_INCOMPLETE:-0}"
    echo "order=$hashes builds=1"
    exit "${STUB_ORDER_EXIT:-0}";;
  shard)
    case "${STUB_SHARD:-ok}" in unsharded) exit 3;; fail) exit 1;; esac
    out=""; bl=""
    while [ $# -gt 0 ]; do
      [ "$1" = --out-dir ] && out="$2"
      [ "$1" = --build-list ] && bl="$2"
      shift
    done
    mkdir -p "$out"
    if [ "${STUB_SHARD:-ok}" = fromlist ]; then
      : > "$out/shard-01.list"
      : > "$out/shard-02.list"
      i=0
      while IFS= read -r l; do
        i=$((i + 1))
        echo "$l" >> "$out/shard-0$(( (i - 1) % 2 + 1 )).list"
      done < "$bl"
      echo '{"prelude":{"file":null},"shards":[{"name":"shard-01","file":"shard-01.list","inode_footprint":111},{"name":"shard-02","file":"shard-02.list","inode_footprint":222}]}' > "$out/plan.json"
      exit 0
    fi
    exit 64;;
esac
exit 64
"#;

const EB: &str = r#"#!/bin/sh
D="${STUB_DIR:?}"
echo "$*" >> "$D/eb.calls"
case " $* " in
  *" --fetch "*) exit 0;;
esac
ec="$1"
stem=$(basename "$ec" .eb)
name=${stem%%-[0-9]*}
ver=${stem#"$name"-}
case ",${STUB_EB_FAIL:-}," in
  *",$stem,"*) echo "ERROR: $stem failed"; exit 1;;
esac
if [ -n "${EASYBUILD_INSTALLPATH:-}" ]; then
  dir="$EASYBUILD_INSTALLPATH/modules/all/$name"
  mkdir -p "$dir"
  printf 'help([[%s]])\n' "$stem" > "$dir/$ver.lua"
fi
exit 0
"#;

struct World {
    root: PathBuf,
    site: PathBuf,
    stubs: PathBuf,
    _keep: tempfile::TempDir,
}

fn world(extra: impl FnOnce(&Path) -> String) -> World {
    let keep = tempfile::tempdir().unwrap();
    let root = keep.path().to_path_buf();
    let extra = extra(&root);
    let stubs = root.join("stubs");
    std::fs::create_dir(&stubs).unwrap();
    for (name, text) in [("eb-stack", EB_STACK), ("eb", EB)] {
        let path = stubs.join(name);
        std::fs::write(&path, text).unwrap();
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755)).unwrap();
    }
    std::fs::create_dir(root.join("easyconfigs")).unwrap();
    let site = root.join("site.toml");
    std::fs::write(
        &site,
        format!(
            r#"
schema_version = 1
kind = "site"
[site]
name = "example"
[site.tools.eb_stack]
bin = "{stubs}/eb-stack"
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
{extra}
"#,
            stubs = stubs.display(),
            root = root.display(),
        ),
    )
    .unwrap();
    World {
        root,
        site,
        stubs,
        _keep: keep,
    }
}

fn resume(shards: u32) -> impl FnOnce(&Path) -> String {
    move |root| {
        format!(
            "\n[resume]\nenabled = true\neasyconfigs = [\"{}/easyconfigs\"]\nshards = {shards}\n",
            root.display()
        )
    }
}

struct Out {
    code: i32,
    stdout: String,
    stderr: String,
    result: Option<Value>,
}

fn invoke(w: &World, cmd: &str, list: Option<&str>, vars: &[(&str, &str)], extra: &[&str]) -> Out {
    let mut c = Command::new(env!("CARGO_BIN_EXE_eb-pipeline"));
    c.arg(cmd).arg("--site").arg(&w.site);
    if let Some(text) = list {
        let path = w.root.join("list.txt");
        std::fs::write(&path, text).unwrap();
        c.arg("--list")
            .arg(&path)
            .args(["--target", "cpu", "--build-type", "production"]);
    }
    c.args(extra);
    c.env(
        "PATH",
        format!(
            "{}:{}",
            w.stubs.display(),
            std::env::var("PATH").unwrap_or_default()
        ),
    )
    .env("STUB_DIR", &w.stubs);
    for (k, v) in vars {
        c.env(k, v);
    }
    let o = c.output().unwrap();
    let stderr = String::from_utf8_lossy(&o.stderr).into_owned();
    let stdout = String::from_utf8_lossy(&o.stdout).into_owned();
    let result = extra
        .iter()
        .position(|a| *a == "--result")
        .and_then(|i| extra.get(i + 1))
        .map(Path::new)
        .filter(|p| p.is_file())
        .and_then(|p| serde_json::from_str(&std::fs::read_to_string(p).unwrap()).ok());
    let parsed = if cmd == "plan" && o.status.success() {
        serde_json::from_str(&stdout).ok()
    } else {
        result
    };
    Out {
        code: o.status.code().unwrap_or(127),
        stdout,
        stderr,
        result: parsed,
    }
}

fn calls(w: &World, name: &str) -> String {
    std::fs::read_to_string(w.stubs.join(name)).unwrap_or_default()
}

fn clear_calls(w: &World) {
    for name in ["eb.calls", "eb-stack.calls"] {
        let _ = std::fs::remove_file(w.stubs.join(name));
    }
}

fn line<'a>(v: &'a Value, easyconfig: &str) -> &'a Value {
    v["lines"]
        .as_array()
        .unwrap()
        .iter()
        .find(|l| l["easyconfig"] == easyconfig)
        .unwrap_or_else(|| panic!("no line {easyconfig} in {v}"))
}

#[test]
fn dry_run_and_resume_result_are_refused_together() {
    let w = world(resume(1));
    let prior = w.root.join("prior.json");
    std::fs::write(&prior, "{}\n").unwrap();
    let out = invoke(
        &w,
        "run",
        Some("Foo-1.0-foss-2025a.eb\n"),
        &[],
        &[
            "--dry-run",
            "--resume-result",
            prior.to_str().unwrap(),
            "--run-id",
            "r1",
            "--log-stamp",
            "20260101_000000",
        ],
    );
    assert_eq!(out.code, 2);
    assert!(out.stdout.is_empty());
    assert!(
        out.stderr
            .contains("the argument '--dry-run' cannot be used with '--resume-result <FILE>'"),
        "{}",
        out.stderr
    );
    assert!(calls(&w, "eb.calls").is_empty());
    assert!(calls(&w, "eb-stack.calls").is_empty());
}

#[test]
fn dry_run_without_resume_keeps_its_output() {
    let w = world(resume(1));
    let out = invoke(
        &w,
        "run",
        Some("Foo-1.0-foss-2025a.eb\n"),
        &[],
        &[
            "--dry-run",
            "--run-id",
            "r1",
            "--log-stamp",
            "20260101_000000",
        ],
    );
    assert_eq!(out.code, 0);
    let root = w.root.display();
    let eb_stack = w.stubs.join("eb-stack");
    let eb_stack = eb_stack.display();
    let expected = format!(
        concat!(
            "# [licences] not configured\n",
            "[shard] {eb_stack} stack order --out {root}/logs/20260101_000000/cpu/input-order.txt --hashes-out {root}/logs/20260101_000000/cpu/input-hashes.txt --easyconfigs {root}/easyconfigs --root Foo==1.0\n",
            "# [sources] not configured\n",
            "# [admission] not configured\n",
            "[build] eb Foo-1.0-foss-2025a.eb\n",
            "# [checks] not configured\n",
            "# [caches] not configured\n",
            "# [logs] not configured\n",
        ),
        eb_stack = eb_stack,
        root = root,
    );
    assert_eq!(out.stdout.as_bytes(), expected.as_bytes());
    assert_eq!(
        out.stderr.as_bytes(),
        b"INFO [run] cpu: dry run: nothing executed\n"
    );
}

#[test]
fn a_default_run_records_no_identity() {
    let w = world(|_| String::new());
    let result = w.root.join("result.json");
    let o = invoke(
        &w,
        "run",
        Some("Foo-1.0-foss-2025a.eb\n"),
        &[],
        &[
            "--run-id",
            "r1",
            "--log-stamp",
            "20260101_000000",
            "--result",
            result.to_str().unwrap(),
        ],
    );
    assert_eq!(o.code, 0, "{}", o.stderr);
    let v = o.result.unwrap();
    assert!(v.get("input_digest").is_none(), "{v}");
    assert!(line(&v, "Foo-1.0-foss-2025a.eb").get("outputs").is_none());
    assert!(
        !calls(&w, "eb-stack.calls").contains("order"),
        "{}",
        calls(&w, "eb-stack.calls")
    );
    let plan = invoke(
        &w,
        "plan",
        Some("Foo-1.0-foss-2025a.eb\n"),
        &[],
        &["--run-id", "plan"],
    );
    assert_eq!(plan.code, 0, "{}", plan.stderr);
    let text = serde_json::to_string(&plan.result.unwrap()).unwrap();
    assert!(!text.contains("hashes-out"), "{text}");
    assert!(!text.contains("SLURM_RESTART_COUNT"), "{text}");
}

#[test]
fn resume_records_a_digest_and_skips_a_unit_whose_module_remains() {
    let w = world(resume(1));
    let list = "Foo-1.0-foss-2025a.eb\n";
    let first_path = w.root.join("first.json");
    let first = invoke(
        &w,
        "run",
        Some(list),
        &[],
        &[
            "--run-id",
            "r1",
            "--log-stamp",
            "20260101_000001",
            "--result",
            first_path.to_str().unwrap(),
        ],
    );
    assert_eq!(first.code, 0, "{}", first.stderr);
    let v = first.result.unwrap();
    let digest = v["input_digest"].as_str().unwrap();
    assert_eq!(digest.len(), 64, "{digest}");
    assert!(digest.chars().all(|c| c.is_ascii_hexdigit()));
    let outputs = line(&v, "Foo-1.0-foss-2025a.eb")["outputs"]
        .as_array()
        .unwrap();
    assert_eq!(outputs.len(), 1, "{outputs:?}");
    let module = PathBuf::from(outputs[0].as_str().unwrap());
    assert!(module.is_file(), "{}", module.display());
    let stack = calls(&w, "eb-stack.calls");
    assert!(stack.contains("order "), "{stack}");
    assert!(stack.contains("--hashes-out"), "{stack}");
    assert!(stack.contains("--easyconfigs"), "{stack}");
    assert!(stack.contains("--root Foo==1.0"), "{stack}");
    assert!(
        !stack.contains("\nshard ") && !stack.starts_with("shard "),
        "{stack}"
    );

    clear_calls(&w);
    let second_path = w.root.join("second.json");
    let second = invoke(
        &w,
        "run",
        Some(list),
        &[],
        &[
            "--run-id",
            "r2",
            "--log-stamp",
            "20260101_000002",
            "--resume-result",
            first_path.to_str().unwrap(),
            "--result",
            second_path.to_str().unwrap(),
        ],
    );
    assert_eq!(second.code, 0, "{}", second.stderr);
    assert!(second.stderr.contains("resumed"), "{}", second.stderr);
    let v = second.result.unwrap();
    assert_eq!(v["input_digest"], digest);
    assert_eq!(v["counts"]["skipped"], 1);
    let skipped = line(&v, "Foo-1.0-foss-2025a.eb");
    assert_eq!(skipped["status"], "skipped");
    assert!(skipped["note"]
        .as_str()
        .unwrap()
        .contains("outputs are still present"));
    assert!(
        calls(&w, "eb.calls").is_empty(),
        "{}",
        calls(&w, "eb.calls")
    );

    std::fs::remove_file(&module).unwrap();
    clear_calls(&w);
    let third_path = w.root.join("third.json");
    let third = invoke(
        &w,
        "run",
        Some(list),
        &[],
        &[
            "--run-id",
            "r3",
            "--log-stamp",
            "20260101_000003",
            "--resume-result",
            first_path.to_str().unwrap(),
            "--result",
            third_path.to_str().unwrap(),
        ],
    );
    assert_eq!(third.code, 0, "{}", third.stderr);
    assert_eq!(
        line(third.result.as_ref().unwrap(), "Foo-1.0-foss-2025a.eb")["status"],
        "success"
    );
    assert!(
        calls(&w, "eb.calls").contains("Foo-1.0-foss-2025a.eb"),
        "{}",
        calls(&w, "eb.calls")
    );
}

#[test]
fn result_may_atomically_replace_the_resume_result_after_it_is_read() {
    let w = world(resume(1));
    let list = "Foo-1.0-foss-2025a.eb\n";
    let shared = w.root.join("result.json");
    let first = invoke(
        &w,
        "run",
        Some(list),
        &[],
        &[
            "--run-id",
            "r1",
            "--log-stamp",
            "20260101_000004",
            "--result",
            shared.to_str().unwrap(),
        ],
    );
    assert_eq!(first.code, 0, "{}", first.stderr);
    let first_inode = std::fs::metadata(&shared).unwrap().ino();

    let second = invoke(
        &w,
        "run",
        Some(list),
        &[],
        &[
            "--run-id",
            "r2",
            "--log-stamp",
            "20260101_000005",
            "--resume-result",
            shared.to_str().unwrap(),
            "--result",
            shared.to_str().unwrap(),
        ],
    );
    assert_eq!(second.code, 0, "{}", second.stderr);
    assert!(second.stderr.contains("resumed"), "{}", second.stderr);
    let result = second.result.unwrap();
    assert_eq!(result["run_id"], "r2");
    assert_eq!(result["counts"]["skipped"], 1);
    let on_disk: Value = serde_json::from_str(&std::fs::read_to_string(&shared).unwrap()).unwrap();
    assert_eq!(on_disk, result);
    assert_ne!(std::fs::metadata(&shared).unwrap().ino(), first_inode);
}

#[test]
fn a_truncated_resume_result_is_rejected_without_replacing_it() {
    let w = world(resume(1));
    let shared = w.root.join("result.json");
    let truncated = b"{\"kind\":\"result\",\"input_digest\":\"abc\",\"lines\":[";
    std::fs::write(&shared, truncated).unwrap();

    let out = invoke(
        &w,
        "run",
        Some("Foo-1.0-foss-2025a.eb\n"),
        &[],
        &[
            "--resume-result",
            shared.to_str().unwrap(),
            "--result",
            shared.to_str().unwrap(),
        ],
    );

    assert_eq!(out.code, 2, "{}", out.stderr);
    assert!(out.stdout.is_empty());
    assert!(out.stderr.contains("EOF"), "{}", out.stderr);
    assert_eq!(std::fs::read(&shared).unwrap(), truncated);
    assert!(calls(&w, "eb.calls").is_empty());
    assert!(calls(&w, "eb-stack.calls").is_empty());
}

#[test]
fn a_changed_hash_file_refuses_the_resume() {
    let w = world(resume(1));
    let list = "Foo-1.0-foss-2025a.eb\n";
    let first_path = w.root.join("first.json");
    let first = invoke(
        &w,
        "run",
        Some(list),
        &[],
        &[
            "--run-id",
            "r1",
            "--log-stamp",
            "20260101_000011",
            "--result",
            first_path.to_str().unwrap(),
        ],
    );
    assert_eq!(first.code, 0, "{}", first.stderr);
    clear_calls(&w);
    let second_path = w.root.join("second.json");
    let second = invoke(
        &w,
        "run",
        Some(list),
        &[("STUB_ORDER_HASHES", "ffff  Foo/1.0-foss-2025a")],
        &[
            "--run-id",
            "r2",
            "--log-stamp",
            "20260101_000012",
            "--resume-result",
            first_path.to_str().unwrap(),
            "--result",
            second_path.to_str().unwrap(),
        ],
    );
    assert_eq!(second.code, 78, "{}", second.stderr);
    assert!(
        second.stderr.contains("input digest mismatch"),
        "{}",
        second.stderr
    );
    assert!(
        calls(&w, "eb.calls").is_empty(),
        "{}",
        calls(&w, "eb.calls")
    );
    let v = second.result.unwrap();
    assert!(v["message"]
        .as_str()
        .unwrap()
        .contains("input digest mismatch"));
    assert!(v["lines"].as_array().unwrap().is_empty(), "{v}");
}

#[test]
fn a_failed_shard_is_rebuilt_and_a_successful_shard_is_skipped() {
    let w = world(resume(2));
    let list = "Foo-1.0-foss-2025a.eb\nBar-2.0-foss-2025a.eb\n";
    let first_path = w.root.join("first.json");
    let first = invoke(
        &w,
        "run",
        Some(list),
        &[
            ("STUB_SHARD", "fromlist"),
            ("STUB_EB_FAIL", "Bar-2.0-foss-2025a"),
        ],
        &[
            "--run-id",
            "r1",
            "--log-stamp",
            "20260101_000021",
            "--result",
            first_path.to_str().unwrap(),
        ],
    );
    assert_eq!(first.code, 1, "{}", first.stderr);
    let v = first.result.unwrap();
    assert_eq!(line(&v, "Foo-1.0-foss-2025a.eb")["status"], "success");
    assert_eq!(line(&v, "Foo-1.0-foss-2025a.eb")["unit"], "shard-01");
    assert_eq!(line(&v, "Bar-2.0-foss-2025a.eb")["status"], "failed");
    assert!(
        calls(&w, "eb-stack.calls").contains("shard "),
        "{}",
        calls(&w, "eb-stack.calls")
    );
    clear_calls(&w);
    let second_path = w.root.join("second.json");
    let second = invoke(
        &w,
        "run",
        Some(list),
        &[("STUB_SHARD", "fromlist")],
        &[
            "--run-id",
            "r2",
            "--log-stamp",
            "20260101_000022",
            "--resume-result",
            first_path.to_str().unwrap(),
            "--result",
            second_path.to_str().unwrap(),
        ],
    );
    assert_eq!(second.code, 0, "{}", second.stderr);
    let v = second.result.unwrap();
    assert_eq!(line(&v, "Foo-1.0-foss-2025a.eb")["status"], "skipped");
    assert_eq!(line(&v, "Bar-2.0-foss-2025a.eb")["status"], "success");
    let eb = calls(&w, "eb.calls");
    assert!(!eb.contains("Foo-1.0-foss-2025a.eb"), "{eb}");
    assert!(eb.contains("Bar-2.0-foss-2025a.eb"), "{eb}");
}

#[test]
fn resume_result_without_the_table_is_usage_and_writes_no_result() {
    let w = world(|_| String::new());
    let prior = w.root.join("prior.json");
    std::fs::write(&prior, "{\"kind\":\"result\"}\n").unwrap();
    let result = w.root.join("result.json");
    let o = invoke(
        &w,
        "run",
        Some("Foo-1.0-foss-2025a.eb\n"),
        &[],
        &[
            "--resume-result",
            prior.to_str().unwrap(),
            "--result",
            result.to_str().unwrap(),
        ],
    );
    assert_eq!(o.code, 2, "{}", o.stderr);
    assert!(o.stderr.contains("[resume] enabled"), "{}", o.stderr);
    assert!(!result.exists());
}

#[test]
fn a_prior_result_without_a_digest_is_refused_and_recorded() {
    let w = world(resume(1));
    let prior = w.root.join("prior.json");
    std::fs::write(
        &prior,
        "{\"schema_version\":1,\"kind\":\"result\",\"lines\":[]}\n",
    )
    .unwrap();
    let result = w.root.join("result.json");
    let o = invoke(
        &w,
        "run",
        Some("Foo-1.0-foss-2025a.eb\n"),
        &[],
        &[
            "--run-id",
            "r1",
            "--log-stamp",
            "20260101_000031",
            "--resume-result",
            prior.to_str().unwrap(),
            "--result",
            result.to_str().unwrap(),
        ],
    );
    assert_eq!(o.code, 78, "{}", o.stderr);
    assert!(o.stderr.contains("input_digest"), "{}", o.stderr);
    let v = o.result.unwrap();
    assert!(v["message"].as_str().unwrap().contains("input_digest"));
    assert!(calls(&w, "eb-stack.calls").is_empty());
}

#[test]
fn an_incomplete_order_is_refused() {
    let w = world(resume(1));
    let result = w.root.join("result.json");
    let o = invoke(
        &w,
        "run",
        Some("Foo-1.0-foss-2025a.eb\n"),
        &[("STUB_ORDER_INCOMPLETE", "2")],
        &[
            "--run-id",
            "r1",
            "--log-stamp",
            "20260101_000041",
            "--result",
            result.to_str().unwrap(),
        ],
    );
    assert_eq!(o.code, 78, "{}", o.stderr);
    assert!(o.stderr.contains("incomplete"), "{}", o.stderr);
    assert!(
        calls(&w, "eb.calls").is_empty(),
        "{}",
        calls(&w, "eb.calls")
    );
}

#[test]
fn enabled_false_does_not_call_order() {
    let w = world(|_| "\n[resume]\nenabled = false\n".into());
    let result = w.root.join("result.json");
    let o = invoke(
        &w,
        "run",
        Some("Foo-1.0-foss-2025a.eb\n"),
        &[],
        &[
            "--run-id",
            "r1",
            "--log-stamp",
            "20260101_000051",
            "--result",
            result.to_str().unwrap(),
        ],
    );
    assert_eq!(o.code, 0, "{}", o.stderr);
    assert!(o.result.unwrap().get("input_digest").is_none());
    assert!(!calls(&w, "eb-stack.calls").contains("order"));
}

#[test]
fn resume_check_requires_absolute_trees_and_a_repository() {
    let w = world(|_| "\n[resume]\nenabled = true\neasyconfigs = [\"relative\"]\n".into());
    let o = invoke(&w, "check", None, &[], &[]);
    assert_eq!(o.code, 2, "{}", o.stderr);
    assert!(o.stderr.contains("not an absolute path"), "{}", o.stderr);

    let root = w.root.display().to_string();
    let slurm = format!(
        r#"
schema_version = 1
kind = "site"
[site]
name = "example"
[scheduler]
kind = "slurm"
[[targets]]
name = "cpu"
[filesystems]
scratch = "{root}/scratch"
tmp = "{root}/tmp/{{job_dir}}"
build_path = "{root}/build/{{job_dir}}"
install_root = "{root}/install"
log_root = "{root}/logs"
job_dir = "{root}/local/{{user}}.{{job}}"
[modules]
init = []
[resume]
enabled = true
easyconfigs = ["{root}/easyconfigs"]
"#
    );
    std::fs::write(&w.site, &slurm).unwrap();
    let o = invoke(&w, "check", None, &[], &[]);
    assert_eq!(o.code, 2, "{}", o.stderr);
    assert!(o.stderr.contains("configs_repo"), "{}", o.stderr);

    let slurm = slurm.replace(
        "log_root = ",
        &format!("configs_repo = \"{root}/easyconfigs\"\nlog_root = "),
    );
    std::fs::write(&w.site, &slurm).unwrap();
    let o = invoke(&w, "check", None, &[], &[]);
    assert_eq!(o.code, 0, "{}", o.stderr);

    let plan = invoke(
        &w,
        "plan",
        Some("Foo-1.0-foss-2025a.eb\n"),
        &[],
        &["--run-id", "plan"],
    );
    assert_eq!(plan.code, 0, "{}", plan.stderr);
    let text = serde_json::to_string(&plan.result.unwrap()).unwrap();
    assert!(text.contains("SLURM_RESTART_COUNT"), "{text}");
    let key = eb_pipeline::resume::workspace_key(&format!("{root}/easyconfigs"));
    assert!(text.contains(&key), "{text}");
}
