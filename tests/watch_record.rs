//! A run with `[watch]` records what each install it built was built
//! against in EESSI: the EESSI version, and the identity and class of each
//! EESSI module eb-stack says it depends on.

mod common;

use common::*;

const WATCH: &str = "[watch]\n";

const CMAKE: &str = "CMake-3.27.6-GCCcore-13.2.0.eb";
const LIBEVENT: &str = "libevent-2.1.12-GCCcore-13.2.0.eb";

/// EESSI with CMake and libevent; Foo is built with CMake and links
/// libevent, Bar links libevent.
fn eessi(t: &T) {
    t.eessi_module("CMake", "3.27.6-GCCcore-13.2.0");
    t.eessi_module("libevent", "2.1.12-GCCcore-13.2.0");
    t.deps(&[
        ("Foo/1.0-GCCcore-13.2.0", CMAKE, "review", "builddependency"),
        ("Foo/1.0-GCCcore-13.2.0", LIBEVENT, "no-rebuild", "runtime"),
        ("Bar/2.0-GCCcore-13.2.0", LIBEVENT, "no-rebuild", "runtime"),
        (
            "libevent/2.1.12-GCCcore-13.2.0",
            LIBEVENT,
            "must-rebuild",
            "changed",
        ),
    ]);
}

fn with<'a>(vars: &'a [(&'static str, String)]) -> Vec<(&'static str, &'a str)> {
    vars.iter().map(|(k, v)| (*k, v.as_str())).collect()
}

#[test]
fn a_run_records_what_each_install_it_built_depends_on_in_eessi() {
    let t = setup(WATCH);
    eessi(&t);
    let vars = eessi_vars(&t, "2023.06");
    let o = run(
        &t,
        "cpu",
        STAMP,
        "Foo-1.0-GCCcore-13.2.0.eb\nBar-2.0-GCCcore-13.2.0.eb\n",
        &with(&vars),
    );
    let err = stderr(&o);
    assert_eq!(code(&o), 0, "{err}");
    assert!(
        err.contains("INFO [watch] cpu: recorded 2 against EESSI 2023.06 x86_64/generic: Foo/1.0-GCCcore-13.2.0, Bar/2.0-GCCcore-13.2.0"),
        "{err}"
    );

    let s = t.watch_state("cpu");
    assert_eq!(s["kind"], "watch-state");
    assert_eq!(s["schema_version"], 1);
    let foo = &s["installs"]["Foo/1.0-GCCcore-13.2.0"];
    assert_eq!(foo["easyconfig"], "Foo-1.0-GCCcore-13.2.0.eb");
    assert_eq!(foo["how"], "built");
    assert_eq!(foo["run"], STAMP);
    assert_eq!(foo["eessi_version"], "2023.06");
    assert_eq!(foo["software_subdir"], "x86_64/generic");
    assert_eq!(
        foo["modules_dirs"][0],
        t.eessi_modules().display().to_string()
    );
    assert!(
        foo.get("revision").is_none(),
        "a tree outside CVMFS has none"
    );
    let cmake = &foo["deps"]["CMake/3.27.6-GCCcore-13.2.0"];
    assert_eq!(cmake["easyconfig"], CMAKE);
    assert_eq!(cmake["class"], "review");
    assert_eq!(cmake["edge_kind"], "builddependency");
    assert!(cmake["identity"].as_str().unwrap().starts_with("fnv1a64:"));
    assert_eq!(
        foo["deps"]["libevent/2.1.12-GCCcore-13.2.0"]["class"],
        "no-rebuild"
    );
    let bar = &s["installs"]["Bar/2.0-GCCcore-13.2.0"];
    assert_eq!(bar["deps"].as_object().unwrap().len(), 1);

    // Every EESSI easyconfig counts as changed, so every edge to EESSI shows.
    let calls = t.calls("ebstack");
    assert!(calls.starts_with("rebuild-set --changed "), "{calls}");
    let changed = calls.split_whitespace().nth(2).unwrap();
    assert!(
        !std::path::Path::new(changed).exists(),
        "the work directory goes once the record is written"
    );
}

#[test]
fn a_record_keeps_content_digests_of_its_site_file_and_easyconfig_trees() {
    let t = setup("[watch]\neasyconfigs = [\"@ROOT@/robot\"]\n");
    eessi(&t);
    let tree = t.root().join("robot");
    std::fs::create_dir_all(tree.join("f/Foo")).unwrap();
    let recipe = tree.join("f/Foo/Foo-1.0.eb");
    std::fs::write(&recipe, "name = 'Foo'\n").unwrap();
    let vars = eessi_vars(&t, "2023.06");

    let o = run(
        &t,
        "cpu",
        STAMP,
        "Foo-1.0-GCCcore-13.2.0.eb\n",
        &with(&vars),
    );
    assert_eq!(code(&o), 0, "{}", stderr(&o));

    let inputs = &t.watch_state("cpu")["inputs"];
    assert_eq!(
        inputs["site_file"]["path"],
        std::fs::canonicalize(&t.site)
            .unwrap()
            .display()
            .to_string()
    );
    assert_eq!(
        inputs["site_file"]["size"],
        std::fs::metadata(&t.site).unwrap().len()
    );
    assert_eq!(
        inputs["easyconfig_trees"][0]["path"],
        std::fs::canonicalize(&tree).unwrap().display().to_string()
    );
    assert_eq!(
        inputs["easyconfig_trees"][0]["size"],
        std::fs::metadata(&recipe).unwrap().len()
    );
    assert_eq!(inputs["site_file"]["sha256"].as_str().unwrap().len(), 64);
    assert_eq!(
        inputs["easyconfig_trees"][0]["sha256"]
            .as_str()
            .unwrap()
            .len(),
        64
    );
}

#[test]
fn a_run_records_the_cvmfs_revision_of_the_repository_and_names_it() {
    let t = setup(WATCH);
    eessi(&t);
    let repo = t.root().join("eessi");
    set_revision(&repo, 41);
    let mut vars = eessi_vars(&t, "2023.06");
    vars.push(("EESSI_CVMFS_REPO", repo.display().to_string()));
    let o = run(
        &t,
        "cpu",
        STAMP,
        "Bar-2.0-GCCcore-13.2.0.eb\n",
        &with(&vars),
    );
    let err = stderr(&o);
    assert_eq!(code(&o), 0, "{err}");
    assert!(
        err.contains("INFO [watch] cpu: recorded 1 against EESSI 2023.06 x86_64/generic at revision 41: Bar/2.0-GCCcore-13.2.0"),
        "{err}"
    );
    assert_eq!(
        t.watch_state("cpu")["installs"]["Bar/2.0-GCCcore-13.2.0"]["revision"],
        41
    );
}

#[test]
fn a_line_that_left_its_install_as_it_was_is_not_recorded_again() {
    let t = setup(WATCH);
    eessi(&t);
    let vars = eessi_vars(&t, "2023.06");
    let o = run(
        &t,
        "cpu",
        STAMP,
        "Foo-1.0-GCCcore-13.2.0.eb\n",
        &with(&vars),
    );
    assert_eq!(code(&o), 0, "{}", stderr(&o));

    let later = "20260102_000000";
    let o = run(
        &t,
        "cpu",
        later,
        "Foo-1.0-GCCcore-13.2.0.eb\nBar-2.0-GCCcore-13.2.0.eb\n",
        &with(&vars),
    );
    let err = stderr(&o);
    assert_eq!(code(&o), 0, "{err}");
    assert!(
        err.contains(
            "INFO [watch] cpu: 1 line(s) left their install as it was and are not recorded again"
        ),
        "{err}"
    );
    assert!(
        err.contains("recorded 1 against EESSI 2023.06 x86_64/generic: Bar/2.0-GCCcore-13.2.0"),
        "{err}"
    );
    let s = t.watch_state("cpu");
    assert_eq!(s["installs"]["Foo/1.0-GCCcore-13.2.0"]["run"], STAMP);
    assert_eq!(s["installs"]["Bar/2.0-GCCcore-13.2.0"]["run"], later);

    // A rebuild is a build: its install gets a new log.
    let o = run(
        &t,
        "cpu",
        "20260103_000000",
        "Foo-1.0-GCCcore-13.2.0.eb --rebuild\n",
        &with(&vars),
    );
    assert_eq!(code(&o), 0, "{}", stderr(&o));
    assert_eq!(
        t.watch_state("cpu")["installs"]["Foo/1.0-GCCcore-13.2.0"]["run"],
        "20260103_000000"
    );
}

#[test]
fn without_eb_stack_or_eessi_the_run_succeeds_and_warns_that_nothing_was_recorded() {
    let t = setup(WATCH);
    eessi(&t);
    let vars = eessi_vars(&t, "2023.06");
    std::fs::remove_file(t.stubs.join("eb-stack")).unwrap();
    let o = run(
        &t,
        "cpu",
        STAMP,
        "Foo-1.0-GCCcore-13.2.0.eb\n",
        &with(&vars),
    );
    let err = stderr(&o);
    assert_eq!(code(&o), 0, "{err}");
    assert!(
        err.contains("WARN [watch] cpu: Foo/1.0-GCCcore-13.2.0 not recorded: eb-stack tells what an install depends on, and cannot run `eb-stack --version`"),
        "{err}"
    );
    assert!(
        err.contains("no EESSI change under them is seen until watch --adopt records them"),
        "{err}"
    );
    assert!(!t
        .root()
        .join("install/.eb-pipeline-watch/cpu.json")
        .exists());

    let t = setup(WATCH);
    eessi(&t);
    let o = run(
        &t,
        "cpu",
        STAMP,
        "Foo-1.0-GCCcore-13.2.0.eb\n",
        &[("STUB_EB_INSTALL", "1")],
    );
    let err = stderr(&o);
    assert_eq!(code(&o), 0, "{err}");
    assert!(
        err.contains("not recorded: no EESSI in the module environment of target cpu: EESSI_VERSION is unset"),
        "{err}"
    );
}

#[test]
fn a_record_that_fails_keeps_its_work_directory_but_not_the_environment() {
    let t = setup(WATCH);
    eessi(&t);
    let mut vars = eessi_vars(&t, "2023.06");
    vars.push(("STUB_EB_STACK_FAIL", "1".to_string()));
    vars.push((
        "EB_PIPELINE_WEBHOOK",
        "https://hooks.example.org/hook/s3cr3t".to_string(),
    ));
    let o = run(
        &t,
        "cpu",
        STAMP,
        "Foo-1.0-GCCcore-13.2.0.eb\n",
        &with(&vars),
    );
    let err = stderr(&o);
    assert_eq!(code(&o), 0, "{err}");
    assert!(
        err.contains("WARN [watch] cpu: Foo/1.0-GCCcore-13.2.0 not recorded: "),
        "{err}"
    );
    let work = t
        .root()
        .join("scratch/eb-pipeline-watch/cpu")
        .join(STAMP)
        .join("record");
    assert!(work.join("eessi-probe.log").exists(), "{err}");
    let mut dirs = vec![work];
    while let Some(d) = dirs.pop() {
        for e in std::fs::read_dir(&d).unwrap() {
            let p = e.unwrap().path();
            if p.is_dir() {
                dirs.push(p);
            } else {
                let text = std::fs::read(&p).unwrap_or_default();
                assert!(
                    !text.windows(6).any(|w| w == b"s3cr3t"),
                    "{} holds the webhook's URL",
                    p.display()
                );
            }
        }
    }
}

#[test]
fn a_record_that_its_job_directory_stops_quotes_why_and_fails_as_any_record_does() {
    let t = setup(WATCH);
    eessi(&t);
    let local = with_job_dirs(&t, JOB_DIR_SRUN);
    let vars = eessi_vars(&t, "2023.06");
    let o = run(
        &t,
        "cpu",
        STAMP,
        "Bar-2.0-GCCcore-13.2.0.eb\n",
        &with(&vars),
    );
    let err = stderr(&o);
    assert_eq!(code(&o), 0, "{err}");
    assert!(
        err.contains("INFO [watch] cpu: recorded 1 against EESSI 2023.06 x86_64/generic: Bar/2.0-GCCcore-13.2.0"),
        "{err}"
    );
    let state = t.root().join("install/.eb-pipeline-watch/cpu.json");
    let recorded = std::fs::read(&state).unwrap();

    // The node of the next run's record makes no directory for its job.
    let later = "20260102_000000";
    let mut no_dir = with(&vars);
    no_dir.push(("STUB_NO_JOB_DIR", "watch"));
    let o = run(&t, "cpu", later, "Foo-1.0-GCCcore-13.2.0.eb\n", &no_dir);
    let err = stderr(&o);
    assert_eq!(code(&o), 0, "{err}");
    let job = std::fs::read_to_string(t.root().join("srun.jobs")).unwrap();
    let job = job.trim();
    let work = t
        .root()
        .join("scratch/eb-pipeline-watch/cpu")
        .join(later)
        .join("record");
    let want = format!(
        "WARN [watch] cpu: Foo/1.0-GCCcore-13.2.0 not recorded: the module environment of target cpu did not load (exit 1); its output is in {}: \"eb-pipeline: job {job} has no directory {}/alice.{job} that it can write to, which filesystems.job_dir names\"; no EESSI change under them is seen until watch --adopt records them\nINFO [summary] cpu: summary.json and summary.html written in {}\n",
        work.join("eessi-probe.log").display(),
        local.display(),
        t.run_dir(later).display()
    );
    assert!(err.ends_with(&want), "{want} does not end\n{err}");

    // Otherwise it fails as any record does: the build stands, the record
    // is as it was, and the work directory stays without the environment.
    let result: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(t.root().join("result-cpu.json")).unwrap())
            .unwrap();
    assert_eq!(result["lines"][0]["status"], "success");
    assert_eq!(std::fs::read(&state).unwrap(), recorded);
    let files: Vec<String> = std::fs::read_dir(&work)
        .unwrap()
        .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
        .collect();
    assert_eq!(files, ["eessi-probe.log"]);
    let calls = t.calls("ebstack");
    assert!(!calls.contains(later), "{calls}");
}

