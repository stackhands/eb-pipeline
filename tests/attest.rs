//! Input attestation is opt-in. With `[attest]` absent, `plan` matches the
//! output of the driver before the feature. With it enabled, `plan` and
//! `run` append a record through `eb-stack` and `rfc9162-sidecar`, and
//! `verify` recomputes that record.

use std::os::unix::fs::PermissionsExt;
use std::path::Path;
use std::process::{Command, Output};

const EB_STACK: &str = r#"#!/bin/sh
printf '%s\n' "$*" >> "$STUB_DIR/ebstack.calls"
case "$1" in
  --version) echo "eb-stack 0.7.4"; exit 0 ;;
esac
if [ "$1" = stack ] && [ "$2" = order ]; then
  out=""
  while [ $# -gt 0 ]; do
    case "$1" in
      --out) out=$2; shift 2 ;;
      *) shift ;;
    esac
  done
  printf '%s\n' "$STUB_EASYCONFIG" > "$out"
  exit 0
fi
if [ "$1" = recipe ] && [ "$2" = easyblock ]; then
  printf '%s\n' "${STUB_EASYBLOCK_CLASS:-ConfigureMake}"
  exit 0
fi
exit 0
"#;

const SIDECAR: &str = r#"#!/bin/sh
printf '%s\n' "$*" >> "$STUB_DIR/sidecar.calls"
cmd=$1
shift
dir="" path="" proof="" blob=""
while [ $# -gt 0 ]; do
  case "$1" in
    --dir) dir=$2; shift 2 ;;
    --path) path=$2; shift 2 ;;
    --proof) proof=$2; shift 2 ;;
    --blob-file) blob=$2; shift 2 ;;
    --key|--origin|--trusted) shift 2 ;;
    *) shift ;;
  esac
done
case "$cmd" in
  init|sync|snapshot) exit 0 ;;
  prove)
    hash=$(sha256sum "$dir/$path" | awk '{print $1}')
    entry=$(printf '{"blob":"%s","op":"put","path":"%s"}' "$hash" "$path")
    root=$(printf '\000%s' "$entry" | sha256sum | awk '{print $1}')
    printf '{"blob":"%s","checkpoint":"note","entry":"%s","inclusion":[],"index":0,"op":"put","origin":"example.org/inputs","path":"%s","root":"%s","size":1}\n' "$hash" "$(printf '%s' "$entry" | sed 's/"/\\"/g')" "$path" "$root"
    exit 0 ;;
  verify-proof)
    if grep -q '"op": "tampered"' "$proof"; then
      echo "proof rejected" >&2
      exit 1
    fi
    want=$(sed -n 's/.*"blob": *"\([0-9a-f]*\)".*/\1/p' "$proof" | head -1)
    got=$(sha256sum "$blob" | awk '{print $1}')
    if [ "$want" != "$got" ]; then
      echo "blob file sha256 $got does not match blob $want" >&2
      exit 1
    fi
    echo "verified proof inputs index 0 size 1 root $want by 1 trusted key(s)"
    exit 0 ;;
esac
echo "unknown $cmd" >&2
exit 2
"#;

const EB: &str = "#!/bin/sh\nexit 0\n";

fn write_exec(path: &Path, text: &str) {
    std::fs::write(path, text).unwrap();
    std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o755)).unwrap();
}

fn bin() -> Command {
    Command::new(env!("CARGO_BIN_EXE_eb-pipeline"))
}

struct Tree {
    root: tempfile::TempDir,
}

impl Tree {
    fn new() -> Self {
        let root = tempfile::tempdir().unwrap();
        let stubs = root.path().join("stubs");
        std::fs::create_dir(&stubs).unwrap();
        write_exec(&stubs.join("eb-stack"), EB_STACK);
        write_exec(&stubs.join("rfc9162-sidecar"), SIDECAR);
        write_exec(&stubs.join("eb"), EB);
        Self { root }
    }

    fn path(&self) -> &Path {
        self.root.path()
    }

