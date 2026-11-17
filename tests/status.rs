//! `status` prints the last runs under the log root from their summaries.

mod common;
use common::{code, eb_pipeline, run, setup, stderr, stdout, T};
use serde_json::Value;

const LIST: &str = "Foo-1.0-foss-2025a.eb\nBar-2.0-foss-2025a.eb\n";
const A: &str = "20260101_000000";
const B: &str = "20260102_000000";
const C: &str = "20260103_000000";

/// A: cpu succeeds. B: cpu fails a line and gpu's rotation is left to
/// another rotate. C: cpu has started and written no summary yet.
fn three_runs() -> T {
    let t = setup("");
    assert_eq!(code(&run(&t, "cpu", A, LIST, &[])), 0);
    let o = run(
        &t,
        "cpu",
        B,
        LIST,
        &[("STUB_EB_FAIL", "Bar-2.0-foss-2025a")],
    );
    assert_eq!(code(&o), 1, "{}", stderr(&o));
    let o = run(&t, "gpu", B, LIST, &[("STUB_ROTATE_EXIT", "75")]);
    assert_eq!(code(&o), 0, "{}", stderr(&o));
    std::fs::create_dir_all(t.run_dir(C).join("cpu")).unwrap();
    std::fs::create_dir_all(t.root().join("logs/notes")).unwrap();
    t
}

fn status(t: &T, extra: &[&str]) -> std::process::Output {
    let site = t.site.display().to_string();
    let mut args = vec!["status", "--site", &site];
    args.extend_from_slice(extra);
    eb_pipeline(t, &args, &[])
}

#[test]
fn status_prints_the_last_runs_newest_first() {
    let t = three_runs();
    let o = status(&t, &["--last", "2"]);
    assert_eq!(code(&o), 0, "{}", stderr(&o));
    let out = stdout(&o);
    let lines: Vec<&str> = out.lines().collect();
    assert_eq!(lines.len(), 4, "{out}");
    assert_eq!(
        lines[0],
        format!("{C}  no summary.json: the run is still going, or ended before writing one")
    );
    assert_eq!(
        lines[1],
        format!(
            "{B}  failed (exit 1)  {}",
            t.run_dir(B).join("summary.html").display()
        )
    );
    assert!(
        lines[2].starts_with("  cpu  run  failed   exit 1 "),
        "{out}"
    );
    assert!(
        lines[2].ends_with("  1 failed, 1 success; build failed"),
        "{out}"
    );
    assert!(
        lines[3].starts_with("  gpu  run  success  exit 0 "),
        "{out}"
    );
    assert!(
        lines[3].ends_with("  2 success; logs skipped (exit 75)"),
        "{out}"
    );
    assert!(!out.contains(A), "{out}");
}

#[test]
fn status_as_json_carries_each_summary() {
    let t = three_runs();
    let o = status(&t, &["--format", "json"]);
    assert_eq!(code(&o), 0, "{}", stderr(&o));
    let v: Value = serde_json::from_str(&stdout(&o)).unwrap();
    assert_eq!(v["kind"], "status");
    assert_eq!(v["schema_version"], 1);
    let runs = v["runs"].as_array().unwrap();
    let names: Vec<&str> = runs.iter().map(|r| r["run"].as_str().unwrap()).collect();
    assert_eq!(names, [C, B, A]);
    assert!(runs[0]["error"]
        .as_str()
        .unwrap()
        .starts_with("no summary.json"));
    assert_eq!(runs[1]["summary"]["exit_code"], 1);
    assert_eq!(runs[1]["summary"]["targets"].as_array().unwrap().len(), 2);
    assert_eq!(runs[2]["summary"]["status"], "success");
}

#[test]
fn status_of_one_target_shows_only_its_runs_and_entries() {
    let t = three_runs();
    let o = status(&t, &["--target", "gpu"]);
    assert_eq!(code(&o), 0, "{}", stderr(&o));
    let out = stdout(&o);
    let lines: Vec<&str> = out.lines().collect();
    assert_eq!(lines.len(), 2, "{out}");
    assert!(
        lines[0].starts_with(&format!("{B}  failed (exit 1)  ")),
        "{out}"
    );
    assert!(
        lines[1].starts_with("  gpu  run  success  exit 0 "),
        "{out}"
    );
}

#[test]
fn the_columns_of_the_targets_line_up_across_runs() {
    let t = setup("");
    assert_eq!(code(&run(&t, "cpu", A, LIST, &[])), 0);
    let o = run(
        &t,
        "cpu",
        B,
        LIST,
        &[("STUB_EB_FAIL", "Bar-2.0-foss-2025a")],
    );
    assert_eq!(code(&o), 1, "{}", stderr(&o));
    let o = status(&t, &[]);
    assert_eq!(code(&o), 0, "{}", stderr(&o));
    let out = stdout(&o);
    let targets: Vec<&str> = out.lines().filter(|l| l.starts_with("  ")).collect();
    assert_eq!(targets.len(), 2, "{out}");
    assert!(
        targets[0].starts_with("  cpu  run  failed   exit 1 "),
        "{out}"
    );
    assert!(
        targets[1].starts_with("  cpu  run  success  exit 0 "),
        "{out}"
    );
}

#[test]
fn status_without_runs_prints_nothing_and_says_so() {
    let t = setup("");
    let o = status(&t, &[]);
    assert_eq!(code(&o), 0, "{}", stderr(&o));
    assert_eq!(stdout(&o), "");
    assert!(
        stderr(&o).contains(&format!(
            "INFO [status] -: no runs under {}",
            t.root().join("logs").display()
        )),
        "{}",
        stderr(&o)
    );
}

#[test]
fn status_refuses_an_unknown_target_and_no_runs() {
    let t = setup("");
    let o = status(&t, &["--target", "nope"]);
    assert_eq!(code(&o), 2);
    assert!(
        stderr(&o).contains("ERROR [main] -: unknown target `nope`; known: cpu, gpu"),
        "{}",
        stderr(&o)
    );
    let o = status(&t, &["--last", "0"]);
    assert_eq!(code(&o), 2);
    assert!(
        stderr(&o).contains("invalid value '0' for '--last <LAST>'"),
        "{}",
        stderr(&o)
    );
}
