//! Write a site-admin triage report from one `eb-pipeline run --result` file.

use eb_pipeline::{classify, exit, util};
use serde_json::{Map, Value};
use std::collections::BTreeMap;
use std::ffi::OsString;
use std::path::{Path, PathBuf};
use std::process::ExitCode;

struct Args {
    result: PathBuf,
    out: Option<PathBuf>,
    max_lines: usize,
    log_tail_lines: usize,
}

#[derive(Clone)]
struct Line {
    easyconfig: String,
    phase: String,
    unit: String,
    status: String,
    exit_code: Option<i64>,
    note: Option<String>,
    log: Option<String>,
}

struct Parsed {
    site: String,
    target: String,
    build_type: String,
    run_id: String,
    exit_code: i64,
    started_at: Option<String>,
    ended_at: Option<String>,
    counts: BTreeMap<String, usize>,
    lines: Vec<Line>,
}

struct Analysis {
    unfinished: Vec<usize>,
    failed: Vec<usize>,
    dominant_failed_exit: Option<(i64, usize)>,
    signature_counts: BTreeMap<String, usize>,
    infrastructure: Option<(String, usize)>,
    classify_errors: Vec<String>,
}

fn main() -> ExitCode {
    match run() {
        Ok(code) => code,
        Err(msg) => {
            eprintln!("eb-admin-report: {msg}");
            ExitCode::from(2)
        }
    }
}

fn run() -> Result<ExitCode, String> {
    let args = parse(util::os_args().into_iter())?;
    util::refuse_output_alias(
        "--out",
        args.out.as_deref(),
        &[("--result", Some(args.result.as_path()))],
    )?;
    let (_, value) = exit::read_json_output(&args.result, "eb-pipeline run", exit::USAGE)
        .map_err(|error| error.msg)?;
    let parsed = parse_result(&value)?;
    let analysis = analyze(&parsed);
    let report = render_report(
        &args.result,
        &parsed,
        &analysis,
        args.max_lines,
        args.log_tail_lines,
    );
    match args.out.as_deref() {
        Some(path) if path != Path::new("-") => {
            util::write_atomic(path, report.as_bytes())
                .map_err(|e| format!("cannot write {}: {e}", path.display()))?;
        }
        _ => util::write_stdout(report.as_bytes())
            .map_err(|e| format!("cannot write the report: {e}"))?,
    }
    Ok(ExitCode::SUCCESS)
}

fn parse(words: impl Iterator<Item = OsString>) -> Result<Args, String> {
    let mut result = util::env_value("EB_PIPELINE_RESULT").map(PathBuf::from);
    let mut out = None;
    let mut max_lines = 20usize;
    let mut log_tail_lines = 1usize;
    let mut words = words.peekable();
    while let Some(word) = words.next() {
        if let Some((flag, value)) = util::split_os_flag_value(word.as_os_str()) {
            if matches!(
                flag.as_str(),
                "--result" | "--out" | "--max-lines" | "--log-tail-lines"
            ) {
                if value.is_empty() {
                    return Err(format!("{flag} needs a value"));
                }
                assign(
                    &flag,
                    value,
                    &mut result,
                    &mut out,
                    &mut max_lines,
                    &mut log_tail_lines,
                )?;
                continue;
            }
        }
        let word = util::os_to_string(word)?;
        match word.as_str() {
            "-h" | "--help" => {
                util::write_stdout(include_str!("eb-admin-report-help.txt").as_bytes())
                    .map_err(|error| format!("cannot write help: {error}"))?;
                std::process::exit(0);
            }
            "-V" | "--version" => {
                let version = format!("eb-admin-report {}\n", env!("CARGO_PKG_VERSION"));
                util::write_stdout(version.as_bytes())
                    .map_err(|error| format!("cannot write version: {error}"))?;
                std::process::exit(0);
            }
            "--result" | "--out" | "--max-lines" | "--log-tail-lines" => {
                let value = util::separate_os_value(&mut words, &format!("{word} needs a value"))?;
                assign(
                    &word,
                    value,
                    &mut result,
                    &mut out,
                    &mut max_lines,
                    &mut log_tail_lines,
                )?;
            }
            _ => return Err(format!("does not know `{word}`")),
        }
    }
    let result = result.ok_or("--result is required")?;
    Ok(Args {
        result,
        out,
        max_lines,
        log_tail_lines,
    })
}

