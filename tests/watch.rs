//! `watch` compares what runs recorded with the EESSI the module
//! environment loads now, and rebuilds only the installs a change reaches,
//! in the order eb-stack gives.

mod common;

use common::*;
use std::process::Output;

const WATCH: &str = "[watch]\neasyconfigs = [\"@ROOT@/robot\"]\n";

const CMAKE: &str = "CMake-3.27.6-GCCcore-13.2.0.eb";
const LIBEVENT: &str = "libevent-2.1.12-GCCcore-13.2.0.eb";
const UNZIP: &str = "UnZip-6.0-GCCcore-13.2.0.eb";

const LIST: &str = "Foo-1.0-GCCcore-13.2.0.eb\nBar-2.0-GCCcore-13.2.0.eb\nBaz-3.0-GCCcore-13.2.0.eb --try-amend='configopts=-DX=OFF -DY=OFF' --rebuild\nQux-4.0-GCCcore-13.2.0.eb\n";

const BAZ_REBUILD: &str =
    "Baz-3.0-GCCcore-13.2.0.eb '--try-amend=configopts=-DX=OFF -DY=OFF' --rebuild";

/// EESSI with CMake, libevent and UnZip. Foo is built with CMake and links
/// libevent; Bar links libevent; Baz links UnZip; Qux is built with Foo,
/// which eb-stack only shows when Foo's easyconfig is among the changed.
/// eb-stack orders Foo, then Qux, then Baz.
fn stack(t: &T) {
    t.eessi_module("CMake", "3.27.6-GCCcore-13.2.0");
    t.eessi_module("libevent", "2.1.12-GCCcore-13.2.0");
    t.eessi_module("UnZip", "6.0-GCCcore-13.2.0");
    t.deps(&[
        ("Foo/1.0-GCCcore-13.2.0", CMAKE, "review", "builddependency"),
        ("Foo/1.0-GCCcore-13.2.0", LIBEVENT, "no-rebuild", "runtime"),
        ("Bar/2.0-GCCcore-13.2.0", LIBEVENT, "no-rebuild", "runtime"),
        ("Baz/3.0-GCCcore-13.2.0", UNZIP, "no-rebuild", "runtime"),
        (
            "Qux/4.0-GCCcore-13.2.0",
            "Foo-1.0-GCCcore-13.2.0.eb",
            "review",
            "builddependency",
        ),
    ]);
    std::fs::write(
        t.root().join("order.txt"),
        "Foo-1.0-GCCcore-13.2.0.eb\nQux-4.0-GCCcore-13.2.0.eb\nBaz-3.0-GCCcore-13.2.0.eb\n",
    )
    .unwrap();
    std::fs::create_dir_all(t.root().join("robot")).unwrap();
}

