use eb_pipeline::{resolve, schema::Site, site};
use std::path::PathBuf;
use std::process::Command;

fn bin() -> Command {
    Command::new(env!("CARGO_BIN_EXE_eb-pipeline"))
}

fn example(name: &str) -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("examples")
        .join(name)
}

const MINIMAL: &str = r#"
schema_version = 1
kind = "site"
[site]
name = "t"
[scheduler]
kind = "slurm"
[phase_defaults]
build = "-p batch -c 4"
short = "-p batch -c 1"
[[targets]]
name = "cpu"
eb_optarch = "GCC:-march=a"
[targets.phases]
build = "-p big -c 64"
[targets.build_types.test]
eb_optarch = "GCC:-march=b"
[[targets]]
name = "other"
[filesystems]
scratch = "/scratch/x"
tmp = "/scratch/x/tmp"
build_path = "/scratch/x/build"
install_root = "/opt/eb"
log_root = "/opt/eb/logs"
[modules]
init = []
"#;

#[test]
fn examples_round_trip() {
    for f in ["small-cluster.toml", "eessi-site.toml"] {
        let text = std::fs::read_to_string(example(f)).unwrap();
        let a: Site = site::parse(&text).unwrap();
        let again = toml::to_string(&a).unwrap();
        let b: Site = site::parse(&again).unwrap();
        assert_eq!(a, b, "{f}");
        assert!(site::validate(&a, None).is_empty(), "{f}");
    }
}

#[test]
fn unknown_keys_are_rejected() {
    for (needle, repl) in [
        ("schema_version = 1", "schema_version = 1\nsurprise = 1"),
        ("[site]\nname = \"t\"", "[site]\nname = \"t\"\nsurprise = 1"),
        ("[modules]\ninit = []", "[modules]\ninit = []\nsurprise = 1"),
        ("name = \"other\"", "name = \"other\"\nsurprise = 1"),
    ] {
        let text = MINIMAL.replace(needle, repl);
        let err = site::parse(&text).unwrap_err();
        assert_eq!(err.code, 2);
        assert!(err.msg.contains("surprise"), "{}", err.msg);
    }
}

#[test]
fn missing_required_sections_are_rejected() {
    for cut in ["[scheduler]\nkind = \"slurm\"\n", "[modules]\ninit = []\n"] {
        let text = MINIMAL.replace(cut, "");
        assert!(site::parse(&text).is_err(), "{cut}");
    }
}

#[test]
fn defaults_and_overrides_resolve() {
    let s = site::parse(MINIMAL).unwrap();
    assert!(site::validate(&s, None).is_empty());
    let v = resolve::explain(&s, Some("cpu"), Some("test")).unwrap();
    let t = &v["targets"][0];
    // target override wins over the default
    assert_eq!(t["phases"]["build"]["args"], "-p big -c 64");
    // default applies where the target is silent
    assert_eq!(t["phases"]["short"]["args"], "-p batch -c 1");
    // per build type override wins over the target value
    assert_eq!(t["eb_optarch"], "GCC:-march=b");
    let v = resolve::explain(&s, Some("cpu"), Some("production")).unwrap();
    assert_eq!(v["targets"][0]["eb_optarch"], "GCC:-march=a");
    // launch line: launch, job name, role arguments
    let launch: Vec<String> = serde_json::from_value(
        resolve::explain(&s, Some("other"), Some("production")).unwrap()["targets"][0]["phases"]
            ["build"]["launch"]
            .clone(),
    )
    .unwrap();
    assert_eq!(
        launch,
        ["srun", "--job-name=t-production-build", "-p", "batch", "-c", "4"]
    );
    assert!(resolve::explain(&s, Some("nope"), None).is_err());
}

#[test]
fn target_names_must_be_unique() {
    let text = MINIMAL.replace("name = \"other\"", "name = \"cpu\"");
    let s = site::parse(&text).unwrap();
    assert!(site::validate(&s, None)
        .iter()
        .any(|e| e.contains("duplicate")));
}