fn assign(
    flag: &str,
    value: OsString,
    result: &mut Option<PathBuf>,
    out: &mut Option<PathBuf>,
    max_lines: &mut usize,
    log_tail_lines: &mut usize,
) -> Result<(), String> {
    match flag {
        "--result" => *result = Some(PathBuf::from(value)),
        "--out" => {
            if out.is_some() {
                return Err("--out given twice".into());
            }
            *out = Some(PathBuf::from(value));
        }
        "--max-lines" => {
            *max_lines = parse_positive_usize(&util::os_to_string(value)?, "--max-lines")?;
        }
        "--log-tail-lines" => {
            *log_tail_lines =
                parse_positive_usize(&util::os_to_string(value)?, "--log-tail-lines")?;
        }
        _ => unreachable!(),
    }
    Ok(())
}

fn parse_positive_usize(raw: &str, flag: &str) -> Result<usize, String> {
    let value = raw
        .parse::<usize>()
        .map_err(|_| format!("{flag} expects a positive integer"))?;
    if value == 0 {
        return Err(format!("{flag} must be >= 1"));
    }
    Ok(value)
}

fn parse_result(value: &Value) -> Result<Parsed, String> {
    let obj = object(value, "result")?;
    if req_str(obj, "kind", "result")? != "result" {
        return Err("expected kind `result`".into());
    }
    let mut counts = BTreeMap::new();
    if let Some(raw) = obj.get("counts") {
        for (key, value) in object(raw, "result.counts")? {
            counts.insert(
                key.to_string(),
                as_u64(value, &format!("result.counts.{key}"))? as usize,
            );
        }
    }
    let mut lines = Vec::new();
    for (index, line) in req_array(obj, "lines", "result")?.iter().enumerate() {
        let line_obj = object(line, &format!("result.lines[{index}]"))?;
        lines.push(Line {
            easyconfig: req_str(line_obj, "easyconfig", &format!("result.lines[{index}]"))?
                .to_string(),
            phase: req_str(line_obj, "phase", &format!("result.lines[{index}]"))?.to_string(),
            unit: req_str(line_obj, "unit", &format!("result.lines[{index}]"))?.to_string(),
            status: req_str(line_obj, "status", &format!("result.lines[{index}]"))?.to_string(),
            exit_code: opt_i64(line_obj, "exit_code", &format!("result.lines[{index}]"))?,
            note: opt_str(line_obj, "note", &format!("result.lines[{index}]"))?.map(str::to_string),
            log: opt_str(line_obj, "log", &format!("result.lines[{index}]"))?.map(str::to_string),
        });
    }
    Ok(Parsed {
        site: req_str(obj, "site", "result")?.to_string(),
        target: req_str(obj, "target", "result")?.to_string(),
        build_type: req_str(obj, "build_type", "result")?.to_string(),
        run_id: req_str(obj, "run_id", "result")?.to_string(),
        exit_code: req_i64(obj, "exit_code", "result")?,
        started_at: opt_str(obj, "started_at", "result")?.map(str::to_string),
        ended_at: opt_str(obj, "ended_at", "result")?.map(str::to_string),
        counts,
        lines,
    })
}