fn with<'a>(vars: &'a [(&'static str, String)]) -> Vec<(&'static str, &'a str)> {
    vars.iter().map(|(k, v)| (*k, v.as_str())).collect()
}

/// A site whose four installs a run built and recorded on EESSI 2023.06.
fn built(extra: &str) -> T {
    let t = setup(&format!("{WATCH}{extra}"));
    stack(&t);
    let vars = eessi_vars(&t, "2023.06");
    let o = run(&t, "cpu", STAMP, LIST, &with(&vars));
    assert_eq!(code(&o), 0, "{}", stderr(&o));
    assert_eq!(
        t.watch_state("cpu")["installs"].as_object().unwrap().len(),
        4
    );
    t
}

/// CMake and libevent rebuilt in place in EESSI, and UnZip gone from it,
/// which eb-stack then plans in Baz's closure.
fn change(t: &T) {
    t.eessi_module("CMake", "3.27.6-GCCcore-13.2.0");
    t.eessi_module("libevent", "2.1.12-GCCcore-13.2.0");
    t.remove_eessi_module("UnZip", "6.0-GCCcore-13.2.0");
    std::fs::write(
        t.root().join("missing.txt"),
        "Baz-3.0-GCCcore-13.2.0.eb UnZip/6.0-GCCcore-13.2.0\n",
    )
    .unwrap();
}

fn watch(t: &T, stamp: &str, extra: &[&str], vars: &[(&str, &str)]) -> Output {
    let site = t.site.display().to_string();
    let mut args = vec![
        "watch",
        "--site",
        &site,
        "--target",
        "cpu",
        "--build-type",
        "production",
        "--log-stamp",
        stamp,
    ];
    args.extend(extra);
    eb_pipeline(t, &args, vars)
}

const LATER: &str = "20260102_000000";

const PLAN: &str = "\
EESSI 2023.06 x86_64/generic: 4 recorded installs, 3 to rebuild, 1 kept, 0 unaffected, 0 gone
rebuild, in this order:
   1. Foo/1.0-GCCcore-13.2.0    CMake/3.27.6-GCCcore-13.2.0 changed (review, builddependency); libevent/2.1.12-GCCcore-13.2.0 changed (no-rebuild, runtime)
   2. Qux/4.0-GCCcore-13.2.0    Foo/1.0-GCCcore-13.2.0 rebuilt (review, builddependency)
   3. UnZip/6.0-GCCcore-13.2.0  not installed: a dependency eb-stack plans for the rebuild
   4. Baz/3.0-GCCcore-13.2.0    UnZip/6.0-GCCcore-13.2.0 vanished (no-rebuild, runtime)
kept:
      Bar/2.0-GCCcore-13.2.0    libevent/2.1.12-GCCcore-13.2.0 changed (no-rebuild, runtime)
";

#[test]
fn without_an_eessi_change_watch_rebuilds_nothing() {
    let t = built("");
    let vars = eessi_vars(&t, "2023.06");
    let builds = t.calls("eb");
    let o = watch(&t, LATER, &["--dry-run"], &with(&vars));
    assert_eq!(code(&o), 0, "{}", stderr(&o));
    assert_eq!(
        stdout(&o),
        "EESSI 2023.06 x86_64/generic: 4 recorded installs, 0 to rebuild, 0 kept, 4 unaffected, 0 gone\nunaffected:\n      Bar/2.0-GCCcore-13.2.0\n      Baz/3.0-GCCcore-13.2.0\n      Foo/1.0-GCCcore-13.2.0\n      Qux/4.0-GCCcore-13.2.0\n"
    );
    let o = watch(&t, LATER, &[], &with(&vars));
    assert_eq!(code(&o), 0, "{}", stderr(&o));
    assert_eq!(stdout(&o), "", "only --dry-run prints on stdout");
    assert!(stderr(&o).contains(
        "INFO [watch] cpu: EESSI 2023.06 x86_64/generic: 4 recorded installs, 0 to rebuild"
    ));
    assert_eq!(t.calls("eb"), builds);
    assert!(
        !t.run_dir(LATER).exists(),
        "a watch that rebuilds nothing writes no run"
    );
}

#[test]
fn while_the_cvmfs_revision_is_the_recorded_one_watch_compares_no_module() {
    let t = setup(WATCH);
    stack(&t);
    let repo = t.root().join("eessi");
    set_revision(&repo, 41);
    let mut vars = eessi_vars(&t, "2023.06");
    vars.push(("EESSI_CVMFS_REPO", repo.display().to_string()));
    let o = run(&t, "cpu", STAMP, LIST, &with(&vars));
    assert_eq!(code(&o), 0, "{}", stderr(&o));
    assert_eq!(
        t.watch_state("cpu")["installs"]["Foo/1.0-GCCcore-13.2.0"]["revision"],
        41
    );

    t.eessi_module("CMake", "3.27.6-GCCcore-13.2.0");
    let calls = t.calls("ebstack");
    let plan = t.root().join("plan.json");
    let plan_arg = plan.display().to_string();
    let o = watch(
        &t,
        LATER,
        &["--dry-run", "--plan-out", &plan_arg],
        &with(&vars),
    );
    assert_eq!(code(&o), 0, "{}", stderr(&o));
    assert!(
        stdout(&o).starts_with("EESSI 2023.06 x86_64/generic at revision 41, as recorded: 4 recorded installs, 0 to rebuild, 0 kept, 4 unaffected, 0 gone\nunaffected:\n"),
        "{}",
        stdout(&o)
    );
    let p: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(&plan).unwrap()).unwrap();
    assert_eq!(p["outcome"], "unchanged");
    assert_eq!(
        t.calls("ebstack"),
        calls,
        "no eb-stack call while the revision holds"
    );

    set_revision(&repo, 42);
    let o = watch(&t, LATER, &["--dry-run"], &with(&vars));
    assert_eq!(code(&o), 0, "{}", stderr(&o));
    assert!(
        stdout(&o).starts_with("EESSI 2023.06 x86_64/generic at revision 42: 4 recorded installs, 2 to rebuild, 0 kept, 2 unaffected, 0 gone\n"),
        "{}",
        stdout(&o)
    );
}