    fn run(&self, args: &[&str], vars: &[(&str, &str)]) -> Output {
        let path = format!(
            "{}:{}",
            self.path().join("stubs").display(),
            std::env::var("PATH").unwrap_or_default()
        );
        let mut cmd = bin();
        cmd.args(args)
            .env("PATH", path)
            .env("STUB_DIR", self.path())
            .env_remove("EB_PIPELINE_SITE")
            .env_remove("EB_PIPELINE_LIST")
            .env_remove("EB_PIPELINE_TARGET")
            .env_remove("EB_PIPELINE_BUILD_TYPE")
            .env_remove("EB_PIPELINE_RUN_ID")
            .env_remove("EB_PIPELINE_LOG_STAMP")
            .env_remove("EB_IMAGE_DIGEST")
            .env_remove("EB_ROUTE_RECEIPT");
        for (k, v) in vars {
            cmd.env(k, v);
        }
        cmd.output().unwrap()
    }
}

fn bare_site(root: &Path) -> String {
    format!(
        "\
schema_version = 1
kind = \"site\"
[site]
name = \"t\"
[scheduler]
kind = \"local\"
[[targets]]
name = \"cpu\"
[filesystems]
scratch = \"{r}/scratch\"
tmp = \"{r}/tmp\"
build_path = \"{r}/build\"
install_root = \"{r}/install\"
log_root = \"{r}/logs\"
[modules]
init = []
",
        r = root.display()
    )
}

