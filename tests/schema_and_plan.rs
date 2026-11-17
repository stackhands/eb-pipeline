use eb_pipeline::{resolve, schema::Site, site};
use std::path::PathBuf;
use std::process::Command;

fn bin() -> Command {
    Command::new(env!("CARGO_BIN_EXE_eb-pipeline"))
}

fn example(name: &str) -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("templates")
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
        [
            "srun",
            "--job-name=t-production-build",
            "-p",
            "batch",
            "-c",
            "4"
        ]
    );
    assert!(resolve::explain(&s, Some("nope"), None).is_err());
}

#[test]
fn staged_targets_record_through_their_own_ledger_config() {
    // A ledger configuration records one `target`.
    let shared =
        format!("{MINIMAL}\n[rebuild]\nstaged = true\nledger_config = \"/etc/eb/ledger.json\"\n");
    let errs = site::validate(&site::parse(&shared).unwrap(), None);
    assert!(
        errs.iter().any(|e| e.starts_with("targets.other:")
            && e.contains("targets.cpu")
            && e.contains("/etc/eb/ledger.json")),
        "{errs:?}"
    );
    let own = shared.replace(
        "name = \"other\"",
        "name = \"other\"\nledger_config = \"/etc/eb/ledger-other.json\"",
    );
    let s = site::parse(&own).unwrap();
    assert!(site::validate(&s, None).is_empty());
    let v = resolve::explain(&s, None, None).unwrap();
    assert_eq!(v["targets"][0]["ledger_config"], "/etc/eb/ledger.json");
    assert_eq!(
        v["targets"][1]["ledger_config"],
        "/etc/eb/ledger-other.json"
    );
    // a target without staged rebuilds records nothing
    let unstaged = shared.replace("name = \"other\"", "name = \"other\"\nstaged = false");
    assert!(site::validate(&site::parse(&unstaged).unwrap(), None).is_empty());
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
    let text = MINIMAL.replace(
        "name = \"other\"",
        "name = \"other\"\nbuild_lists = [\"a.list\"]",
    );
    let s = site::parse(&text).unwrap();
    assert_eq!(site::validate(&s, Some(d.path())).len(), 1);
    std::fs::write(d.path().join("a.list"), "x\n").unwrap();
    assert!(site::validate(&s, Some(d.path())).is_empty());
}