#[test]
fn licence_gate_fails_closed() {
    let text = format!(
        "{MINIMAL}\n[licences]\nenforce = true\n[licences.required]\nsoftware = [\"Foo\"]\n"
    );
    let s = site::parse(&text).unwrap();
    let errs = site::validate(&s, None);
    assert!(errs.iter().any(|e| e.starts_with("licences")), "{errs:?}");
}

#[test]
fn lists_dir_is_checked() {
    let d = tempfile::tempdir().unwrap();
    let text = MINIMAL.replace("name = \"other\"", "name = \"other\"\nbuild_lists = [\"a.list\"]");
    let s = site::parse(&text).unwrap();
    assert_eq!(site::validate(&s, Some(d.path())).len(), 1);
    std::fs::write(d.path().join("a.list"), "x\n").unwrap();
    assert!(site::validate(&s, Some(d.path())).is_empty());
}

#[test]
fn plan_is_deterministic_and_sorted() {
    let d = tempfile::tempdir().unwrap();
    let list = d.path().join("l.txt");
    std::fs::write(&list, "Foo-1.0-foss-2025a.eb\nBar-2.0-foss-2025a.eb --rebuild\n").unwrap();
    let run = || {
        let o = bin()
            .args(["plan", "--site"])
            .arg(example("eessi-site.toml"))
            .arg("--list")
            .arg(&list)
            .args(["--target", "gpu-a", "--build-type", "production", "--run-id", "r1"])
            .output()
            .unwrap();
        assert_eq!(o.status.code(), Some(0));
        String::from_utf8(o.stdout).unwrap()
    };
    let (a, b) = (run(), run());
    assert_eq!(a, b);
    let v: serde_json::Value = serde_json::from_str(&a).unwrap();
    // serde_json maps are sorted; the text must already be in that order
    assert_eq!(serde_json::to_string_pretty(&v).unwrap() + "\n", a);
    let names: Vec<&str> = v["phases"]
        .as_array()
        .unwrap()
        .iter()
        .map(|p| p["name"].as_str().unwrap())
        .collect();
    assert_eq!(
        names,
        ["preflight", "licences", "shard", "sources", "admission", "build", "caches", "logs"]
    );
    // the GPU target's build phase uses its own allocation
    let build = &v["phases"][5]["commands"][0]["argv"];
    assert!(build.to_string().contains("--gpus-per-node=4"));
}

#[test]
fn schema_subcommand_lists_every_key() {
    let o = bin().arg("schema").output().unwrap();
    assert_eq!(o.status.code(), Some(0));
    let text = String::from_utf8(o.stdout).unwrap();
    for (_, fields) in eb_pipeline::schema::SECTIONS {
        for f in *fields {
            assert!(text.contains(&format!("  {} (", f.key)), "{}", f.key);
        }
    }
}

#[test]
fn check_exit_codes() {
    let o = bin()
        .args(["check", "--site"])
        .arg(example("small-cluster.toml"))
        .output()
        .unwrap();
    assert_eq!(o.status.code(), Some(0));
    assert!(o.stdout.is_empty(), "stdout is reserved");
    let d = tempfile::tempdir().unwrap();
    let bad = d.path().join("bad.toml");
    std::fs::write(&bad, MINIMAL.replace("kind = \"site\"", "kind = \"target\"")).unwrap();
    let o = bin().args(["check", "--site"]).arg(&bad).output().unwrap();
    assert_eq!(o.status.code(), Some(2));
    let o = bin().args(["check"]).output().unwrap();
    assert_eq!(o.status.code(), Some(2));
}

#[test]
fn explain_prints_toml_and_json() {
    for fmt in ["toml", "json"] {
        let o = bin()
            .args(["explain", "--site"])
            .arg(example("eessi-site.toml"))
            .args(["--target", "gpu-a", "--format", fmt])
            .output()
            .unwrap();
        assert_eq!(o.status.code(), Some(0));
        let t = String::from_utf8(o.stdout).unwrap();
        assert!(t.contains("EESSI_ACCELERATOR_TARGET_OVERRIDE"), "{fmt}");
    }
}