fn attest_site(root: &Path) -> String {
    format!(
        "\
{base}
[targets.route]
kind = \"command\"
command = \"true\"
receipt_digest_env = \"EB_ROUTE_RECEIPT\"

[attest]
enabled = true
log_dir = \"{r}/log\"
key = \"{r}/key.pem\"
origin = \"example.org/inputs\"
trusted = [\"example.org/inputs+abcd+AAAA\"]
bin = \"{r}/stubs/rfc9162-sidecar\"
image_digest_env = \"EB_IMAGE_DIGEST\"
easyconfigs = [\"{r}/easyconfigs\"]
easyblocks = [\"{r}/easyblocks\"]
patches = [\"{r}/patches\"]
",
        base = bare_site(root),
        r = root.display()
    )
}

fn plan(tree: &Tree, vars: &[(&str, &str)]) -> Output {
    let site = tree.path().join("site.toml");
    let list = tree.path().join("list.txt");
    tree.run(
        &[
            "plan",
            "--site",
            site.to_str().unwrap(),
            "--list",
            list.to_str().unwrap(),
            "--target",
            "cpu",
            "--build-type",
            "production",
            "--run-id",
            "R",
            "--log-stamp",
            "20260101_000000",
        ],
        vars,
    )
}

#[test]
fn a_plan_without_attest_matches_the_output_from_before_the_feature() {
    let tree = Tree::new();
    std::fs::write(tree.path().join("site.toml"), bare_site(tree.path())).unwrap();
    std::fs::write(tree.path().join("list.txt"), "Foo-1.0.eb\n").unwrap();
    let out = plan(&tree, &[]);
    assert_eq!(
        out.status.code(),
        Some(0),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    let text = String::from_utf8(out.stdout).unwrap();
    let got = text.replace(&tree.path().display().to_string(), "@ROOT@");
    let want = include_str!("fixtures/plan-without-attest.json");
    assert_eq!(got, want);

    let mut disabled = bare_site(tree.path());
    disabled.push_str("\n[attest]\nenabled = false\n");
    std::fs::write(tree.path().join("site.toml"), disabled).unwrap();
    let again = plan(&tree, &[]);
    assert_eq!(again.status.code(), Some(0));
    assert_eq!(String::from_utf8(again.stdout).unwrap(), text);
    assert!(!text.contains("input_attestation"));
}

#[test]
fn an_enabled_plan_appends_a_record_and_verify_recomputes_it() {
    let tree = Tree::new();
    let root = tree.path();
    std::fs::create_dir_all(root.join("easyconfigs")).unwrap();
    std::fs::create_dir_all(root.join("easyblocks")).unwrap();
    std::fs::create_dir_all(root.join("patches")).unwrap();
    let easyconfig = root.join("easyconfigs/Foo-1.0.eb");
    std::fs::write(
        &easyconfig,
        "name = 'Foo'\neasyblock = 'EB_Foo'\npatches = ['foo.patch']\n",
    )
    .unwrap();
    std::fs::write(root.join("patches/foo.patch"), "diff\n").unwrap();
    std::fs::write(
        root.join("easyblocks/eb_foo.py"),
        "class EB_Foo(EasyBlock):\n    pass\n",
    )
    .unwrap();
    std::fs::write(root.join("key.pem"), "not-a-key\n").unwrap();
    std::fs::write(root.join("site.toml"), attest_site(root)).unwrap();
    std::fs::write(root.join("list.txt"), "Foo-1.0.eb\n").unwrap();
    let ec = easyconfig.display().to_string();
    let out = plan(
        &tree,
        &[
            ("STUB_EASYCONFIG", ec.as_str()),
            ("EB_IMAGE_DIGEST", "sha256:abc"),
            ("EB_ROUTE_RECEIPT", "receipt-1"),
        ],
    );
    assert_eq!(
        out.status.code(),
        Some(0),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    let plan_json: serde_json::Value = serde_json::from_slice(&out.stdout).unwrap();
    let att = &plan_json["input_attestation"];
    assert!(att["record_sha256"].as_str().unwrap().len() == 64);
    assert_eq!(
        att["proof"]["blob"], att["record_sha256"],
        "the proof blob is the record"
    );
    let calls = std::fs::read_to_string(root.join("ebstack.calls")).unwrap();
    assert!(
        calls.contains("stack order --out ") && calls.contains("--easyconfigs"),
        "{calls}"
    );
    assert!(calls.contains("--root Foo==1.0"), "{calls}");
    assert!(!calls.contains("srun"), "{calls}");
    let side = std::fs::read_to_string(root.join("sidecar.calls")).unwrap();
    assert!(side.contains("snapshot"), "{side}");
    assert!(side.contains("prove"), "{side}");
    let record = std::fs::read_dir(root.join("log/inputs"))
        .unwrap()
        .next()
        .unwrap()
        .unwrap()
        .path();
    let body: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(&record).unwrap()).unwrap();
    assert_eq!(body["kind"], "eb-pipeline-input-attestation-v1");
    assert_eq!(body["image_digest"], "sha256:abc");
    assert_eq!(body["route_receipt"], "receipt-1");
    assert_eq!(body["easyblocks"][0]["class"], "EB_Foo");
    assert!(body["easyblocks"][0]["sha256"].is_string());
    assert_eq!(body["patches"][0]["name"], "foo.patch");
    assert_eq!(body["list"]["kind"], "list");

    let plan_path = root.join("plan.json");
    std::fs::write(&plan_path, &out.stdout).unwrap();
    let verified = tree.run(
        &[
            "verify",
            "--site",
            root.join("site.toml").to_str().unwrap(),
            "--list",
            root.join("list.txt").to_str().unwrap(),
            "--target",
            "cpu",
            "--build-type",
            "production",
            "--from",
            plan_path.to_str().unwrap(),
        ],
        &[
            ("STUB_EASYCONFIG", ec.as_str()),
            ("EB_IMAGE_DIGEST", "sha256:abc"),
            ("EB_ROUTE_RECEIPT", "receipt-1"),
        ],
    );
    assert_eq!(
        verified.status.code(),
        Some(0),
        "{}",
        String::from_utf8_lossy(&verified.stderr)
    );
    assert_eq!(verified.stderr, b"");
    let expected = format!(
        "verified input record {}\n",
        att["record_sha256"].as_str().unwrap()
    );
    assert_eq!(verified.stdout, expected.as_bytes());

    let recorded = tree.run(
        &[
            "verify",
            "--site",
            root.join("site.toml").to_str().unwrap(),
            "--recorded-list",
            "--target",
            "cpu",
            "--build-type",
            "production",
            "--from",
            plan_path.to_str().unwrap(),
        ],
        &[
            ("STUB_EASYCONFIG", ec.as_str()),
            ("EB_IMAGE_DIGEST", "sha256:abc"),
            ("EB_ROUTE_RECEIPT", "receipt-1"),
        ],
    );
    assert_eq!(recorded.status.code(), Some(0));
    assert_eq!(recorded.stdout, verified.stdout);
    assert_eq!(recorded.stderr, verified.stderr);

    let conflict = tree.run(
        &[
            "verify",
            "--site",
            root.join("site.toml").to_str().unwrap(),
            "--list",
            root.join("list.txt").to_str().unwrap(),
            "--recorded-list",
            "--target",
            "cpu",
            "--build-type",
            "production",
            "--from",
            plan_path.to_str().unwrap(),
        ],
        &[],
    );
    assert_eq!(conflict.status.code(), Some(2));
    assert!(conflict.stdout.is_empty());
    assert!(
        String::from_utf8_lossy(&conflict.stderr)
            .contains("the argument '--list <LIST>' cannot be used with '--recorded-list'"),
        "{}",
        String::from_utf8_lossy(&conflict.stderr)
    );

    let mut bad_inclusion = plan_json.clone();
    bad_inclusion["input_attestation"]["proof"]["root"] = serde_json::json!("0".repeat(64));
    let bad_inclusion_path = root.join("bad-inclusion.json");
    std::fs::write(
        &bad_inclusion_path,
        serde_json::to_string_pretty(&bad_inclusion).unwrap() + "\n",
    )
    .unwrap();
    let rejected_inclusion = tree.run(
        &[
            "verify",
            "--site",
            root.join("site.toml").to_str().unwrap(),
            "--list",
            root.join("list.txt").to_str().unwrap(),
            "--target",
            "cpu",
            "--build-type",
            "production",
            "--from",
            bad_inclusion_path.to_str().unwrap(),
        ],
        &[
            ("STUB_EASYCONFIG", ec.as_str()),
            ("EB_IMAGE_DIGEST", "sha256:abc"),
            ("EB_ROUTE_RECEIPT", "receipt-1"),
        ],
    );
    assert_eq!(rejected_inclusion.status.code(), Some(1));
    assert!(String::from_utf8_lossy(&rejected_inclusion.stderr)
        .contains("inclusion proof does not meet its checkpoint root"));

    std::fs::write(
        &easyconfig,
        "name = 'Foo'\neasyblock = 'EB_Foo'\npatches = ['foo.patch']\nchanged\n",
    )
    .unwrap();
    let mismatch = tree.run(
        &[
            "verify",
            "--site",
            root.join("site.toml").to_str().unwrap(),
            "--list",
            root.join("list.txt").to_str().unwrap(),
            "--target",
            "cpu",
            "--build-type",
            "production",
            "--from",
            plan_path.to_str().unwrap(),
        ],
        &[
            ("STUB_EASYCONFIG", ec.as_str()),
            ("EB_IMAGE_DIGEST", "sha256:abc"),
            ("EB_ROUTE_RECEIPT", "receipt-1"),
        ],
    );
    assert_eq!(mismatch.status.code(), Some(78), "{mismatch:?}");
    assert!(
        String::from_utf8_lossy(&mismatch.stderr).contains("does not match attestation"),
        "{}",
        String::from_utf8_lossy(&mismatch.stderr)
    );

    std::fs::write(
        &easyconfig,
        "name = 'Foo'\neasyblock = 'EB_Foo'\npatches = ['foo.patch']\n",
    )
    .unwrap();
    let mut tampered = plan_json.clone();
    tampered["input_attestation"]["proof"]["op"] = serde_json::json!("tampered");
    let bad = root.join("bad.json");
    std::fs::write(
        &bad,
        serde_json::to_string_pretty(&tampered).unwrap() + "\n",
    )
    .unwrap();
    let rejected = tree.run(
        &[
            "verify",
            "--site",
            root.join("site.toml").to_str().unwrap(),
            "--list",
            root.join("list.txt").to_str().unwrap(),
            "--target",
            "cpu",
            "--build-type",
            "production",
            "--from",
            bad.to_str().unwrap(),
        ],
        &[
            ("STUB_EASYCONFIG", ec.as_str()),
            ("EB_IMAGE_DIGEST", "sha256:abc"),
            ("EB_ROUTE_RECEIPT", "receipt-1"),
        ],
    );
    assert_eq!(rejected.status.code(), Some(1), "{rejected:?}");
    assert!(
        String::from_utf8_lossy(&rejected.stderr).contains("proof rejected"),
        "{}",
        String::from_utf8_lossy(&rejected.stderr)
    );
}

#[test]
fn attestation_helper_signals_are_reported_as_signal_exits() {
    let tree = Tree::new();
    let root = tree.path();
    std::fs::create_dir_all(root.join("easyconfigs")).unwrap();
    std::fs::create_dir_all(root.join("easyblocks")).unwrap();
    std::fs::create_dir_all(root.join("patches")).unwrap();
    let easyconfig = root.join("easyconfigs/Foo-1.0.eb");
    std::fs::write(&easyconfig, "name = 'Foo'\n").unwrap();
    std::fs::write(root.join("key.pem"), "not-a-key\n").unwrap();
    std::fs::write(root.join("site.toml"), attest_site(root)).unwrap();
    std::fs::write(root.join("list.txt"), "Foo-1.0.eb\n").unwrap();
    write_exec(
        &root.join("stubs/rfc9162-sidecar"),
        "#!/bin/sh\nkill -TERM $$\n",
    );

    let ec = easyconfig.display().to_string();
    let out = plan(
        &tree,
        &[
            ("STUB_EASYCONFIG", ec.as_str()),
            ("EB_IMAGE_DIGEST", "sha256:abc"),
            ("EB_ROUTE_RECEIPT", "receipt-1"),
        ],
    );
    assert_eq!(out.status.code(), Some(143), "{out:?}");
    assert!(
        String::from_utf8_lossy(&out.stderr).contains("exited 143"),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );

    write_exec(&root.join("stubs/rfc9162-sidecar"), SIDECAR);
    write_exec(&root.join("stubs/eb-stack"), "#!/bin/sh\nkill -TERM $$\n");
    let out = plan(
        &tree,
        &[
            ("STUB_EASYCONFIG", ec.as_str()),
            ("EB_IMAGE_DIGEST", "sha256:abc"),
            ("EB_ROUTE_RECEIPT", "receipt-1"),
        ],
    );
    assert_eq!(out.status.code(), Some(143), "{out:?}");
    assert!(
        String::from_utf8_lossy(&out.stderr).contains("exited 143"),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
}

#[test]
fn verify_without_the_feature_is_a_usage_error_and_a_missing_proof_is_refused() {
    let tree = Tree::new();
    std::fs::write(tree.path().join("site.toml"), bare_site(tree.path())).unwrap();
    std::fs::write(tree.path().join("list.txt"), "Foo-1.0.eb\n").unwrap();
    let planned = plan(&tree, &[]);
    assert_eq!(planned.status.code(), Some(0));
    let path = tree.path().join("plan.json");
    std::fs::write(&path, &planned.stdout).unwrap();
    let off = tree.run(
        &[
            "verify",
            "--site",
            tree.path().join("site.toml").to_str().unwrap(),
            "--list",
            tree.path().join("list.txt").to_str().unwrap(),
            "--target",
            "cpu",
            "--build-type",
            "production",
            "--from",
            path.to_str().unwrap(),
        ],
        &[],
    );
    assert_eq!(off.status.code(), Some(2), "{off:?}");

    let mut site = bare_site(tree.path());
    site.push_str(&format!(
        "\n[attest]\nenabled = true\nlog_dir = \"{r}/log\"\nkey = \"{r}/key.pem\"\norigin = \"example.org/inputs\"\ntrusted = [\"example.org/inputs+abcd+AAAA\"]\neasyconfigs = [\"{r}/easyconfigs\"]\n",
        r = tree.path().display()
    ));
    std::fs::create_dir_all(tree.path().join("easyconfigs")).unwrap();
    std::fs::write(tree.path().join("key.pem"), "k\n").unwrap();
    std::fs::write(tree.path().join("site.toml"), site).unwrap();
    let missing = tree.run(
        &[
            "verify",
            "--site",
            tree.path().join("site.toml").to_str().unwrap(),
            "--list",
            tree.path().join("list.txt").to_str().unwrap(),
            "--target",
            "cpu",
            "--build-type",
            "production",
            "--from",
            path.to_str().unwrap(),
        ],
        &[],
    );
    assert_eq!(missing.status.code(), Some(78), "{missing:?}");
    assert!(String::from_utf8_lossy(&missing.stderr).contains("has no input_attestation"));
}

#[test]
fn an_empty_image_digest_and_an_ambiguous_patch_are_refused() {
    let tree = Tree::new();
    let root = tree.path();
    std::fs::create_dir_all(root.join("easyconfigs")).unwrap();
    std::fs::create_dir_all(root.join("easyblocks")).unwrap();
    std::fs::create_dir_all(root.join("patches")).unwrap();
    let easyconfig = root.join("easyconfigs/Foo-1.0.eb");
    std::fs::write(&easyconfig, "easyblock = 'EB_Foo'\n").unwrap();
    std::fs::write(root.join("key.pem"), "k\n").unwrap();
    std::fs::write(root.join("site.toml"), attest_site(root)).unwrap();
    std::fs::write(root.join("list.txt"), "Foo-1.0.eb\n").unwrap();
    let ec = easyconfig.display().to_string();
    let empty = plan(&tree, &[("STUB_EASYCONFIG", ec.as_str())]);
    assert_eq!(empty.status.code(), Some(78), "{empty:?}");
    assert!(
        String::from_utf8_lossy(&empty.stderr).contains("image_digest_env"),
        "{}",
        String::from_utf8_lossy(&empty.stderr)
    );

    std::fs::write(
        &easyconfig,
        "easyblock = 'EB_Foo'\npatches = ['foo.patch']\n",
    )
    .unwrap();
    std::fs::write(root.join("easyconfigs/foo.patch"), "one\n").unwrap();
    std::fs::write(root.join("patches/foo.patch"), "two\n").unwrap();
    let ambiguous = plan(
        &tree,
        &[
            ("STUB_EASYCONFIG", ec.as_str()),
            ("EB_IMAGE_DIGEST", "sha256:abc"),
        ],
    );
    assert_eq!(ambiguous.status.code(), Some(78), "{ambiguous:?}");
    assert!(
        String::from_utf8_lossy(&ambiguous.stderr).contains("more than one file"),
        "{}",
        String::from_utf8_lossy(&ambiguous.stderr)
    );
}

#[test]
fn a_dry_run_does_not_append_and_a_real_run_stores_the_proof() {
    let tree = Tree::new();
    let root = tree.path();
    std::fs::create_dir_all(root.join("easyconfigs")).unwrap();
    std::fs::create_dir_all(root.join("easyblocks")).unwrap();
    std::fs::create_dir_all(root.join("patches")).unwrap();
    std::fs::write(
        root.join("easyconfigs/Foo-1.0.eb"),
        "name = 'Foo'\nversion = '1.0'\n",
    )
    .unwrap();
    std::fs::write(
        root.join("easyblocks/configuremake.py"),
        "class ConfigureMake(EasyBlock):\n    pass\n",
    )
    .unwrap();
    std::fs::write(root.join("key.pem"), "k\n").unwrap();
    std::fs::write(root.join("site.toml"), attest_site(root)).unwrap();
    std::fs::write(root.join("list.txt"), "Foo-1.0.eb\n").unwrap();
    let ec = root.join("easyconfigs/Foo-1.0.eb").display().to_string();
    let vars = [
        ("STUB_EASYCONFIG", ec.as_str()),
        ("EB_IMAGE_DIGEST", "sha256:abc"),
        ("EB_ROUTE_RECEIPT", "receipt-1"),
    ];
    let dry_plan = root.join("dry-plan.json");
    let dry = tree.run(
        &[
            "run",
            "--dry-run",
            "--plan-out",
            dry_plan.to_str().unwrap(),
            "--site",
            root.join("site.toml").to_str().unwrap(),
            "--list",
            root.join("list.txt").to_str().unwrap(),
            "--target",
            "cpu",
            "--build-type",
            "production",
            "--log-stamp",
            "20260101_000000",
        ],
        &vars,
    );
    assert_eq!(
        dry.status.code(),
        Some(0),
        "{}",
        String::from_utf8_lossy(&dry.stderr)
    );
    assert!(!root.join("sidecar.calls").exists());
    let dry_doc: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(dry_plan).unwrap()).unwrap();
    assert!(dry_doc.get("input_attestation").is_none());

    let result = root.join("result.json");
    let run_plan = root.join("run-plan.json");
    let ran = tree.run(
        &[
            "run",
            "--plan-out",
            run_plan.to_str().unwrap(),
            "--site",
            root.join("site.toml").to_str().unwrap(),
            "--list",
            root.join("list.txt").to_str().unwrap(),
            "--target",
            "cpu",
            "--build-type",
            "production",
            "--log-stamp",
            "20260101_000000",
            "--result",
            result.to_str().unwrap(),
        ],
        &vars,
    );
    assert_eq!(
        ran.status.code(),
        Some(0),
        "{}",
        String::from_utf8_lossy(&ran.stderr)
    );
    let doc: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(&result).unwrap()).unwrap();
    assert!(doc["input_attestation"]["record_sha256"].is_string());
    let plan_doc: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(run_plan).unwrap()).unwrap();
    assert_eq!(plan_doc["input_attestation"], doc["input_attestation"]);
    let record = std::fs::read_to_string(
        std::fs::read_dir(root.join("log/inputs"))
            .unwrap()
            .next()
            .unwrap()
            .unwrap()
            .path(),
    )
    .unwrap();
    let body: serde_json::Value = serde_json::from_str(&record).unwrap();
    assert_eq!(body["easyblocks"][0]["class"], "ConfigureMake");
    assert!(body["easyblocks"][0]["path"].is_string());
    let calls = std::fs::read_to_string(root.join("ebstack.calls")).unwrap();
    assert!(calls.contains("recipe easyblock Foo"), "{calls}");
}

#[test]
fn a_real_sidecar_proof_still_verifies_after_a_later_snapshot() {
    let Ok(sidecar) = std::env::var("RFC9162_SIDECAR") else {
        return;
    };
    let tree = Tree::new();
    let root = tree.path();
    let key = root.join("real.pem");
    let status = Command::new("openssl")
        .args(["genpkey", "-algorithm", "ED25519", "-out"])
        .arg(&key)
        .status()
        .unwrap();
    assert!(status.success());
    let line = Command::new("python3")
        .arg("-c")
        .arg(
            r#"
import base64, hashlib, subprocess, sys
der = subprocess.check_output(["openssl", "pkey", "-in", sys.argv[1], "-pubout", "-outform", "DER"])
pk = der[-32:]
name = sys.argv[2]
digest = hashlib.sha256(name.encode() + bytes([0x0A, 0x01]) + pk).digest()[:4]
print(f"{name}+{digest.hex()}+{base64.b64encode(bytes([0x01]) + pk).decode()}")
"#,
        )
        .arg(&key)
        .arg("example.org/inputs")
        .output()
        .unwrap();
    assert!(
        line.status.success(),
        "{}",
        String::from_utf8_lossy(&line.stderr)
    );
    let trusted = String::from_utf8(line.stdout).unwrap();
    let trusted = trusted.trim();
    std::fs::create_dir_all(root.join("easyconfigs")).unwrap();
    std::fs::create_dir_all(root.join("easyblocks")).unwrap();
    let easyconfig = root.join("easyconfigs/Foo-1.0.eb");
    std::fs::write(&easyconfig, "easyblock = 'EB_Foo'\n").unwrap();
    std::fs::write(
        root.join("easyblocks/eb_foo.py"),
        "class EB_Foo:\n    pass\n",
    )
    .unwrap();
    let site = format!(
        "\
{base}
[attest]
enabled = true
log_dir = \"{r}/log\"
key = \"{key}\"
origin = \"example.org/inputs\"
trusted = [\"{trusted}\"]
bin = \"{sidecar}\"
easyconfigs = [\"{r}/easyconfigs\"]
easyblocks = [\"{r}/easyblocks\"]
",
        base = bare_site(root),
        r = root.display(),
        key = key.display(),
        sidecar = sidecar,
    );
    std::fs::write(root.join("site.toml"), &site).unwrap();
    std::fs::write(root.join("list.txt"), "Foo-1.0.eb\n").unwrap();
    let ec = easyconfig.display().to_string();
    let out = plan(&tree, &[("STUB_EASYCONFIG", ec.as_str())]);
    assert_eq!(
        out.status.code(),
        Some(0),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    let plan_path = root.join("plan.json");
    std::fs::write(&plan_path, &out.stdout).unwrap();
    let again = plan(&tree, &[("STUB_EASYCONFIG", ec.as_str())]);
    assert_eq!(
        again.status.code(),
        Some(0),
        "{}",
        String::from_utf8_lossy(&again.stderr)
    );
    let verified = tree.run(
        &[
            "verify",
            "--site",
            root.join("site.toml").to_str().unwrap(),
            "--list",
            root.join("list.txt").to_str().unwrap(),
            "--target",
            "cpu",
            "--build-type",
            "production",
            "--from",
            plan_path.to_str().unwrap(),
        ],
        &[("STUB_EASYCONFIG", ec.as_str())],
    );
    assert_eq!(
        verified.status.code(),
        Some(0),
        "{}",
        String::from_utf8_lossy(&verified.stderr)
    );
    assert!(String::from_utf8_lossy(&verified.stdout).contains("verified input record "));
}