#[test]
fn plan_is_deterministic_and_sorted() {
    let d = tempfile::tempdir().unwrap();
    let list = d.path().join("l.txt");
    std::fs::write(
        &list,
        "Foo-1.0-foss-2025a.eb\nBar-2.0-foss-2025a.eb --rebuild\n",
    )
    .unwrap();
    let run = || {
        let o = bin()
            .args(["plan", "--site"])
            .arg(example("eessi-site.toml"))
            .arg("--list")
            .arg(&list)
            .args([
                "--target",
                "gpu-a",
                "--build-type",
                "production",
                "--run-id",
                "r1",
            ])
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
        [
            "preflight",
            "licences",
            "shard",
            "sources",
            "admission",
            "build",
            "caches",
            "logs"
        ]
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
    std::fs::write(
        &bad,
        MINIMAL.replace("kind = \"site\"", "kind = \"target\""),
    )
    .unwrap();
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

#[test]
fn plan_shows_job_id_placeholder_and_default_footprint() {
    let d = tempfile::tempdir().unwrap();
    let list = d.path().join("l.txt");
    std::fs::write(&list, "Foo-1.0-foss-2025a.eb\n").unwrap();
    let o = bin()
        .args(["plan", "--site"])
        .arg(example("eessi-site.toml"))
        .arg("--list")
        .arg(&list)
        .args(["--target", "cpu-a", "--build-type", "production"])
        .output()
        .unwrap();
    assert_eq!(o.status.code(), Some(0));
    let v: serde_json::Value = serde_json::from_slice(&o.stdout).unwrap();
    let adm = v["phases"]
        .as_array()
        .unwrap()
        .iter()
        .find(|p| p["name"] == "admission")
        .unwrap()["commands"][0]["argv"]
        .to_string();
    assert!(adm.contains("${SLURM_JOB_ID}"), "{adm}");
    assert!(adm.contains("150000"), "{adm}");
    assert!(v["log_dir"]
        .as_str()
        .unwrap()
        .ends_with("/<log-stamp>/cpu-a"));
}

#[test]
fn check_warns_when_admission_has_no_live_jobs_cmd() {
    let d = tempfile::tempdir().unwrap();
    let path = d.path().join("site.toml");
    let text = std::fs::read_to_string(example("eessi-site.toml")).unwrap();
    let without = text.replace("live_jobs_cmd = \"/opt/eb-tools/bin/live-jobs\"\n", "");
    assert_ne!(without, text);
    for (site, warns) in [(without, true), (text, false)] {
        std::fs::write(&path, site).unwrap();
        let o = bin().args(["check", "--site"]).arg(&path).output().unwrap();
        let err = String::from_utf8(o.stderr).unwrap();
        assert_eq!(o.status.code(), Some(0), "{err}");
        assert_eq!(
            err.contains("admission.live_jobs_cmd is unset"),
            warns,
            "{err}"
        );
    }
}

#[test]
fn plan_expands_a_shard_plan() {
    let d = tempfile::tempdir().unwrap();
    let list = d.path().join("l.txt");
    std::fs::write(&list, "Foo-1.0-foss-2025a.eb\nBar-2.0-foss-2025a.eb\n").unwrap();
    let sp = d.path().join("shards");
    std::fs::create_dir(&sp).unwrap();
    std::fs::write(sp.join("prelude.list"), "Zed-1.0-foss-2025a.eb\n").unwrap();
    std::fs::write(sp.join("shard-01.list"), "Foo-1.0-foss-2025a.eb\n").unwrap();
    std::fs::write(sp.join("shard-02.list"), "Bar-2.0-foss-2025a.eb\n").unwrap();
    std::fs::write(
        sp.join("plan.json"),
        r#"{"prelude":{"file":"prelude.list","inode_footprint":7},
            "shards":[{"name":"shard-01","file":"shard-01.list","inode_footprint":111},
                      {"name":"shard-02","file":"shard-02.list","inode_footprint":222}]}"#,
    )
    .unwrap();
    let o = bin()
        .args(["plan", "--site"])
        .arg(example("eessi-site.toml"))
        .arg("--list")
        .arg(&list)
        .args([
            "--target",
            "cpu-a",
            "--build-type",
            "production",
            "--shard-plan",
        ])
        .arg(&sp)
        .output()
        .unwrap();
    assert_eq!(
        o.status.code(),
        Some(0),
        "{}",
        String::from_utf8_lossy(&o.stderr)
    );
    let v: serde_json::Value = serde_json::from_slice(&o.stdout).unwrap();
    let phase = |n: &str| {
        v["phases"]
            .as_array()
            .unwrap()
            .iter()
            .find(|p| p["name"] == n)
            .unwrap()
            .clone()
    };
    let adm = phase("admission")["commands"].as_array().unwrap().clone();
    assert_eq!(adm.len(), 3);
    assert!(adm[0]["argv"].to_string().contains("\"7\""));
    assert!(adm[2]["argv"].to_string().contains("\"222\""));
    let builds: Vec<String> = phase("build")["commands"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|c| c["name"].as_str().unwrap().starts_with("build:"))
        .map(|c| c["name"].as_str().unwrap().to_string())
        .collect();
    assert_eq!(
        builds,
        [
            "build:Zed-1.0-foss-2025a",
            "build:Foo-1.0-foss-2025a",
            "build:Bar-2.0-foss-2025a"
        ]
    );
}

#[test]
fn admission_needs_a_default_footprint() {
    let text = std::fs::read_to_string(example("eessi-site.toml"))
        .unwrap()
        .replace("default_footprint = 150000\n", "");
    let s = site::parse(&text).unwrap();
    assert!(site::validate(&s, None)
        .iter()
        .any(|e| e.contains("default_footprint")));
}

#[test]
fn rotate_needs_compress_after_days_and_a_later_delete() {
    // `eb-keep-logs rotate` refuses a call without --compress-after, and a
    // --delete-after that is not later.
    let site = |rotate: &str| format!("{MINIMAL}[logs]\n[logs.rotate]\n{rotate}");
    for missing in ["", "delete_after_days = 365\n"] {
        let err = site::parse(&site(missing)).unwrap_err();
        assert_eq!(err.code, 2);
        assert!(err.msg.contains("compress_after_days"), "{}", err.msg);
    }
    for delete in [30, 7] {
        let text = site(&format!(
            "compress_after_days = 30\ndelete_after_days = {delete}\n"
        ));
        let errs = site::validate(&site::parse(&text).unwrap(), None);
        assert!(
            errs.iter()
                .any(|e| e.starts_with("logs.rotate.delete_after_days:")),
            "{errs:?}"
        );
    }
    let d = tempfile::tempdir().unwrap();
    let path = d.path().join("site.toml");
    for bad in [
        "delete_after_days = 365\n",
        "compress_after_days = 30\ndelete_after_days = 30\n",
    ] {
        std::fs::write(&path, site(bad)).unwrap();
        let o = bin().args(["check", "--site"]).arg(&path).output().unwrap();
        assert_eq!(o.status.code(), Some(2), "{bad}");
    }

    let ok = site("compress_after_days = 30\ndelete_after_days = 365\n");
    assert!(site::validate(&site::parse(&ok).unwrap(), None).is_empty());
    let v = plan_of(&ok, "Foo-1.0-foss-2025a.eb\n", "other");
    let a = argv_of(&phase(&v, "logs")["commands"][0]);
    let i = a.iter().position(|x| x == "eb-keep-logs").unwrap();
    assert_eq!(
        a[i + 1..],
        [
            "rotate",
            "/opt/eb/logs",
            "--compress-after",
            "30",
            "--delete-after",
            "365",
            "--lock-timeout",
            "0"
        ]
    );
    let only_compress = site("compress_after_days = 30\n");
    assert!(site::validate(&site::parse(&only_compress).unwrap(), None).is_empty());
}

/// Plan `list` for `target` of a site file given as text.
fn plan_of(site_text: &str, list: &str, target: &str) -> serde_json::Value {
    let d = tempfile::tempdir().unwrap();
    let s = d.path().join("site.toml");
    let l = d.path().join("l.txt");
    std::fs::write(&s, site_text).unwrap();
    std::fs::write(&l, list).unwrap();
    let o = bin()
        .args(["plan", "--site"])
        .arg(&s)
        .arg("--list")
        .arg(&l)
        .args(["--target", target, "--build-type", "production"])
        .output()
        .unwrap();
    assert_eq!(
        o.status.code(),
        Some(0),
        "{}",
        String::from_utf8_lossy(&o.stderr)
    );
    serde_json::from_slice(&o.stdout).unwrap()
}

fn phase(v: &serde_json::Value, name: &str) -> serde_json::Value {
    v["phases"]
        .as_array()
        .unwrap()
        .iter()
        .find(|p| p["name"] == name)
        .unwrap()
        .clone()
}

fn argv_of(c: &serde_json::Value) -> Vec<String> {
    c["argv"]
        .as_array()
        .unwrap()
        .iter()
        .map(|a| a.as_str().unwrap().to_string())
        .collect()
}

fn with_target(extra: &str) -> String {
    MINIMAL.replace("name = \"other\"", &format!("name = \"other\"\n{extra}"))
}

#[test]
fn zero_one_site_files_plan_as_before() {
    // none of the 0.2.0 keys set: no selection, no route extras, single install path
    let v = plan_of(MINIMAL, "Foo-1.0-foss-2025a.eb --rebuild\n", "other");
    assert!(v.get("deselected").is_none());
    let b = &phase(&v, "build")["env"];
    assert_eq!(b["EASYBUILD_INSTALLPATH"], "/opt/eb");
    assert!(b.get("EASYBUILD_INSTALLPATH_SOFTWARE").is_none());
    let c = &phase(&v, "build")["commands"][0];
    assert!(argv_of(c).contains(&"--rebuild".to_string()));
    assert!(c.get("env").is_none());
}

#[test]
fn keep_failed_runs_with_the_basic_output_style() {
    // Without a terminal, `eb --last-log` in EasyBuild's default style wraps
    // the path at 80 columns, and `eb-keep-logs failed` reads that path.
    for scheduler_env in ["", "[scheduler.env]\nEASYBUILD_OUTPUT_STYLE = \"rich\"\n"] {
        let site = format!("{MINIMAL}{scheduler_env}[logs]\nkeep_failed = true\n");
        let v = plan_of(&site, "Foo-1.0-foss-2025a.eb\n", "other");
        let build = phase(&v, "build");
        let keep = &build["commands"][1];
        assert_eq!(keep["name"], "keep-failed:Foo-1.0-foss-2025a");
        assert_eq!(
            keep["env"]["EASYBUILD_OUTPUT_STYLE"], "basic",
            "{scheduler_env}"
        );
        assert!(build["commands"][0].get("env").is_none(), "{scheduler_env}");
    }
}

const SELECT_LIST: &str = "A-1.0.eb\nB-1.0.eb --only-targets=other\nC-1.0.eb --only-targets=x,other --rebuild\nD-1.0.eb --only-targets x\nE-1.0.eb --only-targets=x\n";

fn build_names(v: &serde_json::Value) -> Vec<String> {
    phase(v, "build")["commands"]
        .as_array()
        .unwrap()
        .iter()
        .map(|c| c["name"].as_str().unwrap().to_string())
        .filter(|n| n.starts_with("build:"))
        .collect()
}

#[test]
fn selection_in_plan() {
    let site = with_target(
        "[targets.select]\noption = \"--only-targets\"\nvalue = \"other\"\nwhen_absent = \"none\"\n",
    );
    let v = plan_of(&site, SELECT_LIST, "other");
    assert_eq!(build_names(&v), ["build:B-1.0", "build:C-1.0"]);
    let skipped: Vec<&str> = v["deselected"]
        .as_array()
        .unwrap()
        .iter()
        .map(|d| d["easyconfig"].as_str().unwrap())
        .collect();
    assert_eq!(skipped, ["A-1.0.eb", "D-1.0.eb", "E-1.0.eb"]);
    // the option stays on the line without strip
    let c = &phase(&v, "build")["commands"][0];
    assert!(argv_of(c).contains(&"--only-targets=other".to_string()));
}

#[test]
fn selection_when_absent_all_and_strip() {
    let site = with_target(
        "[targets.select]\noption = \"--only-targets\"\nvalue = \"other\"\nstrip = true\n",
    );
    let v = plan_of(&site, SELECT_LIST, "other");
    assert_eq!(
        build_names(&v),
        ["build:A-1.0", "build:B-1.0", "build:C-1.0"]
    );
    for c in phase(&v, "build")["commands"].as_array().unwrap() {
        assert!(!argv_of(c).iter().any(|a| a.contains("--only-targets")));
    }
    let c = &phase(&v, "build")["commands"][2];
    assert!(argv_of(c).contains(&"--rebuild".to_string()));
}

#[test]
fn selection_separator() {
    let site = with_target(
        "[targets.select]\noption = \"--only-targets\"\nvalue = \"other\"\nseparator = \"+\"\nwhen_absent = \"none\"\n",
    );
    let v = plan_of(
        &site,
        "A-1.0.eb --only-targets=x+other\nB-1.0.eb --only-targets=x,other\n",
        "other",
    );
    assert_eq!(build_names(&v), ["build:A-1.0"]);
}

#[test]
fn select_option_needs_its_leading_dashes() {
    for option in ["for", "-for"] {
        let frag = format!("[targets.select]\noption = \"{option}\"\nvalue = \"o\"\n");
        let s = site::parse(&with_target(&frag)).unwrap();
        let errs = site::validate(&s, None);
        assert!(
            errs.iter()
                .any(|e| e.contains("select.option") && e.contains("write `--for`")),
            "{option}: {errs:?}"
        );
    }
    let doc = eb_pipeline::schema::reference();
    let (_, select) = doc.split_once("[targets[].select]").unwrap();
    let (_, option) = select.split_once("option (string, required)").unwrap();
    let text = option.lines().nth(1).unwrap_or_default();
    assert!(text.contains("leading dashes"), "{text}");
}

#[test]
fn selection_is_validated() {
    for (frag, needle) in [
        ("option = \"only\"\nvalue = \"o\"", "select.option"),
        ("option = \"--x\"\nvalue = \"\"", "select.value"),
        (
            "option = \"--x\"\nvalue = \"o\"\nwhen_absent = \"some\"",
            "when_absent",
        ),
        (
            "option = \"--x\"\nvalue = \"o\"\nseparator = \"\"",
            "separator",
        ),
    ] {
        let s = site::parse(&with_target(&format!("[targets.select]\n{frag}\n"))).unwrap();
        let errs = site::validate(&s, None);
        assert!(errs.iter().any(|e| e.contains(needle)), "{frag}: {errs:?}");
    }
}

#[test]
fn plan_refuses_a_log_stamp_that_is_not_a_run_directory() {
    let d = tempfile::tempdir().unwrap();
    let s = d.path().join("site.toml");
    let l = d.path().join("l.txt");
    std::fs::write(&s, MINIMAL).unwrap();
    std::fs::write(&l, "Foo-1.0-foss-2025a.eb\n").unwrap();
    let plan = |stamp: &str, from_env: bool| {
        let mut c = bin();
        c.args(["plan", "--site"])
            .arg(&s)
            .arg("--list")
            .arg(&l)
            .args(["--target", "other", "--build-type", "production"]);
        if from_env {
            c.env("EB_PIPELINE_LOG_STAMP", stamp);
        } else {
            c.args(["--log-stamp", stamp]);
        }
        c.output().unwrap()
    };
    for from_env in [false, true] {
        for bad in [
            "2026-11-16",
            "20261116_0000",
            "../20261116_000000",
            "20261116_000000/x",
        ] {
            let o = plan(bad, from_env);
            assert_eq!(o.status.code(), Some(2), "{bad:?}");
            let err = String::from_utf8_lossy(&o.stderr);
            assert!(err.contains("--log-stamp"), "{bad:?}: {err}");
        }
        let o = plan("20261116_235959", from_env);
        assert_eq!(o.status.code(), Some(0));
        let v: serde_json::Value = serde_json::from_slice(&o.stdout).unwrap();
        assert_eq!(v["log_dir"], "/opt/eb/logs/20261116_235959/other");
    }
}

#[test]
fn plan_rotates_with_utc_ages() {
    // Run directories are named in UTC; eb-keep-logs counts a run's age from
    // the local midnight of the date in its name.
    let site = format!("{MINIMAL}[logs]\n[logs.rotate]\ncompress_after_days = 30\n");
    let v = plan_of(&site, "Foo-1.0-foss-2025a.eb\n", "other");
    let rotate = &phase(&v, "logs")["commands"][0];
    assert_eq!(rotate["name"], "rotate");
    assert_eq!(rotate["env"]["TZ"], "UTC");
}

#[test]
fn plan_rotates_without_waiting_for_another_rotate() {
    let site = format!("{MINIMAL}[logs]\n[logs.rotate]\ncompress_after_days = 30\n");
    let v = plan_of(&site, "Foo-1.0-foss-2025a.eb\n", "other");
    let rotate = &phase(&v, "logs")["commands"][0];
    let a = argv_of(rotate);
    assert_eq!(a[a.len() - 2..], ["--lock-timeout", "0"]);
    assert_eq!(
        rotate["note"],
        "without --lock-timeout 0 when `eb-keep-logs --help` does not list it, as in eb-keep-logs 0.2.1"
    );
}

const ROUTE: &str = r#"[targets.route]
kind = "command"
command = "/opt/r/route --quiet"
args = ["--root", "{runtime_root}", "--ro", "{shares_ro}", "--rw", "{shares_rw}", "--tag", "{target}-{phase}"]
runtime_root = "/opt/rt"
shares_ro = ["{work_dir}", "/srv/x"]
shares_rw = ["{phase_dir}"]
receipt_digest_env = "DIGEST"
[targets.route.env]
MARK = "{phase_dir}"
"#;

fn cpu_shard_site(route: &str) -> String {
    format!(
        "{}\n[shards]\ncount = 2\neasyconfigs = [\"/opt/ec\"]\n[sources]\n",
        with_target(route)
    )
}

#[test]
fn route_renders_args_env_and_separator() {
    let site =
        cpu_shard_site(ROUTE).replace("[phase_defaults]", "[phase_defaults]\ncpu = \"-p c\"");
    let d = tempfile::tempdir().unwrap();
    let sp = d.path().join("sp");
    std::fs::create_dir(&sp).unwrap();
    std::fs::write(sp.join("cpu.list"), "Pre-1.0.eb\n").unwrap();
    std::fs::write(sp.join("s.list"), "Foo-1.0.eb\n").unwrap();
    std::fs::write(
        sp.join("plan.json"),
        r#"{"cpu_prerequisites":{"file":"cpu.list"},"shards":[{"name":"shard-01","file":"s.list"}]}"#,
    )
    .unwrap();
    let s = d.path().join("site.toml");
    std::fs::write(&s, &site).unwrap();
    let l = d.path().join("l.txt");
    std::fs::write(&l, "Pre-1.0.eb\nFoo-1.0.eb\n").unwrap();
    let o = bin()
        .args(["plan", "--site"])
        .arg(&s)
        .arg("--list")
        .arg(&l)
        .args([
            "--target",
            "other",
            "--build-type",
            "production",
            "--shard-plan",
        ])
        .arg(&sp)
        .output()
        .unwrap();
    assert_eq!(
        o.status.code(),
        Some(0),
        "{}",
        String::from_utf8_lossy(&o.stderr)
    );
    let v: serde_json::Value = serde_json::from_slice(&o.stdout).unwrap();
    let cmds = phase(&v, "build")["commands"].as_array().unwrap().clone();
    let pre = cmds.iter().find(|c| c["name"] == "build:Pre-1.0").unwrap();
    let a = argv_of(pre);
    let log = v["log_dir"].as_str().unwrap();
    let sep = a.iter().position(|x| x == "--").unwrap();
    assert_eq!(
        a[..sep],
        [
            "/opt/r/route".to_string(),
            "--quiet".into(),
            "--root".into(),
            "/opt/rt".into(),
            "--ro".into(),
            "/scratch/x/build:/srv/x".into(),
            "--rw".into(),
            format!("{log}/build"),
            "--tag".into(),
            "other-build".into(),
        ]
    );
    // the phase command follows the separator
    assert_eq!(a[sep + 1], "srun");
    assert!(a.contains(&"eb".to_string()));
    assert_eq!(pre["env"]["MARK"], format!("{log}/build"));
    assert_eq!(pre["env"]["EB_PIPELINE_ROUTE_RUNTIME_ROOT"], "/opt/rt");
    assert_eq!(
        pre["env"]["EB_PIPELINE_ROUTE_SHARES_RO"],
        "/scratch/x/build:/srv/x"
    );
    assert_eq!(pre["env"]["EB_PIPELINE_ROUTE_RECEIPT_DIGEST"], "${DIGEST}");
    // a non CPU-role command is not routed
    let foo = cmds.iter().find(|c| c["name"] == "build:Foo-1.0").unwrap();
    assert_eq!(argv_of(foo)[0], "srun");
    assert!(foo.get("env").is_none());
}

#[test]
fn route_phases_select_what_is_routed() {
    let d = tempfile::tempdir().unwrap();
    let sp = d.path().join("sp");
    std::fs::create_dir(&sp).unwrap();
    std::fs::write(sp.join("cpu.list"), "Pre-1.0.eb\n").unwrap();
    std::fs::write(
        sp.join("plan.json"),
        r#"{"cpu_prerequisites":{"file":"cpu.list"},"shards":[]}"#,
    )
    .unwrap();
    let l = d.path().join("l.txt");
    std::fs::write(&l, "Pre-1.0.eb\n").unwrap();
    let routed = |phases: &str| {
        let route = format!(
            "[targets.route]\nkind = \"command\"\ncommand = \"/opt/r/route\"\nargs = [\"{{phase}}\"]\n{phases}"
        );
        let s = d.path().join("site.toml");
        std::fs::write(&s, cpu_shard_site(&route)).unwrap();
        let o = bin()
            .args(["plan", "--site"])
            .arg(&s)
            .arg("--list")
            .arg(&l)
            .args([
                "--target",
                "other",
                "--build-type",
                "production",
                "--shard-plan",
            ])
            .arg(&sp)
            .output()
            .unwrap();
        assert_eq!(
            o.status.code(),
            Some(0),
            "{}",
            String::from_utf8_lossy(&o.stderr)
        );
        let v: serde_json::Value = serde_json::from_slice(&o.stdout).unwrap();
        let first = |ph: &str| argv_of(&phase(&v, ph)["commands"][0])[0].clone();
        (first("sources"), first("build"))
    };
    // default: build only
    let (src, build) = routed("");
    assert_eq!((src.as_str(), build.as_str()), ("srun", "/opt/r/route"));
    let (src, build) = routed("phases = [\"build\", \"sources\"]\n");
    assert_eq!(
        (src.as_str(), build.as_str()),
        ("/opt/r/route", "/opt/r/route")
    );
    let (src, build) = routed("phases = [\"sources\"]\n");
    assert_eq!((src.as_str(), build.as_str()), ("/opt/r/route", "srun"));
}

#[test]
fn route_without_args_keeps_the_zero_one_argv() {
    let site = with_target("[targets.route]\nkind = \"command\"\ncommand = \"/opt/r/route\"\n");
    let s = site::parse(&site).unwrap();
    let launch = resolve::explain(&s, Some("other"), Some("production")).unwrap()["targets"][0]
        ["phases"]["cpu"]["launch"]
        .clone();
    assert_eq!(launch[0], "/opt/r/route");
    assert_eq!(launch[1], "srun");
    assert!(!launch.to_string().contains("\"--\""));
}

#[test]
fn route_is_validated() {
    for (frag, needle) in [
        ("kind = \"none\"\nargs = [\"x\"]", "need kind command"),
        (
            "kind = \"command\"\ncommand = \"r\"\nargs = [\"{nope}\"]",
            "unknown placeholder",
        ),
        (
            "kind = \"command\"\ncommand = \"r\"\nshares_ro = [\"{runtime_root}\"]",
            "unknown placeholder",
        ),
        (
            "kind = \"command\"\ncommand = \"r\"\nphases = [\"caches\"]",
            "route.phases",
        ),
        (
            "kind = \"command\"\ncommand = \"r\"\nreceipt_digest_env = \"a-b\"",
            "receipt_digest_env",
        ),
    ] {
        let s = site::parse(&with_target(&format!("[targets.route]\n{frag}\n"))).unwrap();
        let errs = site::validate(&s, None);
        assert!(errs.iter().any(|e| e.contains(needle)), "{frag}: {errs:?}");
    }
}

#[test]
fn cpu_hardware_and_fetch_profile() {
    let site = format!(
        "{}\n[sources]\n",
        with_target(
            "[targets.hardware]\nvendor_id = \"v\"\ncpu_family = 1\nmodel = 2\n[targets.cpu_hardware]\nvendor_id = \"w\"\ncpu_family = 3\nmodel = 4\n[targets.fetch_profile]\nretries = 3\nmirror = \"m\"\n"
        )
    );
    let v = plan_of(&site, "Foo-1.0.eb\n", "other");
    let fetch = &phase(&v, "sources")["commands"][0]["env"];
    assert_eq!(
        fetch["EB_PIPELINE_FETCH_PROFILE"],
        "{\"mirror\":\"m\",\"retries\":3}"
    );
    // a non CPU-role command exports the base class
    assert_eq!(fetch["EB_PIPELINE_HW_VENDOR_ID"], "v");
    assert_eq!(fetch["EB_PIPELINE_HW_MODEL"], "2");
    let s = site::parse(&site).unwrap();
    let e = resolve::explain(&s, Some("other"), None).unwrap();
    assert_eq!(e["targets"][0]["cpu_hardware"]["model"], 4);
    // without cpu_hardware nothing is exported
    let site = with_target("[targets.hardware]\nvendor_id = \"v\"\n");
    let v = plan_of(&site, "Foo-1.0.eb\n", "other");
    assert!(phase(&v, "build")["commands"][0].get("env").is_none());
}

#[test]
fn cpu_hardware_applies_to_cpu_role_units() {
    let site = with_target(
        "[targets.hardware]\nvendor_id = \"v\"\nmodel = 2\n[targets.cpu_hardware]\nvendor_id = \"w\"\nmodel = 4\n",
    ) + "\n[shards]\ncount = 2\neasyconfigs = [\"/opt/ec\"]\n";
    let d = tempfile::tempdir().unwrap();
    let sp = d.path().join("sp");
    std::fs::create_dir(&sp).unwrap();
    std::fs::write(sp.join("cpu.list"), "Pre-1.0.eb\n").unwrap();
    std::fs::write(sp.join("s.list"), "Foo-1.0.eb\n").unwrap();
    std::fs::write(
        sp.join("plan.json"),
        r#"{"cpu_prerequisites":{"file":"cpu.list"},"shards":[{"name":"shard-01","file":"s.list"}]}"#,
    )
    .unwrap();
    let s = d.path().join("site.toml");
    std::fs::write(&s, &site).unwrap();
    let l = d.path().join("l.txt");
    std::fs::write(&l, "Pre-1.0.eb\nFoo-1.0.eb\n").unwrap();
    let o = bin()
        .args(["plan", "--site"])
        .arg(&s)
        .arg("--list")
        .arg(&l)
        .args([
            "--target",
            "other",
            "--build-type",
            "production",
            "--shard-plan",
        ])
        .arg(&sp)
        .output()
        .unwrap();
    assert_eq!(
        o.status.code(),
        Some(0),
        "{}",
        String::from_utf8_lossy(&o.stderr)
    );
    let v: serde_json::Value = serde_json::from_slice(&o.stdout).unwrap();
    let cmds = phase(&v, "build")["commands"].as_array().unwrap().clone();
    let env_of = |n: &str| cmds.iter().find(|c| c["name"] == n).unwrap()["env"].clone();
    assert_eq!(env_of("build:Pre-1.0")["EB_PIPELINE_HW_MODEL"], "4");
    assert_eq!(env_of("build:Foo-1.0")["EB_PIPELINE_HW_MODEL"], "2");
}

#[test]
fn target_package_env_wins_per_key() {
    let site = with_target("[targets.package_env.Foo]\nB = \"target\"\nC = \"target\"\n").replace(
        "[modules]\ninit = []",
        "[modules]\ninit = []\n[modules.package_env.Foo]\nA = \"global\"\nB = \"global\"\n",
    );
    let v = plan_of(&site, "Foo-1.0-foss-2025a.eb\nBar-1.0.eb\n", "other");
    let cmds = phase(&v, "build")["commands"].as_array().unwrap().clone();
    let e = &cmds[0]["env"];
    assert_eq!(e["A"], "global");
    assert_eq!(e["B"], "target");
    assert_eq!(e["C"], "target");
    assert!(cmds[1].get("env").is_none());
    // another target keeps the global value only
    let v = plan_of(&site, "Foo-1.0-foss-2025a.eb\n", "cpu");
    let e = &phase(&v, "build")["commands"][0]["env"];
    assert_eq!(e["B"], "global");
    assert!(e.get("C").is_none());
}

#[test]
fn install_path_variables() {
    let v = plan_of(MINIMAL, "Foo-1.0.eb\n", "other");
    let e = &phase(&v, "build")["env"];
    assert_eq!(e["EASYBUILD_INSTALLPATH"], "/opt/eb");
    assert!(e.get("EASYBUILD_READ_ONLY_INSTALLDIR").is_none());

    let site = MINIMAL.replace(
        "install_root = \"/opt/eb\"",
        "install_root = \"/opt/eb\"\nexport_install_vars = \"split\"\nread_only_installdir = true\nread_only_installdir_value = \"True\"",
    );
    let v = plan_of(&site, "Foo-1.0.eb\n", "other");
    let e = &phase(&v, "build")["env"];
    assert!(e.get("EASYBUILD_INSTALLPATH").is_none());
    assert_eq!(e["EASYBUILD_INSTALLPATH_SOFTWARE"], "/opt/eb/software");
    assert_eq!(e["EASYBUILD_INSTALLPATH_MODULES"], "/opt/eb/modules");
    assert_eq!(e["EASYBUILD_READ_ONLY_INSTALLDIR"], "True");

    // the target's roots win in split mode
    let site = site.replace(
        "name = \"other\"",
        "name = \"other\"\nsoftware_root = \"/srv/sw\"\nmodules_root = \"/srv/mod\"",
    );
    let v = plan_of(&site, "Foo-1.0.eb\n", "other");
    let e = &phase(&v, "build")["env"];
    assert_eq!(e["EASYBUILD_INSTALLPATH_SOFTWARE"], "/srv/sw");
    assert_eq!(e["EASYBUILD_INSTALLPATH_MODULES"], "/srv/mod");

    // the default value of the read-only variable is 1
    let site = MINIMAL.replace(
        "install_root = \"/opt/eb\"",
        "install_root = \"/opt/eb\"\nread_only_installdir = true",
    );
    let v = plan_of(&site, "Foo-1.0.eb\n", "other");
    assert_eq!(
        phase(&v, "build")["env"]["EASYBUILD_READ_ONLY_INSTALLDIR"],
        "1"
    );

    let bad = MINIMAL.replace(
        "install_root = \"/opt/eb\"",
        "install_root = \"/opt/eb\"\nexport_install_vars = \"both\"",
    );
    let s = site::parse(&bad).unwrap();
    assert!(site::validate(&s, None)
        .iter()
        .any(|e| e.contains("export_install_vars")));
}

#[test]
fn cache_init_module_default_and_override() {
    let site = format!(
        "{MINIMAL}\n[caches]\ncommand = \"/opt/w\"\ninit_module = \"dflt\"\n[[caches.targets]]\ntarget = \"cpu\"\nconfig = \"/opt/c\"\n[[caches.targets]]\ntarget = \"other\"\nconfig = \"/opt/c2\"\ninit_module = \"mine\"\n"
    );
    let cache_module = |t: &str| {
        let v = plan_of(&site, "Foo-1.0.eb\n", t);
        phase(&v, "caches")["commands"][0]["env"]["EB_PIPELINE_CACHE_INIT_MODULE"].clone()
    };
    assert_eq!(cache_module("cpu"), "dflt");
    assert_eq!(cache_module("other"), "mine");
    // no default and no entry value: nothing exported
    let site = site.replace("init_module = \"dflt\"\n", "");
    let v = plan_of(&site, "Foo-1.0.eb\n", "cpu");
    assert!(phase(&v, "caches")["commands"][0]["env"]
        .get("EB_PIPELINE_CACHE_INIT_MODULE")
        .is_none());
}

fn fetch_argvs(v: &serde_json::Value) -> Vec<Vec<String>> {
    phase(v, "sources")["commands"]
        .as_array()
        .unwrap()
        .iter()
        .map(|c| {
            let a = argv_of(c);
            let i = a.iter().position(|x| x == "eb").unwrap();
            a[i..].to_vec()
        })
        .collect()
}

const ROBOT_LIST: &str = "A-1.eb --robot --rebuild\nB-1.eb --robot=/opt/ec\nC-1.eb --robot=/opt/ec --robot-paths=/opt/x:\nD-1.eb -r\n";

#[test]
fn prefetch_keeps_the_robot_by_default() {
    let site = format!("{MINIMAL}\n[sources]\nextra_args = [\"--x\"]\n");
    let v = plan_of(&site, ROBOT_LIST, "other");
    assert_eq!(
        fetch_argvs(&v),
        [
            vec!["eb", "A-1.eb", "--fetch", "--robot", "--robot", "--x"],
            vec![
                "eb",
                "B-1.eb",
                "--fetch",
                "--robot",
                "--robot=/opt/ec",
                "--x"
            ],
            vec![
                "eb",
                "C-1.eb",
                "--fetch",
                "--robot",
                "--robot=/opt/ec",
                "--robot-paths=/opt/x:",
                "--x"
            ],
            vec!["eb", "D-1.eb", "--fetch", "--robot", "-r", "--x"],
        ]
    );
}

#[test]
fn disable_robot_in_extra_args_fetches_own_sources_only() {
    let site = format!("{MINIMAL}\n[sources]\nextra_args = [\"--disable-robot\"]\n");
    let v = plan_of(&site, ROBOT_LIST, "other");
    assert_eq!(
        fetch_argvs(&v),
        [
            vec!["eb", "A-1.eb", "--fetch", "--disable-robot"],
            vec![
                "eb",
                "B-1.eb",
                "--fetch",
                "--robot-paths=/opt/ec:",
                "--disable-robot"
            ],
            vec![
                "eb",
                "C-1.eb",
                "--fetch",
                "--robot-paths=/opt/ec:/opt/x:",
                "--disable-robot"
            ],
            vec!["eb", "D-1.eb", "--fetch", "-r", "--disable-robot"],
        ]
    );
    // the build keeps the line's own options
    let b = argv_of(&phase(&v, "build")["commands"][1]);
    assert!(b.contains(&"--robot=/opt/ec".to_string()), "{b:?}");
}

#[test]
fn without_robot_keeps_the_last_robot_path() {
    use eb_pipeline::plan::without_robot;
    let s = |v: &[&str]| v.iter().map(|x| x.to_string()).collect::<Vec<_>>();
    assert_eq!(
        without_robot(s(&["--robot=/a", "--robot=/b", "-x"]).iter()),
        ["-x", "--robot-paths=/b:"]
    );
    assert_eq!(
        without_robot(s(&["--robot=/a", "--robot"]).iter()),
        Vec::<String>::new()
    );
    assert_eq!(
        without_robot(s(&["--robot-paths=:/x", "--robot=/a:/b"]).iter()),
        ["--robot-paths=/a:/b::/x"]
    );
}

#[test]
fn shard_command_runs_inside_the_module_wrapper() {
    let site = format!("{MINIMAL}\n[shards]\ncount = 2\neasyconfigs = [\"/opt/ec\"]\n")
        .replace("init = []", "init = [\"EESSI/2026.06\"]");
    let v = plan_of(&site, "Foo-1.0.eb\n", "other");
    let a = argv_of(&phase(&v, "shard")["commands"][0]);
    let i = a
        .iter()
        .position(|x| x == resolve::MODULE_SCRIPT)
        .unwrap_or_else(|| panic!("no module wrapper in {a:?}"));
    assert_eq!(a[i - 2..i], ["bash", "-c"]);
    assert_eq!(
        a[i + 1..i + 5],
        ["eb-pipeline", "eb-stack", "stack", "shard"]
    );
}

#[test]
fn plan_gives_each_staged_line_its_own_run_id() {
    let d = tempfile::tempdir().unwrap();
    std::fs::create_dir_all(d.path().join("all/Foo")).unwrap();
    std::fs::write(d.path().join("all/Foo/1.0.lua"), "").unwrap();
    let site = format!(
        "{}\n[rebuild]\nstaged = true\n",
        with_target(&format!("modules_root = \"{}\"", d.path().display()))
    );
    let v = plan_of(&site, "Foo-1.0.eb --rebuild\nFoo-1.0.eb -f\n", "other");
    let ids: Vec<String> = phase(&v, "build")["commands"]
        .as_array()
        .unwrap()
        .iter()
        .map(|c| {
            let a = argv_of(c);
            let i = a.iter().position(|x| x == "--run-id").unwrap();
            a[i + 1].clone()
        })
        .collect();
    assert_eq!(ids, ["<run-id>-Foo-1.0", "<run-id>-Foo-1.0.2"]);
}

#[test]
fn a_target_build_path_gives_each_target_its_own_build_directory() {
    let site = MINIMAL.replace("/scratch/x/build", "/scratch/x/build/{target}");
    assert!(site::validate(&site::parse(&site).unwrap(), None).is_empty());
    for t in ["cpu", "other"] {
        let v = plan_of(&site, "Foo-1.0.eb\n", t);
        assert_eq!(
            phase(&v, "build")["env"]["EASYBUILD_BUILDPATH"],
            format!("/scratch/x/build/{t}")
        );
        // Without the placeholder every target gets the same build path.
        let v = plan_of(MINIMAL, "Foo-1.0.eb\n", t);
        assert_eq!(
            phase(&v, "build")["env"]["EASYBUILD_BUILDPATH"],
            "/scratch/x/build"
        );
    }
}

#[test]
fn a_target_source_path_gives_each_target_its_own_download_directory() {
    let site = MINIMAL.replace(
        "build_path = \"/scratch/x/build\"",
        "build_path = \"/scratch/x/build\"\nsource_paths = [\"/scratch/x/sources/{target}\", \"/opt/eb/sources\"]",
    ) + "[sources]\n";
    assert!(site::validate(&site::parse(&site).unwrap(), None).is_empty());
    for t in ["cpu", "other"] {
        let v = plan_of(&site, "Foo-1.0.eb\n", t);
        for p in ["sources", "build"] {
            assert_eq!(
                phase(&v, p)["env"]["EASYBUILD_SOURCEPATH"],
                format!("/scratch/x/sources/{t}:/opt/eb/sources"),
                "{p}"
            );
        }
    }
}

#[test]
fn work_dir_is_the_target_build_path() {
    let site = with_target(
        "[targets.route]\nkind = \"command\"\ncommand = \"/opt/r/route\"\nargs = [\"--rw\", \"{work_dir}\"]\n",
    )
    .replace("/scratch/x/build", "/scratch/x/build/{target}");
    let s = site::parse(&site).unwrap();
    assert!(site::validate(&s, None).is_empty());
    let t = &resolve::explain(&s, Some("other"), None).unwrap()["targets"][0];
    let launch: Vec<String> = serde_json::from_value(t["phases"]["cpu"]["launch"].clone()).unwrap();
    assert_eq!(
        launch[..4],
        ["/opt/r/route", "--rw", "/scratch/x/build/other", "--"]
    );
    assert_eq!(t["env"]["EASYBUILD_BUILDPATH"], "/scratch/x/build/other");
}

#[test]
fn a_target_state_root_gives_each_target_its_own_state_root() {
    let d = tempfile::tempdir().unwrap();
    std::fs::create_dir_all(d.path().join("all/Foo")).unwrap();
    std::fs::write(d.path().join("all/Foo/1.0.lua"), "").unwrap();
    let site = format!(
        "{}\n[rebuild]\nstaged = true\nstate_root = \"/opt/eb/{{target}}/.eb-staged-rebuild\"\n",
        with_target(&format!(
            "site_prefix = \"/opt/eb/other\"\nmodules_root = \"{}\"",
            d.path().display()
        ))
    );
    let s = site::parse(&site).unwrap();
    assert!(site::validate(&s, None).is_empty());
    let v = plan_of(&site, "Foo-1.0.eb --rebuild\n", "other");
    let a = argv_of(&phase(&v, "build")["commands"][0]);
    let i = a.iter().position(|x| x == "--state-root").unwrap();
    assert_eq!(a[i + 1], "/opt/eb/other/.eb-staged-rebuild");
    let e = resolve::explain(&s, None, None).unwrap();
    assert_eq!(
        e["targets"][0]["state_root"],
        "/opt/eb/cpu/.eb-staged-rebuild"
    );
    assert_eq!(
        e["targets"][1]["state_root"],
        "/opt/eb/other/.eb-staged-rebuild"
    );
}

#[test]
fn a_staged_line_waits_a_bounded_time_for_the_state_root_lock() {
    let d = tempfile::tempdir().unwrap();
    std::fs::create_dir_all(d.path().join("all/Foo")).unwrap();
    std::fs::write(d.path().join("all/Foo/1.0.lua"), "").unwrap();
    let target = with_target(&format!("modules_root = \"{}\"", d.path().display()));
    for (key, wait) in [
        ("", "600"),
        ("lock_timeout = 45\n", "45"),
        ("lock_timeout = 0\n", "0"),
    ] {
        let site = format!("{target}\n[rebuild]\nstaged = true\n{key}");
        let v = plan_of(&site, "Foo-1.0.eb --rebuild\n", "other");
        let a = argv_of(&phase(&v, "build")["commands"][0]);
        let i = a.iter().position(|x| x == "--lock-timeout").unwrap();
        assert_eq!(a[i + 1], wait, "{key:?}");
        assert_eq!(a[i - 2], "--run-id", "{a:?}");
    }
    let negative = format!("{target}\n[rebuild]\nlock_timeout = -1\n");
    assert!(site::parse(&negative).is_err());
}

#[test]
fn per_target_paths_take_the_target_placeholder_only() {
    let errs = |text: String| site::validate(&site::parse(&text).unwrap(), None);
    let build_path = |p: &str| MINIMAL.replace("/scratch/x/build", p);
    let state_root = |p: &str| {
        format!(
            "{}\n[rebuild]\nstaged = true\nstate_root = \"{p}\"\n",
            with_target("site_prefix = \"/opt/eb/other\"")
        )
    };
    assert!(errs(build_path("/scratch/x/{target}/b-{target}")).is_empty());
    assert_eq!(
        errs(build_path("/scratch/x/{targets}")),
        ["filesystems.build_path: unknown placeholder {targets}; known: {target}"]
    );
    assert_eq!(
        errs(build_path("/scratch/x/{target")),
        ["filesystems.build_path: unbalanced `{`"]
    );
    let source_paths = |p: &str| {
        MINIMAL.replace(
            "build_path = \"/scratch/x/build\"",
            &format!(
                "build_path = \"/scratch/x/build\"\nsource_paths = [\"/opt/eb/sources\", \"{p}\"]"
            ),
        )
    };
    assert!(errs(source_paths("/scratch/x/sources/{target}")).is_empty());
    assert_eq!(
        errs(source_paths("/scratch/x/{site}")),
        ["filesystems.source_paths: unknown placeholder {site}; known: {target}"]
    );
    assert_eq!(
        errs(MINIMAL.replace("/scratch/x/tmp", "/scratch/x/tmp/{target}")),
        ["filesystems.tmp: {target} expands in filesystems.build_path, filesystems.source_paths and rebuild.state_root only"]
    );
    assert!(errs(state_root("/opt/eb/{target}/state")).is_empty());
    assert_eq!(
        errs(state_root("/opt/eb/{phase}/state")),
        ["rebuild.state_root: unknown placeholder {phase}; known: {target}"]
    );
    assert_eq!(
        errs(MINIMAL.replace(
            "kind = \"slurm\"",
            "kind = \"slurm\"\njob_name_template = \"{project}-{nope}\""
        )),
        ["scheduler.job_name_template: unknown placeholder {nope}; known: {project}, {build}, {phase}, {target}"]
    );
}

#[test]
fn a_target_state_root_cannot_split_one_live_tree() {
    // Both targets of MINIMAL install into /opt/eb.
    let site =
        format!("{MINIMAL}\n[rebuild]\nstaged = true\nstate_root = \"/opt/eb/{{target}}/state\"\n");
    let errs = site::validate(&site::parse(&site).unwrap(), None);
    assert_eq!(errs.len(), 1, "{errs:?}");
    assert!(
        errs[0].starts_with("targets.other: ")
            && errs[0].contains("/opt/eb/software")
            && errs[0].contains("targets.cpu"),
        "{errs:?}"
    );
    // One state root for both locks the tree once.
    let shared = site.replace("/opt/eb/{target}/state", "/opt/eb/state");
    assert!(site::validate(&site::parse(&shared).unwrap(), None).is_empty());
    for fix in [
        "site_prefix = \"/opt/eb/other\"",
        "software_root = \"/opt/eb/other/software\"",
        "staged = false",
    ] {
        let fixed = site.replace("name = \"other\"", &format!("name = \"other\"\n{fix}"));
        let errs = site::validate(&site::parse(&fixed).unwrap(), None);
        assert!(errs.is_empty(), "{fix}: {errs:?}");
    }
}

#[test]
fn a_target_name_is_a_directory_name() {
    for bad in [".", ".."] {
        let text = MINIMAL.replace("name = \"other\"", &format!("name = \"{bad}\""));
        let errs = site::validate(&site::parse(&text).unwrap(), None);
        assert!(
            errs.iter()
                .any(|e| e.starts_with(&format!("targets: invalid name `{bad}`;"))),
            "{errs:?}"
        );
    }
    let text = MINIMAL.replace("name = \"other\"", "name = \"a..b\"");
    assert!(site::validate(&site::parse(&text).unwrap(), None).is_empty());
}

#[test]
fn eessi_site_install_staged_targets_need_both_roots() {
    let errs = |text: &str| -> Vec<String> {
        site::validate(&site::parse(text).unwrap(), None)
            .into_iter()
            .filter(|e| e.contains("need software_root and modules_root"))
            .collect()
    };
    let site =
        format!("{MINIMAL}\n[modules.eessi]\nsite_install = true\n[rebuild]\nstaged = true\n");
    let e = errs(&site);
    assert_eq!(e.len(), 2, "{e:?}");
    assert!(e[0].starts_with("targets.cpu: "), "{e:?}");
    assert!(e[1].starts_with("targets.other: "), "{e:?}");

    let roots = "\nsoftware_root = \"/opt/s\"\nmodules_root = \"/opt/m\"";
    let fixed = site
        .replace("name = \"cpu\"", &format!("name = \"cpu\"{roots}"))
        .replace("name = \"other\"", "name = \"other\"\nstaged = false");
    assert!(errs(&fixed).is_empty(), "{:?}", errs(&fixed));
    let half = site.replace(
        "name = \"cpu\"",
        "name = \"cpu\"\nmodules_root = \"/opt/m\"",
    );
    assert_eq!(errs(&half).len(), 2);
    let split = site.replace(
        "install_root = \"/opt/eb\"",
        "install_root = \"/opt/eb\"\nexport_install_vars = \"split\"",
    );
    assert!(errs(&split).is_empty());
    assert!(errs(&site.replace("staged = true", "staged = false")).is_empty());
    assert!(errs(&site.replace("site_install = true", "site_install = false")).is_empty());
}

#[test]
fn explain_names_the_eessi_extend_install_path() {
    let site = format!("{MINIMAL}\n[modules.eessi]\nsite_install = true\n").replace(
        "name = \"other\"",
        "name = \"other\"\nsite_prefix = \"/opt/site/\"\ncpu_target = \"x86_64/amd/zen4\"\naccel_target = \"accel/nvidia/cc90\"",
    );
    let note = |text: &str| {
        let s = site::parse(text).unwrap();
        resolve::explain(&s, Some("other"), None).unwrap()["targets"][0]["install_note"].clone()
    };
    assert_eq!(
        note(&site),
        "EESSI-extend replaces EASYBUILD_INSTALLPATH when it loads: \
         /opt/site/versions/<release>/software/linux/x86_64/amd/zen4, \
         plus `/accel/nvidia/cc90` for an NVIDIA target when EESSI_ACCELERATOR_INSTALL is set"
    );
    let cpu_only = site.replace(
        "accel_target = \"accel/nvidia/cc90\"",
        "accel_target = \"\"",
    );
    assert!(note(&cpu_only)
        .as_str()
        .unwrap()
        .ends_with("/linux/x86_64/amd/zen4"));
    let split = site.replace(
        "install_root = \"/opt/eb\"",
        "install_root = \"/opt/eb\"\nexport_install_vars = \"split\"",
    );
    assert!(note(&split).is_null());
    assert!(note(&site.replace("site_install = true", "site_install = false")).is_null());
    assert!(note(MINIMAL).is_null());
}

#[test]
fn eessi_template_roots_sit_under_the_eessi_extend_path() {
    let text = std::fs::read_to_string(example("eessi-site.toml")).unwrap();
    let s = site::parse(&text).unwrap();
    let v = resolve::explain(&s, None, None).unwrap();
    for t in v["targets"].as_array().unwrap() {
        let base = format!(
            "{}/versions/<release>/software/linux/",
            t["site_prefix"].as_str().unwrap()
        );
        let note = t["install_note"].as_str().unwrap();
        assert!(note.contains(&format!("loads: {base}")), "{note}");
        let sw = t["software_root"].as_str().unwrap();
        let md = t["modules_root"].as_str().unwrap();
        assert!(sw.starts_with(&base) && sw.ends_with("/software"), "{sw}");
        assert!(md.starts_with(&base) && md.ends_with("/modules"), "{md}");
    }
}

#[test]
fn eessi_template_targets_have_a_build_path_and_a_state_root_each() {
    let text = std::fs::read_to_string(example("eessi-site.toml")).unwrap();
    let s = site::parse(&text).unwrap();
    let v = resolve::explain(&s, None, None).unwrap();
    let targets = v["targets"].as_array().unwrap();
    assert_eq!(targets.len(), 4);
    for t in targets {
        let name = t["name"].as_str().unwrap();
        assert_eq!(
            t["env"]["EASYBUILD_BUILDPATH"],
            format!("/scratch/<build-user>/build/{name}")
        );
        assert_eq!(t["site_prefix"], format!("/opt/eb/{name}"));
        assert_eq!(
            t["state_root"],
            format!("/opt/eb/{name}/.eb-staged-rebuild")
        );
    }
}

#[test]
fn plan_notes_a_rebuild_line_that_runs_in_place() {
    let site = format!("{MINIMAL}\n[rebuild]\nstaged = true\n");
    let v = plan_of(&site, "Foo-1.0.eb --rebuild\nBar-1.0.eb\n", "other");
    let cmds = phase(&v, "build")["commands"].clone();
    assert_eq!(argv_of(&cmds[0]).iter().filter(|a| *a == "eb").count(), 1);
    assert_eq!(
        cmds[0]["note"],
        "no module file /opt/eb/modules/all/Foo/1.0[.lua]; the rebuild runs as plain eb"
    );
    assert!(cmds[1].get("note").is_none());
    let off = plan_of(MINIMAL, "Foo-1.0.eb --rebuild\n", "other");
    assert!(phase(&off, "build")["commands"][0].get("note").is_none());
}

#[test]
fn installed_modules_dirs_from_env_plans_placeholders() {
    let site = format!(
        "{MINIMAL}\n[shards]\ncount = 2\neasyconfigs = [\"/opt/ec\"]\ninstalled_modules_dirs = [\"/opt/m\"]\ninstalled_modules_dirs_from_env = [\"MODS_A\", \"MODS_B\"]\n"
    );
    let v = plan_of(&site, "Foo-1.0.eb\n", "cpu");
    let a = argv_of(&phase(&v, "shard")["commands"][0]);
    let dirs: Vec<&String> = a
        .iter()
        .zip(a.iter().skip(1))
        .filter(|(f, _)| *f == "--installed-modules-dir")
        .map(|(_, d)| d)
        .collect();
    assert_eq!(dirs, ["/opt/m", "${MODS_A}", "${MODS_B}"]);
    let bad = site.replace("\"MODS_A\"", "\"1-x\"");
    let s = site::parse(&bad).unwrap();
    assert!(site::validate(&s, None)
        .iter()
        .any(|e| e.contains("installed_modules_dirs_from_env")));
}

#[test]
fn eessi_site_prefix_follows_the_install_prefix() {
    let site = MINIMAL.replace(
        "[modules]\ninit = []",
        "[modules]\ninit = []\n[modules.eessi]\nsite_install = true\n",
    );
    let v = plan_of(&site, "Foo-1.0.eb\n", "cpu");
    let env = &phase(&v, "build")["env"];
    assert_eq!(env["EASYBUILD_INSTALLPATH"], "/opt/eb");
    assert_eq!(env["EESSI_SITE_SOFTWARE_PREFIX"], "/opt/eb");
    assert_eq!(env["EESSI_SITE_INSTALL"], "1");

    let site = site.replace(
        "name = \"cpu\"",
        "name = \"cpu\"\nsite_prefix = \"/srv/site\"",
    );
    let v = plan_of(&site, "Foo-1.0.eb\n", "cpu");
    let env = &phase(&v, "build")["env"];
    assert_eq!(env["EASYBUILD_INSTALLPATH"], "/srv/site");
    assert_eq!(env["EESSI_SITE_SOFTWARE_PREFIX"], "/srv/site");

    // Without the EESSI table the variable is not exported.
    let v = plan_of(MINIMAL, "Foo-1.0.eb\n", "cpu");
    assert!(phase(&v, "build")["env"]
        .get("EESSI_SITE_SOFTWARE_PREFIX")
        .is_none());
}

#[test]
fn the_log_root_reference_names_the_stamp_and_the_target() {
    let text = eb_pipeline::schema::reference();
    assert!(text.contains("<log_root>/<log-stamp>/<target>/"), "{text}");
}

#[test]
fn the_eessi_template_has_one_default_target() {
    let text = std::fs::read_to_string(example("eessi-site.toml")).unwrap();
    let site = site::parse(&text).unwrap();
    let defaults: Vec<_> = site
        .targets
        .iter()
        .filter(|t| t.default)
        .map(|t| t.name.as_str())
        .collect();
    assert_eq!(defaults, ["cpu-a"]);
}

#[test]
fn eessi_example_exercises_the_target_keys() {
    let list = "<Name>-1.0.eb\nB-1.0.eb --only-targets=gpu-b\nC-1.0.eb --only-targets=gpu-a,gpu-b --rebuild\n";
    let text = std::fs::read_to_string(example("eessi-site.toml")).unwrap();
    let v = plan_of(&text, list, "gpu-a");
    assert_eq!(build_names(&v), ["build:<Name>-1.0", "build:C-1.0"]);
    assert_eq!(v["deselected"][0]["easyconfig"], "B-1.0.eb");
    let build = phase(&v, "build")["commands"][0].clone();
    assert!(!argv_of(&build).iter().any(|a| a.contains("--only-targets")));
    let src = phase(&v, "sources")["commands"][0].clone();
    assert_eq!(src["env"]["OPTARCH"], "<gpu-value>");
    assert!(src["env"]["EB_PIPELINE_FETCH_PROFILE"]
        .as_str()
        .unwrap()
        .contains("\"retries\":3"));
    assert_eq!(src["env"]["EB_PIPELINE_HW_MODEL"], "1");
    // other targets plan every line
    let v = plan_of(&text, list, "cpu-a");
    assert_eq!(v["lines"], 3);
    assert_eq!(build_names(&v).len(), 3);
}

#[test]
fn eessi_targets_take_eessi_subdirectory_forms() {
    let text = std::fs::read_to_string(example("eessi-site.toml")).unwrap();
    let errors = |t: &str| site::validate(&site::parse(t).unwrap(), None);
    for (from, to) in [
        ("x86_64/<vendor>/<uarch>", "aarch64/neoverse_v1"),
        ("x86_64/<vendor>/<uarch>", "riscv64/spacemit/x60"),
        ("accel/nvidia/cc90", "accel/amd/gfx90a"),
        ("accel/nvidia/cc90", ""),
    ] {
        let e = errors(&text.replacen(from, to, 1));
        assert!(e.is_empty(), "{to}: {e:?}");
    }
    for (from, to, key) in [
        ("x86_64/<vendor>/<uarch>", "<vendor>/<uarch>", "cpu_target"),
        ("x86_64/<vendor>/<uarch>", "x86_64", "cpu_target"),
        ("x86_64/<vendor>/<uarch>", "x86_64//zen4", "cpu_target"),
        (
            "x86_64/<vendor>/<uarch>",
            "x86_64/amd/zen4/extra",
            "cpu_target",
        ),
        ("accel/nvidia/cc90", "nvidia/cc90", "accel_target"),
        ("accel/nvidia/cc90", "accel/nvidia/9.0", "accel_target"),
        ("accel/nvidia/cc90", "accel/amd/gfx", "accel_target"),
    ] {
        let e = errors(&text.replacen(from, to, 1));
        assert!(
            e.iter()
                .any(|m| m.contains(key) && m.contains(&format!("`{to}`"))),
            "{to}: {e:?}"
        );
    }
    let plain = text
        .replacen("x86_64/<vendor>/<uarch>", "<vendor>/<uarch>", 1)
        .replacen("accel/nvidia/cc90", "<accel>/<model>", 1)
        .replace("[modules.eessi]\n", "[modules.eessi]\nenabled = false\n");
    assert_eq!(errors(&plain), Vec::<String>::new());
}

#[test]
fn check_warns_when_unuse_path_strands_the_first_init_module() {
    let d = tempfile::tempdir().unwrap();
    let path = d.path().join("site.toml");
    for (init, warns) in [
        ("[\"EasyBuild\"]", true),
        ("[\"/opt/modules/EESSI/2026.06.lua\", \"EasyBuild\"]", false),
    ] {
        let site = MINIMAL.replace("init = []", &format!("init = {init}\nunuse_path = true"));
        std::fs::write(&path, site).unwrap();
        let o = bin().args(["check", "--site"]).arg(&path).output().unwrap();
        let err = String::from_utf8(o.stderr).unwrap();
        assert_eq!(o.status.code(), Some(0), "{err}");
        assert_eq!(err.contains("modules.unuse_path"), warns, "{init}: {err}");
    }
    for f in ["small-cluster.toml", "eessi-site.toml"] {
        let o = bin()
            .args(["check", "--site"])
            .arg(example(f))
            .output()
            .unwrap();
        let err = String::from_utf8(o.stderr).unwrap();
        assert!(!err.contains("modules.unuse_path"), "{f}: {err}");
    }
}

#[test]
fn template_roles_start_one_task() {
    for f in ["small-cluster.toml", "eessi-site.toml"] {
        let v: toml::Value = toml::from_str(&std::fs::read_to_string(example(f)).unwrap()).unwrap();
        let mut tables = vec![&v["phase_defaults"]];
        for t in v["targets"].as_array().unwrap() {
            tables.extend(t.get("phases"));
        }
        for (role, args) in tables.iter().flat_map(|t| t.as_table().unwrap()) {
            let args = args.as_str().unwrap();
            assert!(args.contains("--ntasks-per-node=1"), "{f} {role}: {args}");
        }
    }
    let v: toml::Value =
        toml::from_str(&std::fs::read_to_string(example("small-cluster.toml")).unwrap()).unwrap();
    let mut roles: Vec<&String> = v["phase_defaults"].as_table().unwrap().keys().collect();
    roles.sort();
    assert_eq!(roles, ["build", "cache", "cpu", "plan", "short"]);
}

fn shard_list_arg(v: &serde_json::Value) -> String {
    let a = argv_of(&phase(v, "shard")["commands"][0]);
    let i = a.iter().position(|x| x == "--build-list").unwrap();
    a[i + 1].clone()
}

#[test]
fn plan_shards_the_selected_list_and_counts_it() {
    let site = format!(
        "{}\n[shards]\ncount = 2\neasyconfigs = [\"/opt/ec\"]\n",
        with_target(
            "[targets.select]\noption = \"--only-targets\"\nvalue = \"other\"\nwhen_absent = \"none\"\n"
        )
    );
    let v = plan_of(&site, SELECT_LIST, "other");
    let list = shard_list_arg(&v);
    assert!(list.ends_with("/selected-other.list"), "{list}");
    assert_eq!(v["selected_list"], list);
    assert_eq!(v["selected"], 2);
    assert_eq!(v["lines"], 5);
    assert_eq!(build_names(&v), ["build:B-1.0", "build:C-1.0"]);
}

#[test]
fn plan_without_selection_shards_the_list_as_given() {
    let site = format!("{MINIMAL}\n[shards]\ncount = 2\neasyconfigs = [\"/opt/ec\"]\n");
    let v = plan_of(&site, "Foo-1.0.eb\n", "cpu");
    assert!(shard_list_arg(&v).ends_with("/l.txt"));
    assert!(v.get("selected").is_none());
    assert!(v.get("selected_list").is_none());
}