#[test]
fn a_dry_run_prints_the_targeted_rebuild_in_the_order_eb_stack_gives() {
    let t = built("");
    let vars = eessi_vars(&t, "2023.06");
    let before =
        std::fs::read_to_string(t.root().join("install/.eb-pipeline-watch/cpu.json")).unwrap();
    let builds = t.calls("eb");
    change(&t);
    let plan = t.root().join("plan.json");
    let plan_arg = plan.display().to_string();
    let o = watch(
        &t,
        LATER,
        &["--dry-run", "--plan-out", &plan_arg],
        &with(&vars),
    );
    let err = stderr(&o);
    assert_eq!(code(&o), 0, "{err}");
    assert_eq!(stdout(&o), PLAN);
    assert!(
        err.contains("INFO [watch] cpu: dry run: nothing rebuilt; eb-stack's files are in "),
        "{err}"
    );

    let p: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(&plan).unwrap()).unwrap();
    assert_eq!(p["kind"], "watch-plan");
    assert_eq!(p["schema_version"], 1);
    assert_eq!(p["outcome"], "rebuild");
    assert_eq!(p["eessi"]["version"], "2023.06");
    assert_eq!(
        p["rebuild"],
        serde_json::json!([
            "Foo-1.0-GCCcore-13.2.0.eb --rebuild",
            "Qux-4.0-GCCcore-13.2.0.eb --rebuild",
            UNZIP,
            BAZ_REBUILD
        ])
    );
    let bar = p["installs"]
        .as_array()
        .unwrap()
        .iter()
        .find(|v| v["module"] == "Bar/2.0-GCCcore-13.2.0")
        .unwrap();
    assert_eq!(bar["action"], "keep");
    assert_eq!(bar["changes"][0]["change"], "changed");

    // eb-stack found Qux, and ordered the list over EasyBuild's tree, for
    // the vanished UnZip, under EESSI's and the target's.
    let calls = t.calls("ebstack");
    let cascade = calls
        .lines()
        .find(|l| l.contains("cascade.changed"))
        .unwrap();
    assert!(cascade.starts_with("rebuild-set --changed "), "{cascade}");
    let changed = cascade.split_whitespace().nth(2).unwrap();
    assert_eq!(
        std::fs::read_to_string(changed).unwrap(),
        "Baz-3.0-GCCcore-13.2.0.eb\nFoo-1.0-GCCcore-13.2.0.eb\n"
    );
    let shard = calls
        .lines()
        .find(|l| l.starts_with("stack shard"))
        .unwrap();
    let robot = format!(
        "--shards 1 --easyconfigs {}/robot --easyconfigs ",
        t.root().display()
    );
    assert!(shard.contains(&robot), "{shard}");

    assert_eq!(t.calls("eb"), builds, "a dry run builds nothing");
    let after =
        std::fs::read_to_string(t.root().join("install/.eb-pipeline-watch/cpu.json")).unwrap();
    assert_eq!(after, before, "a dry run changes no record");
}