#[test]
fn a_rebuild_set_that_its_job_directory_stops_quotes_why() {
    let t = setup(WATCH);
    eessi(&t);
    let local = with_job_dirs(&t, JOB_DIR_SRUN);
    let vars = eessi_vars(&t, "2023.06");
    // The probe's job has a directory; the next job, that of rebuild-set,
    // has none.
    let mut no_dir = with(&vars);
    no_dir.push(("STUB_NO_JOB_DIR", "rebuild-set"));
    let o = run(&t, "cpu", STAMP, "Foo-1.0-GCCcore-13.2.0.eb\n", &no_dir);
    let err = stderr(&o);
    assert_eq!(code(&o), 0, "{err}");
    let job = std::fs::read_to_string(t.root().join("srun.jobs")).unwrap();
    let job = job.trim();
    let work = t
        .root()
        .join("scratch/eb-pipeline-watch/cpu")
        .join(STAMP)
        .join("record");
    let want = format!(
        "WARN [watch] cpu: Foo/1.0-GCCcore-13.2.0 not recorded: eb-stack rebuild-set exited 1; its output is in {}: \"eb-pipeline: job {job} has no directory {}/alice.{job} that it can write to, which filesystems.job_dir names\"; no EESSI change under them is seen until watch --adopt records them\nINFO [summary] cpu: summary.json and summary.html written in {}\n",
        work.join("rebuild-set.log").display(),
        local.display(),
        t.run_dir(STAMP).display()
    );
    assert!(err.ends_with(&want), "{want} does not end\n{err}");
    let state = t.root().join("install/.eb-pipeline-watch/cpu.json");
    assert!(!state.exists(), "{}", state.display());
    assert_eq!(t.calls("ebstack"), "");
}

