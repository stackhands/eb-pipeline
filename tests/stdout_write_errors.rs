use std::fs;
use std::path::Path;
use std::process::{Command, Stdio};

#[derive(Clone, Copy, Debug)]
enum OutputKind {
    Report,
    Help,
    Version,
}

#[derive(Clone, Copy)]
struct BinarySpec {
    name: &'static str,
    expected_exit: i32,
    report: fn(&Path) -> Command,
}

fn binary(name: &str) -> Command {
    let path = match name {
        "eb-pipeline" => env!("CARGO_BIN_EXE_eb-pipeline"),
        "eb-cvmfs" => env!("CARGO_BIN_EXE_eb-cvmfs"),
        "eb-junit" => env!("CARGO_BIN_EXE_eb-junit"),
        "eb-retry-list" => env!("CARGO_BIN_EXE_eb-retry-list"),
        "eb-shard-weights" => env!("CARGO_BIN_EXE_eb-shard-weights"),
        "eb-test-map" => env!("CARGO_BIN_EXE_eb-test-map"),
        _ => unreachable!("unknown binary"),
    };
    Command::new(path)
}

fn run_with_full_stdout(mut command: Command) -> (i32, String) {
    let full = fs::OpenOptions::new()
        .write(true)
        .open("/dev/full")
        .unwrap();
    let output = command.stdout(Stdio::from(full)).output().unwrap();
    (
        output.status.code().unwrap_or(-1),
        String::from_utf8_lossy(&output.stderr).into_owned(),
    )
}

fn canonical_stdout_error(stderr: &str) -> Option<String> {
    let marker = "cannot write stdout:";
    stderr
        .lines()
        .filter_map(|line| line.find(marker).map(|index| line[index..].to_string()))
        .next_back()
}

fn stdout_error_mismatch(
    spec: BinarySpec,
    kind: OutputKind,
    root: &Path,
    expected_message: &mut Option<String>,
) -> Option<String> {
    let command = match kind {
        OutputKind::Report => (spec.report)(root),
        OutputKind::Help => {
            let mut c = binary(spec.name);
            c.arg("--help");
            c
        }
        OutputKind::Version => {
            let mut c = binary(spec.name);
            c.arg("--version");
            c
        }
    };
    let (code, stderr) = run_with_full_stdout(command);
    if code != spec.expected_exit {
        return Some(format!(
            "{} {kind:?} exited with {code}, expected {}: {stderr}",
            spec.name, spec.expected_exit
        ));
    }
    let observed = match canonical_stdout_error(&stderr) {
        Some(message) => message,
        None => {
            return Some(format!(
                "{} {kind:?} did not report a canonical stdout write error: {stderr}",
                spec.name
            ));
        }
    };
    if expected_message.as_deref().is_none() {
        *expected_message = Some(observed);
        return None;
    }
    if observed != expected_message.as_deref().unwrap() {
        return Some(format!(
            "{} {kind:?} used a different stdout error message: {}; expected {}",
            spec.name,
            observed,
            expected_message.as_deref().unwrap()
        ));
    }
    None
}

fn report_pipeline(_: &Path) -> Command {
    let mut c = binary("eb-pipeline");
    c.arg("schema");
    c
}

fn report_cvmfs(_: &Path) -> Command {
    let mut c = binary("eb-cvmfs");
    c.args(["--dry-run", "repo.example", "--", "true"]);
    c
}

fn report_junit(root: &Path) -> Command {
    let result = root.join("result.json");
    fs::write(
        &result,
        r#"{
  "schema_version": 1,
  "kind": "result",
  "site": "example",
  "target": "cpu",
  "build_type": "production",
  "run_id": "n",
  "exit_code": 0,
  "lines": []
}
"#,
    )
    .unwrap();
    let mut c = binary("eb-junit");
    c.arg("--result").arg(result);
    c
}

fn report_retry_list(root: &Path) -> Command {
    let list = root.join("list.txt");
    fs::write(&list, "Foo-1.0.eb\n").unwrap();
    let result = root.join("result.json");
    fs::write(
        &result,
        format!(
            r#"{{
  "schema_version": 1,
  "kind": "result",
  "site": "example",
  "target": "cpu",
  "build_type": "production",
  "run_id": "n",
  "list": "{}",
  "exit_code": 1,
  "lines": [
    {{
      "easyconfig": "Foo-1.0.eb",
      "status": "failed",
      "log": "/tmp/fail.log"
    }}
  ]
}}
"#,
            list.display()
        ),
    )
    .unwrap();
    let mut c = binary("eb-retry-list");
    c.arg("--result").arg(result).arg("--list").arg(list);
    c
}

fn report_shard_weights(root: &Path) -> Command {
    let result = root.join("result.json");
    fs::write(
        &result,
        r#"{
  "schema_version": 1,
  "kind": "result",
  "site": "example",
  "target": "cpu",
  "build_type": "production",
  "run_id": "n",
  "ended_at": "2026-01-01T00:00:00Z",
  "exit_code": 0,
  "lines": [
    {
      "easyconfig": "Foo-1.0.eb",
      "phase": "build",
      "status": "success",
      "duration_seconds": 1.0
    }
  ]
}
"#,
    )
    .unwrap();
    let mut c = binary("eb-shard-weights");
    c.arg("--result").arg(result);
    c
}

fn report_test_map(root: &Path) -> Command {
    let list = root.join("list.txt");
    fs::write(&list, "Foo-1.0.eb\n").unwrap();
    let mut c = binary("eb-test-map");
    c.args(["--list"])
        .arg(list)
        .args(["--format", "eessi-modules"]);
    c
}

#[test]
fn every_binary_names_stdout_and_fails_when_stdout_write_fails() {
    let dir = tempfile::tempdir().unwrap();
    let mut failures = Vec::new();
    let specs = [
        BinarySpec {
            name: "eb-pipeline",
            expected_exit: 1,
            report: report_pipeline,
        },
        BinarySpec {
            name: "eb-cvmfs",
            expected_exit: 2,
            report: report_cvmfs,
        },
        BinarySpec {
            name: "eb-junit",
            expected_exit: 2,
            report: report_junit,
        },
        BinarySpec {
            name: "eb-retry-list",
            expected_exit: 2,
            report: report_retry_list,
        },
        BinarySpec {
            name: "eb-shard-weights",
            expected_exit: 2,
            report: report_shard_weights,
        },
        BinarySpec {
            name: "eb-test-map",
            expected_exit: 2,
            report: report_test_map,
        },
    ];
    let mut expected_message = None;
    for spec in specs {
        for kind in [OutputKind::Report, OutputKind::Help, OutputKind::Version] {
            if let Some(mismatch) =
                stdout_error_mismatch(spec, kind, dir.path(), &mut expected_message)
            {
                failures.push(mismatch);
            }
        }
    }
    assert!(
        failures.is_empty(),
        "stdout write contract mismatches:\n{}",
        failures.join("\n")
    );
}