#[test]
fn a_rebuild_line_leaves_a_parenthesis_and_a_star_bare() {
    let t = setup(WATCH);
    stack(&t);
    let vars = eessi_vars(&t, "2023.06");
    let list = "\
Foo-1.0-GCCcore-13.2.0.eb --filter-deps=foo(bar) --opt=a*b --comment=\"it's\"
Bar-2.0-GCCcore-13.2.0.eb
Baz-3.0-GCCcore-13.2.0.eb
Qux-4.0-GCCcore-13.2.0.eb
";
    let o = run(&t, "cpu", STAMP, list, &with(&vars));
    assert_eq!(code(&o), 0, "{}", stderr(&o));
    assert_eq!(
        t.watch_state("cpu")["installs"]["Foo/1.0-GCCcore-13.2.0"]["args"],
        serde_json::json!(["--filter-deps=foo(bar)", "--opt=a*b", "--comment=it's"])
    );

    t.eessi_module("CMake", "3.27.6-GCCcore-13.2.0");
    let plan = t.root().join("paren-plan.json");
    let plan_arg = plan.display().to_string();
    let o = watch(
        &t,
        LATER,
        &["--dry-run", "--plan-out", &plan_arg],
        &with(&vars),
    );
    assert_eq!(code(&o), 0, "{}", stderr(&o));
    let p: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(&plan).unwrap()).unwrap();
    let foo = p["rebuild"]
        .as_array()
        .unwrap()
        .iter()
        .find(|l| l.as_str().unwrap().starts_with("Foo-"))
        .unwrap()
        .as_str()
        .unwrap();
    assert_eq!(
        foo,
        "Foo-1.0-GCCcore-13.2.0.eb --filter-deps=foo(bar) --opt=a*b \"--comment=it's\" --rebuild"
    );
}

#[test]
fn watch_rebuilds_only_the_installs_the_change_reaches_and_records_them_again() {
    let t = built("");
    let vars = eessi_vars(&t, "2023.06");
    let builds = t.calls("eb");
    change(&t);
    let o = watch(&t, LATER, &[], &with(&vars));
    let err = stderr(&o);
    assert_eq!(code(&o), 0, "{err}");
    assert_eq!(stdout(&o), "");
    for line in PLAN.lines() {
        assert!(
            err.contains(&format!("INFO [watch] cpu: {line}\n")),
            "{line} missing from {err}"
        );
    }
    let calls = t.calls("eb");
    let rebuilt: Vec<&str> = calls[builds.len()..]
        .lines()
        .map(|l| l.split(" --tmp-logdir=").next().unwrap())
        .collect();
    assert_eq!(
        rebuilt,
        [
            "Foo-1.0-GCCcore-13.2.0.eb --rebuild",
            "Qux-4.0-GCCcore-13.2.0.eb --rebuild",
            UNZIP,
            "Baz-3.0-GCCcore-13.2.0.eb --try-amend=configopts=-DX=OFF -DY=OFF --rebuild"
        ]
    );
    assert!(
        err.contains("INFO [watch] cpu: recorded 4 against EESSI 2023.06 x86_64/generic: Foo/1.0-GCCcore-13.2.0, Qux/4.0-GCCcore-13.2.0, UnZip/6.0-GCCcore-13.2.0, Baz/3.0-GCCcore-13.2.0"),
        "{err}"
    );

    // The rebuild is a run like any other, with the plan beside it.
    let log_dir = t.run_dir(LATER).join("cpu");
    assert_eq!(
        std::fs::read_to_string(log_dir.join("watch.list")).unwrap(),
        format!("Foo-1.0-GCCcore-13.2.0.eb --rebuild\nQux-4.0-GCCcore-13.2.0.eb --rebuild\n{UNZIP}\n{BAZ_REBUILD}\n")
    );
    assert!(log_dir.join("watch-plan.json").is_file());
    let s = t.summary(LATER);
    let entry = &s["targets"][0];
    assert_eq!(entry["command"], "watch");
    assert_eq!(entry["status"], "success");
    assert_eq!(
        entry["list"],
        log_dir.join("watch.list").display().to_string()
    );
    assert_eq!(entry["watch"]["outcome"], "rebuild");
    assert_eq!(entry["packages"].as_array().unwrap().len(), 4);
    let page = std::fs::read_to_string(t.run_dir(LATER).join("summary.html")).unwrap();
    assert!(page.contains("<details><summary>The plan of <code>watch</code></summary>"));

    let state = t.watch_state("cpu");
    let installs = &state["installs"];
    assert_eq!(
        installs["UnZip/6.0-GCCcore-13.2.0"]["how"], "built",
        "the dependency eb-stack added is now a local install to watch"
    );
    assert_eq!(installs["Foo/1.0-GCCcore-13.2.0"]["run"], LATER);
    assert_eq!(
        installs["Bar/2.0-GCCcore-13.2.0"]["run"], STAMP,
        "kept, not rebuilt"
    );
    assert!(
        installs["Baz/3.0-GCCcore-13.2.0"]["deps"]
            .as_object()
            .unwrap()
            .is_empty(),
        "UnZip is no longer in EESSI to depend on"
    );
    assert_eq!(
        installs["Baz/3.0-GCCcore-13.2.0"]["args"],
        serde_json::json!(["--try-amend=configopts=-DX=OFF -DY=OFF"]),
        "a rebuild keeps the line's options"
    );
    assert!(installs["Foo/1.0-GCCcore-13.2.0"].get("args").is_none());

    // Each install now has the EESSI of now as its baseline.
    let o = watch(&t, "20260103_000000", &["--dry-run"], &with(&vars));
    assert_eq!(code(&o), 0, "{}", stderr(&o));
    assert!(
        stdout(&o).starts_with("EESSI 2023.06 x86_64/generic: 5 recorded installs, 0 to rebuild, 0 kept, 5 unaffected, 0 gone\n"),
        "{}",
        stdout(&o)
    );
}