#[test]
fn watch_paths_must_be_absolute() {
    let t = setup("[watch]\nstate_dir = \"state\"\nmodules_dirs = [\"@ROOT@/eessi\", \"rel\"]\n");
    let site = t.site.display().to_string();
    let o = eb_pipeline(&t, &["check", "--site", &site], &[]);
    let err = stderr(&o);
    assert_eq!(code(&o), 2, "{err}");
    assert!(
        err.contains("watch.state_dir: `state` is not an absolute path"),
        "{err}"
    );
    assert!(
        err.contains("watch.modules_dirs: `rel` is not an absolute path"),
        "{err}"
    );
    assert!(!err.contains("eessi`"), "{err}");
}

#[test]
fn the_watch_state_dir_takes_target_only() {
    let t = setup("[watch]\nstate_dir = \"@ROOT@/state/{target}\"\n");
    let site = t.site.display().to_string();
    let o = eb_pipeline(&t, &["check", "--site", &site], &[]);
    assert_eq!(code(&o), 0, "{}", stderr(&o));
    let t = setup("[watch]\nstate_dir = \"@ROOT@/state/{user}.{job}\"\n");
    let site = t.site.display().to_string();
    let o = eb_pipeline(&t, &["check", "--site", &site], &[]);
    let err = stderr(&o);
    assert_eq!(code(&o), 2, "{err}");
    for p in ["{user}", "{job}"] {
        assert!(
            err.contains(&format!(
                "watch.state_dir: unknown placeholder {p}; known: {{target}}"
            )),
            "{err}"
        );
    }
}