fn render_report(
    result_path: &Path,
    parsed: &Parsed,
    analysis: &Analysis,
    max_lines: usize,
    log_tail_lines: usize,
) -> String {
    let unfinished: Vec<&Line> = analysis
        .unfinished
        .iter()
        .map(|index| &parsed.lines[*index])
        .collect();
    let systemic = analysis.infrastructure.is_some();
    let mut out = String::new();
    out.push_str("# eb-admin-report\n\n");
    out.push_str(&format!("- Result: `{}`\n", result_path.display()));
    out.push_str(&format!("- Site: `{}`\n", parsed.site));
    out.push_str(&format!("- Target: `{}`\n", parsed.target));
    out.push_str(&format!("- Build type: `{}`\n", parsed.build_type));
    out.push_str(&format!("- Run id: `{}`\n", parsed.run_id));
    out.push_str(&format!("- Exit code: `{}`\n", parsed.exit_code));
    if let Some(started) = &parsed.started_at {
        out.push_str(&format!("- Started: `{started}`\n"));
    }
    if let Some(ended) = &parsed.ended_at {
        out.push_str(&format!("- Ended: `{ended}`\n"));
    }
    out.push('\n');
    out.push_str("## Counts\n\n");
    out.push_str("| status | count |\n|---|---:|\n");
    if parsed.counts.is_empty() {
        out.push_str("| (none) | 0 |\n");
    } else {
        for (status, count) in &parsed.counts {
            out.push_str(&format!("| {status} | {count} |\n"));
        }
    }
    out.push('\n');
    out.push_str("## Triage\n\n");
    out.push_str(&format!(
        "- Unfinished lines: {} of {}\n",
        unfinished.len(),
        parsed.lines.len()
    ));
    out.push_str(&format!("- Failed lines: {}\n", analysis.failed.len()));
    if let Some((code, count)) = analysis.dominant_failed_exit {
        out.push_str(&format!(
            "- Dominant failed exit code: `{code}` on `{count}` line(s)\n"
        ));
    }
    out.push_str(&format!(
        "- Failure signatures observed: {}\n",
        analysis.signature_counts.len()
    ));
    out.push_str(&format!(
        "- Systemic infrastructure failure: `{}`\n",
        if systemic { "yes" } else { "no" }
    ));
    if systemic {
        if let Some((kind, count)) = &analysis.infrastructure {
            out.push_str(&format!(
                "- Infrastructure class: `{kind}` on `{count}` line(s)\n"
            ));
            out.push_str(
                "- Suggested action: resolve the shared infrastructure issue before producing a retry list.\n",
            );
        }
    } else if unfinished.is_empty() {
        out.push_str("- Suggested action: no retry needed.\n");
    } else {
        out.push_str(
            "- Suggested action: run `eb-retry-list --result <result.json> --list <list>` to prepare a retry list.\n",
        );
    }
    out.push('\n');
    out.push_str("## Failure signatures (eb-stack bot-classify)\n\n");
    if analysis.signature_counts.is_empty() {
        out.push_str(
            "No failure signatures were recorded (missing logs or unavailable classifier).\n\n",
        );
    } else {
        out.push_str("| signature | lines |\n|---|---:|\n");
        for (signature, count) in &analysis.signature_counts {
            out.push_str(&format!("| {signature} | {count} |\n"));
        }
        out.push('\n');
    }
    if !analysis.classify_errors.is_empty() {
        out.push_str("Classifier notes:\n");
        for note in analysis.classify_errors.iter().take(5) {
            out.push_str(&format!("- {note}\n"));
        }
        if analysis.classify_errors.len() > 5 {
            out.push_str(&format!(
                "- and {} more classification error(s)\n",
                analysis.classify_errors.len() - 5
            ));
        }
        out.push('\n');
    }
    out.push_str("## Actionable lines\n\n");
    if unfinished.is_empty() {
        out.push_str("All lines succeeded.\n");
        return out;
    }
    for (index, line) in unfinished.iter().take(max_lines).enumerate() {
        out.push_str(&format!(
            "{}. `{}` status=`{}` phase=`{}` unit=`{}`",
            index + 1,
            line.easyconfig,
            line.status,
            line.phase,
            line.unit
        ));
        if let Some(code) = line.exit_code {
            out.push_str(&format!(" exit=`{code}`"));
        }
        out.push('\n');
        if let Some(note) = &line.note {
            out.push_str(&format!("   - note: {note}\n"));
        }
        if let Some(log) = &line.log {
            out.push_str(&format!("   - log: `{log}`\n"));
            if let Some(last) = log_tail(Path::new(log), log_tail_lines) {
                out.push_str(&format!("   - log tail: `{}`\n", last.replace('`', "'")));
            }
        }
    }
    if unfinished.len() > max_lines {
        out.push_str(&format!(
            "\nOnly the first {max_lines} unfinished line(s) are shown.\n"
        ));
    }
    out
}