#[test]
fn the_rebuild_of_a_target_with_a_selector_builds_every_line_of_its_list() {
    let t = setup(WATCH);
    let site = std::fs::read_to_string(&t.site).unwrap().replace(
        "name = \"cpu\"\n",
        "name = \"cpu\"\n[targets.select]\noption = \"--only-targets\"\nvalue = \"cpu\"\nwhen_absent = \"none\"\nstrip = true\n",
    );
    std::fs::write(&t.site, site).unwrap();
    stack(&t);
    let vars = eessi_vars(&t, "2023.06");
    let list = "Foo-1.0-GCCcore-13.2.0.eb --only-targets=cpu\nBar-2.0-GCCcore-13.2.0.eb --only-targets=gpu\nBaz-3.0-GCCcore-13.2.0.eb --only-targets=gpu,cpu --try-amend=x=1\n";
    let o = run(&t, "cpu", STAMP, list, &with(&vars));
    assert_eq!(code(&o), 0, "{}", stderr(&o));
    let installs = &t.watch_state("cpu")["installs"];
    assert!(
        installs["Foo/1.0-GCCcore-13.2.0"].get("args").is_none(),
        "the record keeps the options eb got, without the one the selector strips"
    );
    assert_eq!(
        installs["Baz/3.0-GCCcore-13.2.0"]["args"],
        serde_json::json!(["--try-amend=x=1"])
    );
    assert!(installs.get("Bar/2.0-GCCcore-13.2.0").is_none());

    let builds = t.calls("eb");
    change(&t);
    let o = watch(&t, LATER, &[], &with(&vars));
    assert_eq!(code(&o), 0, "{}", stderr(&o));
    let calls = t.calls("eb");
    let rebuilt: Vec<&str> = calls[builds.len()..]
        .lines()
        .map(|l| l.split(" --tmp-logdir=").next().unwrap())
        .collect();
    assert_eq!(
        rebuilt,
        [
            "Foo-1.0-GCCcore-13.2.0.eb --rebuild",
            UNZIP,
            "Baz-3.0-GCCcore-13.2.0.eb --try-amend=x=1 --rebuild"
        ],
        "the vanished UnZip, which has no selector option, is built too"
    );
    assert!(t.summary(LATER)["targets"][0].get("deselected").is_none());
}

#[test]
fn a_failed_rebuild_is_planned_again_by_the_next_watch() {
    let t = built("");
    let vars = eessi_vars(&t, "2023.06");
    change(&t);
    let mut failing = with(&vars);
    failing.push(("STUB_EB_FAIL", "Qux-4.0-GCCcore-13.2.0"));
    let o = watch(&t, LATER, &[], &failing);
    assert_eq!(code(&o), 1, "{}", stderr(&o));
    assert_eq!(t.summary(LATER)["targets"][0]["status"], "failed");
    let qux = &t.watch_state("cpu")["installs"]["Qux/4.0-GCCcore-13.2.0"];
    assert_eq!(qux["pending"][0]["module"], "Foo/1.0-GCCcore-13.2.0");
    assert_eq!(qux["pending"][0]["change"], "rebuilt");

    std::fs::remove_file(t.root().join("missing.txt")).unwrap();
    let o = watch(&t, "20260103_000000", &["--dry-run"], &with(&vars));
    assert_eq!(code(&o), 0, "{}", stderr(&o));
    let out = stdout(&o);
    assert!(out.starts_with("EESSI 2023.06 x86_64/generic: 5 recorded installs, 1 to rebuild, 0 kept, 4 unaffected, 0 gone\n"), "{out}");
    assert!(
        out.contains("   1. Qux/4.0-GCCcore-13.2.0    Foo/1.0-GCCcore-13.2.0 rebuilt"),
        "{out}"
    );
}

