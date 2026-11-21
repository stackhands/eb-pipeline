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

fn command_for(spec: BinarySpec, kind: OutputKind, root: &Path) -> Command {
    match kind {
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
    }
}

fn stdout_error_mismatch(
    spec: BinarySpec,
    kind: OutputKind,
    root: &Path,
    expected_message: &mut Option<String>,
) -> Option<String> {
    let command = command_for(spec, kind, root);
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

fn specs() -> [BinarySpec; 6] {
    [
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
    ]
}

#[test]
fn every_binary_names_stdout_and_fails_when_stdout_write_fails() {
    let dir = tempfile::tempdir().unwrap();
    let mut failures = Vec::new();
    let specs = specs();
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

#[derive(Clone, Copy, Debug)]
enum StderrFault {
    Full,
    ClosedPipe,
}

fn stderr_stdio(fault: StderrFault) -> Stdio {
    match fault {
        StderrFault::Full => Stdio::from(
            fs::OpenOptions::new()
                .write(true)
                .open("/dev/full")
                .unwrap(),
        ),
        StderrFault::ClosedPipe => {
            let (reader, writer) = std::io::pipe().unwrap();
            drop(reader);
            Stdio::from(writer)
        }
    }
}

fn exit_with_unwritable_stderr(mut command: Command, stdout_full: bool, fault: StderrFault) -> i32 {
    command.stderr(stderr_stdio(fault));
    if stdout_full {
        command.stdout(Stdio::from(
            fs::OpenOptions::new()
                .write(true)
                .open("/dev/full")
                .unwrap(),
        ));
    } else {
        command.stdout(Stdio::null());
    }
    command.status().unwrap().code().unwrap_or(-1)
}

#[test]
fn every_binary_keeps_its_exit_code_when_stdout_and_stderr_are_unwritable() {
    let dir = tempfile::tempdir().unwrap();
    let mut failures = Vec::new();
    for spec in specs() {
        for kind in [OutputKind::Report, OutputKind::Help, OutputKind::Version] {
            for fault in [StderrFault::Full, StderrFault::ClosedPipe] {
                let code =
                    exit_with_unwritable_stderr(command_for(spec, kind, dir.path()), true, fault);
                if code != spec.expected_exit {
                    failures.push(format!(
                        "{} {kind:?} with stdout and stderr {fault:?} exited with {code}, expected {}",
                        spec.name, spec.expected_exit
                    ));
                }
            }
        }
    }
    assert!(
        failures.is_empty(),
        "an unwritable stderr changed an exit code:\n{}",
        failures.join("\n")
    );
}

#[test]
fn every_binary_keeps_its_usage_exit_code_when_stderr_is_unwritable() {
    let mut failures = Vec::new();
    for spec in specs() {
        for fault in [StderrFault::Full, StderrFault::ClosedPipe] {
            let mut command = binary(spec.name);
            command.arg("--no-such-option");
            let code = exit_with_unwritable_stderr(command, false, fault);
            if code != 2 {
                failures.push(format!(
                    "{} with stderr {fault:?} exited with {code}, expected 2",
                    spec.name
                ));
            }
        }
    }
    assert!(
        failures.is_empty(),
        "an unwritable stderr changed a usage exit code:\n{}",
        failures.join("\n")
    );
}