fn analyze(parsed: &Parsed) -> Analysis {
    let unfinished: Vec<usize> = parsed
        .lines
        .iter()
        .enumerate()
        .filter_map(|(index, line)| (line.status != "success").then_some(index))
        .collect();
    let failed: Vec<usize> = unfinished
        .iter()
        .copied()
        .filter(|index| parsed.lines[*index].status == "failed")
        .collect();
    let failed_lines: Vec<&Line> = failed.iter().map(|index| &parsed.lines[*index]).collect();
    let dominant_failed_exit = dominant_exit_code(&failed_lines);
    let mut signature_counts = BTreeMap::<String, usize>::new();
    let mut infrastructure_counts = BTreeMap::<String, usize>::new();
    let mut classify_errors = Vec::new();
    for line in &failed_lines {
        let Some(raw_log) = line.log.as_deref() else {
            continue;
        };
        let log = Path::new(raw_log);
        let text = match util::read_text_file(log) {
            Ok(text) => text,
            Err(error) => {
                classify_errors.push(format!(
                    "{}: cannot read `{}`: {error}",
                    line.easyconfig,
                    log.display()
                ));
                continue;
            }
        };
        let job = match classify::classify_log(log) {
            Ok(job) => job,
            Err(error) => {
                classify_errors.push(format!("{}: {error}", line.easyconfig));
                continue;
            }
        };
        let signature = signature(&job.class, &job.sub_class);
        *signature_counts.entry(signature).or_default() += 1;
        if let Some(kind) = classify::infrastructure_class(&job, &text) {
            *infrastructure_counts.entry(kind.to_string()).or_default() += 1;
        }
    }
    let infrastructure = if !failed.is_empty()
        && unfinished.len() == parsed.lines.len()
        && failed.len() == parsed.lines.len()
        && infrastructure_counts.len() == 1
    {
        infrastructure_counts.into_iter().next()
    } else {
        None
    };
    Analysis {
        unfinished,
        failed,
        dominant_failed_exit,
        signature_counts,
        infrastructure,
        classify_errors,
    }
}

fn signature(class: &str, sub_class: &str) -> String {
    match (class.is_empty(), sub_class.is_empty()) {
        (false, false) => format!("{class}/{sub_class}"),
        (false, true) => class.to_string(),
        (true, false) => sub_class.to_string(),
        (true, true) => "unclassified".into(),
    }
}

fn dominant_exit_code(lines: &[&Line]) -> Option<(i64, usize)> {
    let mut counts = BTreeMap::<i64, usize>::new();
    for line in lines {
        if let Some(code) = line.exit_code {
            *counts.entry(code).or_default() += 1;
        }
    }
    counts.into_iter().max_by_key(|(_, count)| *count)
}

fn log_tail(path: &Path, lines: usize) -> Option<String> {
    let text = util::read_text_file(path).ok()?;
    let tail = text
        .lines()
        .rev()
        .filter(|line| !line.trim().is_empty())
        .take(lines)
        .collect::<Vec<_>>();
    if tail.is_empty() {
        None
    } else {
        Some(tail.into_iter().rev().collect::<Vec<_>>().join(" || "))
    }
}

fn object<'a>(value: &'a Value, what: &str) -> Result<&'a Map<String, Value>, String> {
    value
        .as_object()
        .ok_or_else(|| format!("{what} must be a JSON object"))
}

fn req_array<'a>(
    obj: &'a Map<String, Value>,
    key: &str,
    what: &str,
) -> Result<&'a [Value], String> {
    match obj.get(key) {
        Some(Value::Array(values)) => Ok(values.as_slice()),
        Some(_) => Err(format!("{what}.{key} must be an array")),
        None => Err(format!("{what} is missing `{key}`")),
    }
}