#[test]
fn with_rebuild_review_false_a_build_dependency_change_is_kept_with_a_warning() {
    let t = built("rebuild_review = false\n");
    let vars = eessi_vars(&t, "2023.06");
    t.eessi_module("CMake", "3.27.6-GCCcore-13.2.0");
    let o = watch(&t, LATER, &["--dry-run"], &with(&vars));
    let err = stderr(&o);
    assert_eq!(code(&o), 0, "{err}");
    assert!(
        stdout(&o).contains("1 kept, 3 unaffected"),
        "{}",
        stdout(&o)
    );
    assert!(
        err.contains("WARN [watch] cpu: Foo/1.0-GCCcore-13.2.0 kept although CMake/3.27.6-GCCcore-13.2.0 changed (review), as rebuild_review = false"),
        "{err}"
    );
}

#[test]
fn a_new_eessi_version_is_refused_rather_than_mixed_into_the_old_stack() {
    let t = built("");
    let vars = eessi_vars(&t, "2025.06");
    let o = watch(&t, LATER, &["--dry-run"], &with(&vars));
    let err = stderr(&o);
    assert_eq!(code(&o), 78, "{err}");
    assert!(
        err.contains("ERROR [main] -: the module environment of target cpu loads EESSI 2025.06 x86_64/generic, and 4 of 4 recorded installs were built against EESSI 2023.06 x86_64/generic; a rebuild in place would mix the two: build the stack for the new EESSI (eb-stack stack gap lists what it lacks), then record it with watch --adopt"),
        "{err}"
    );
    assert!(stdout(&o)
        .starts_with("Bar/2.0-GCCcore-13.2.0  built against EESSI 2023.06 x86_64/generic\n"));

    let o = watch(&t, LATER, &["--adopt"], &with(&vars));
    assert_eq!(code(&o), 0, "{}", stderr(&o));
    let s = t.watch_state("cpu");
    assert_eq!(
        s["installs"]["Foo/1.0-GCCcore-13.2.0"]["eessi_version"],
        "2025.06"
    );
    assert_eq!(s["installs"]["Foo/1.0-GCCcore-13.2.0"]["how"], "adopted");
}

#[test]
fn without_eb_stack_watch_says_what_to_rebuild_and_refuses_to_guess_the_rest() {
    let t = built("");
    let vars = eessi_vars(&t, "2023.06");
    t.eessi_module("CMake", "3.27.6-GCCcore-13.2.0");
    std::fs::remove_file(t.stubs.join("eb-stack")).unwrap();
    let builds = t.calls("eb");
    let o = watch(&t, LATER, &["--dry-run"], &with(&vars));
    let err = stderr(&o);
    assert_eq!(code(&o), 78, "{err}");
    assert_eq!(
        stdout(&o),
        "EESSI 2023.06 x86_64/generic: 4 recorded installs, 1 to rebuild, 0 kept, 3 unaffected, 0 gone\nrebuild, in an order not known without eb-stack:\n      Foo/1.0-GCCcore-13.2.0  CMake/3.27.6-GCCcore-13.2.0 changed (review, builddependency)\nunaffected:\n      Bar/2.0-GCCcore-13.2.0\n      Baz/3.0-GCCcore-13.2.0\n      Qux/4.0-GCCcore-13.2.0\n"
    );
    assert!(
        err.contains(
            "eb-stack tells what an install depends on, and cannot run `eb-stack --version`"
        ),
        "{err}"
    );
    assert!(
        err.contains("without it neither the order of the rebuild nor the installs that depend on it are known, and nothing is rebuilt"),
        "{err}"
    );
    assert_eq!(t.calls("eb"), builds);
}

#[test]
fn watch_refuses_without_its_table_waits_for_another_watch_and_for_eessi() {
    let t = setup("");
    let vars = eessi_vars(&t, "2023.06");
    let o = watch(&t, LATER, &[], &with(&vars));
    assert_eq!(code(&o), 78, "{}", stderr(&o));
    assert!(stderr(&o).contains(
        "the site has no [watch] table, or it is disabled, so no install is recorded to watch"
    ));

    let t = setup(WATCH);
    stack(&t);
    let vars = eessi_vars(&t, "2023.06");
    let o = watch(&t, LATER, &[], &with(&vars));
    assert_eq!(code(&o), 0, "{}", stderr(&o));
    assert!(
        stderr(&o).contains("WARN [watch] cpu: no install is recorded in "),
        "{}",
        stderr(&o)
    );

    let dir = t.root().join("install/.eb-pipeline-watch");
    std::fs::create_dir_all(&dir).unwrap();
    let held = std::fs::File::create(dir.join("cpu.watch.lock")).unwrap();
    held.lock().unwrap();
    let o = watch(&t, LATER, &[], &with(&vars));
    assert_eq!(code(&o), 75, "{}", stderr(&o));
    assert!(
        stderr(&o).contains("another watch of target cpu holds "),
        "{}",
        stderr(&o)
    );
    drop(held);

    let mut gone = with(&vars);
    gone.push(("EESSI_MODULEPATH", "/nonexistent/modules/all"));
    let o = watch(&t, LATER, &[], &gone);
    assert_eq!(code(&o), 75, "{}", stderr(&o));
    assert!(
        stderr(&o).contains("cannot read the EESSI module tree /nonexistent/modules/all"),
        "{}",
        stderr(&o)
    );

    let o = watch(&t, LATER, &[], &[]);
    assert_eq!(code(&o), 78, "{}", stderr(&o));
    assert!(
        stderr(&o)
            .contains("no EESSI in the module environment of target cpu: EESSI_VERSION is unset"),
        "{}",
        stderr(&o)
    );
}

/// A stand-in for srun that drops its options and runs the command as job
/// 4242 of alice. With STUB_SRUN_REFUSE set it starts nothing, as srun does
/// when Slurm refuses the job.
const SRUN: &str = r#"#!/bin/sh
if [ -n "${STUB_SRUN_REFUSE:-}" ]; then
  echo "srun: error: Unable to allocate resources: Invalid account or account/partition combination specified" >&2
  exit 1