fn req_str<'a>(obj: &'a Map<String, Value>, key: &str, what: &str) -> Result<&'a str, String> {
    match obj.get(key) {
        Some(Value::String(value)) => Ok(value),
        Some(_) => Err(format!("{what}.{key} must be a string")),
        None => Err(format!("{what} is missing `{key}`")),
    }
}

fn opt_str<'a>(
    obj: &'a Map<String, Value>,
    key: &str,
    what: &str,
) -> Result<Option<&'a str>, String> {
    match obj.get(key) {
        Some(Value::String(value)) => Ok(Some(value)),
        Some(_) => Err(format!("{what}.{key} must be a string")),
        None => Ok(None),
    }
}

fn req_i64(obj: &Map<String, Value>, key: &str, what: &str) -> Result<i64, String> {
    match obj.get(key) {
        Some(Value::Number(value)) => value
            .as_i64()
            .ok_or_else(|| format!("{what}.{key} must be an integer")),
        Some(_) => Err(format!("{what}.{key} must be an integer")),
        None => Err(format!("{what} is missing `{key}`")),
    }
}

fn opt_i64(obj: &Map<String, Value>, key: &str, what: &str) -> Result<Option<i64>, String> {
    match obj.get(key) {
        Some(Value::Number(value)) => value
            .as_i64()
            .ok_or_else(|| format!("{what}.{key} must be an integer"))
            .map(Some),
        Some(_) => Err(format!("{what}.{key} must be an integer")),
        None => Ok(None),
    }
}

fn as_u64(value: &Value, what: &str) -> Result<u64, String> {
    match value {
        Value::Number(number) => number
            .as_u64()
            .ok_or_else(|| format!("{what} must be an unsigned integer")),
        _ => Err(format!("{what} must be an unsigned integer")),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn report_marks_a_systemic_command_path_failure() {
        let dir = tempfile::tempdir().unwrap();
        let log = dir.path().join("line.log");
        std::fs::write(&log, "first\nbash: eb: command not found\n").unwrap();
        let parsed = Parsed {
            site: "site".into(),
            target: "target".into(),
            build_type: "production".into(),
            run_id: "run".into(),
            exit_code: 1,
            started_at: None,
            ended_at: None,
            counts: BTreeMap::from([("failed".into(), 2usize)]),
            lines: vec![
                Line {
                    easyconfig: "A.eb".into(),
                    phase: "build".into(),
                    unit: "all".into(),
                    status: "failed".into(),
                    exit_code: Some(127),
                    note: None,
                    log: Some(log.display().to_string()),
                },
                Line {
                    easyconfig: "B.eb".into(),
                    phase: "build".into(),
                    unit: "all".into(),
                    status: "failed".into(),
                    exit_code: Some(127),
                    note: None,
                    log: Some(log.display().to_string()),
                },
            ],
        };
        let analysis = Analysis {
            unfinished: vec![0, 1],
            failed: vec![0, 1],
            dominant_failed_exit: Some((127, 2)),
            signature_counts: BTreeMap::from([("infra/command-path".into(), 2usize)]),
            infrastructure: Some(("command-path".into(), 2usize)),
            classify_errors: Vec::new(),
        };
        let report = render_report(Path::new("result.json"), &parsed, &analysis, 20, 1);
        assert!(
            report.contains("Systemic infrastructure failure: `yes`"),
            "{report}"
        );
        assert!(
            report.contains("Infrastructure class: `command-path`"),
            "{report}"
        );
        assert!(report.contains("command not found"), "{report}");
    }

    #[test]
    fn parse_requires_result_kind() {
        let value: Value = serde_json::from_str(r#"{"kind":"plan","schema_version":1}"#).unwrap();
        let err = match parse_result(&value) {
            Ok(_) => String::new(),
            Err(err) => err,
        };
        assert!(err.contains("expected kind `result`"), "{err}");
    }
}