fi
while [ $# -gt 0 ]; do case $1 in -*) shift ;; *) break ;; esac; done
SLURM_JOB_ID=4242 SLURM_JOB_USER=alice exec "$@"
"#;

#[test]
fn a_probe_that_its_job_directory_stops_quotes_why() {
    let t = setup(WATCH);
    stack(&t);
    let local = with_job_dirs(&t, SRUN);
    let vars = eessi_vars(&t, "2023.06");
    let log = t
        .root()
        .join("scratch/eb-pipeline-watch/cpu")
        .join(LATER)
        .join("plan/eessi-probe.log");
    let error = format!(
        "ERROR [main] -: the module environment of target cpu did not load (exit 1); its output is in {}",
        log.display()
    );

    let o = watch(&t, LATER, &["--dry-run"], &with(&vars));
    let err = stderr(&o);
    assert_eq!(code(&o), 75, "{err}");
    let want = format!(
        "{error}: \"eb-pipeline: job 4242 has no directory {}/alice.4242 that it can write to, which filesystems.job_dir names\"\n",
        local.display()
    );
    assert!(err.ends_with(&want), "{want} does not end\n{err}");

    // Another failure of the job leaves the error as it was.
    let mut refused = with(&vars);
    refused.push(("STUB_SRUN_REFUSE", "1"));
    let o = watch(&t, LATER, &["--dry-run"], &refused);
    let err = stderr(&o);
    assert_eq!(code(&o), 75, "{err}");
    assert!(err.ends_with(&format!("{error}\n")), "{err}");

    std::fs::create_dir_all(local.join("alice.4242")).unwrap();
    let o = watch(&t, LATER, &["--dry-run"], &with(&vars));
    assert_eq!(code(&o), 0, "{}", stderr(&o));
}

#[test]
fn a_stack_shard_that_its_job_directory_stops_quotes_why() {
    let t = built("");
    let local = with_job_dirs(&t, JOB_DIR_SRUN);
    let vars = eessi_vars(&t, "2023.06");
    change(&t);
    // The jobs of the probe and of rebuild-set have a directory; the next
    // job, that of stack shard, has none.
    let mut no_dir = with(&vars);
    no_dir.push(("STUB_NO_JOB_DIR", "shard"));
    let o = watch(&t, LATER, &["--dry-run"], &no_dir);
    let err = stderr(&o);
    assert_eq!(code(&o), 1, "{err}");
    let job = std::fs::read_to_string(t.root().join("srun.jobs")).unwrap();
    let job = job.trim();
    let log = t
        .root()
        .join("scratch/eb-pipeline-watch/cpu")
        .join(LATER)
        .join("plan/order.log");
    let want = format!(
        "ERROR [main] -: eb-stack stack shard exited 1; its output is in {}: \"eb-pipeline: job {job} has no directory {}/alice.{job} that it can write to, which filesystems.job_dir names\"\n",
        log.display(),
        local.display()
    );
    assert!(err.ends_with(&want), "{want} does not end\n{err}");
    let calls = t.calls("ebstack");
    assert!(calls.contains("cascade.changed"), "{calls}");
    assert!(
        !calls.lines().any(|l| l.starts_with("stack shard")),
        "{calls}"
    );
}

#[test]
fn adopt_records_the_installs_no_run_recorded() {
    let t = setup(WATCH);
    stack(&t);
    let vars = eessi_vars(&t, "2023.06");
    std::fs::rename(t.stubs.join("eb-stack"), t.root().join("eb-stack")).unwrap();
    let o = run(&t, "cpu", STAMP, LIST, &with(&vars));
    assert_eq!(code(&o), 0, "{}", stderr(&o));
    assert!(stderr(&o).contains("not recorded"));
    std::fs::rename(t.root().join("eb-stack"), t.stubs.join("eb-stack")).unwrap();

    let o = watch(&t, LATER, &["--adopt", "--dry-run"], &with(&vars));
    assert_eq!(code(&o), 0, "{}", stderr(&o));
    assert_eq!(
        stdout(&o),
        "Bar/2.0-GCCcore-13.2.0\nBaz/3.0-GCCcore-13.2.0\nFoo/1.0-GCCcore-13.2.0\nQux/4.0-GCCcore-13.2.0\n"
    );
    assert!(!t
        .root()
        .join("install/.eb-pipeline-watch/cpu.json")
        .exists());

    let o = watch(&t, LATER, &["--adopt"], &with(&vars));
    let err = stderr(&o);
    assert_eq!(code(&o), 0, "{err}");
    assert!(err.contains("INFO [watch] cpu: adopted 4 against EESSI 2023.06 x86_64/generic: Bar/2.0-GCCcore-13.2.0, Baz/3.0-GCCcore-13.2.0, Foo/1.0-GCCcore-13.2.0, Qux/4.0-GCCcore-13.2.0"), "{err}");
    let foo = &t.watch_state("cpu")["installs"]["Foo/1.0-GCCcore-13.2.0"];
    assert_eq!(foo["how"], "adopted");
    assert_eq!(foo["easyconfig"], "Foo-1.0-GCCcore-13.2.0.eb");
    assert_eq!(
        foo["deps"]["CMake/3.27.6-GCCcore-13.2.0"]["class"],
        "review"
    );
}

#[test]
fn an_install_whose_module_is_gone_is_dropped_from_the_record() {
    let t = built("");
    let vars = eessi_vars(&t, "2023.06");
    std::fs::remove_file(
        t.root()
            .join("install/modules/all/Bar/2.0-GCCcore-13.2.0.lua"),
    )
    .unwrap();
    let o = watch(&t, LATER, &[], &with(&vars));
    let err = stderr(&o);
    assert_eq!(code(&o), 0, "{err}");
    assert!(err.contains("INFO [watch] cpu: gone, dropped from the record:\nINFO [watch] cpu:       Bar/2.0-GCCcore-13.2.0\n"), "{err}");
    assert!(t.watch_state("cpu")["installs"]
        .get("Bar/2.0-GCCcore-13.2.0")
        .is_none());
}
