//! Write a JUnit XML report from `eb-pipeline run --result` files.
//!
//! Each recorded line keeps the status the result file already has. This
//! tool does not classify failures and does not call eb-stack.

use eb_pipeline::util;
use serde_json::{Map, Number, Value};
use std::collections::BTreeMap;
use std::env;
use std::fs;
use std::io::{self, Write};
use std::path::Path;
use std::process::ExitCode;

struct Args {
    results: Vec<String>,
    reframe_reports: Vec<String>,
    properties: BTreeMap<String, String>,
    out: Option<String>,
    fail_on_failure: bool,
}

struct Suite {
    name: String,
    class: String,
    hostname: String,
    timestamp: Option<String>,
    time_ms: u64,
    props: Vec<(String, String)>,
    cases: Vec<Case>,
}

struct Case {
    name: String,
    /// Easyconfig, shown as the JUnit `file`. Absent for the synthetic run case.
    file: Option<String>,
    time_ms: Option<u64>,
    body: Option<Body>,
}

struct Body {
    skipped: bool,
    type_attr: String,
    message: String,
    lines: Vec<(String, String)>,
}

fn main() -> ExitCode {
    match run() {
        Ok(code) => code,
        Err(msg) => {
            eprintln!("eb-junit: {msg}");
            ExitCode::from(2)
        }
    }
}

fn run() -> Result<ExitCode, String> {
    let args = parse(env::args().skip(1))?;
    let inputs: Vec<_> = args
        .results
        .iter()
        .map(|path| ("--result", Some(Path::new(path))))
        .chain(
            args.reframe_reports
                .iter()
                .map(|path| ("--reframe-report", Some(Path::new(path)))),
        )
        .collect();
    util::refuse_output_alias("--out", args.out.as_deref().map(Path::new), &inputs)?;
    let mut labeled = Vec::with_capacity(args.results.len());
    for path in &args.results {
        let text = fs::read_to_string(path).map_err(|e| format!("cannot read {path}: {e}"))?;
        let value = serde_json::from_str(&text).map_err(|e| format!("cannot parse {path}: {e}"))?;
        labeled.push((path.clone(), value));
    }
    let mut reframe = Vec::with_capacity(args.reframe_reports.len());
    for path in &args.reframe_reports {
        let text = fs::read_to_string(path).map_err(|e| format!("cannot read {path}: {e}"))?;
        let value = serde_json::from_str(&text).map_err(|e| format!("cannot parse {path}: {e}"))?;
        reframe.push((path.clone(), value));
    }
    let (report, failures) = render_labeled_counted(&labeled, &reframe, &args.properties)?;
    match args.out.as_deref() {
        Some(path) if path != "-" => {
            fs::write(path, &report).map_err(|e| format!("cannot write {path}: {e}"))?;
        }
        _ => {
            let mut out = io::stdout().lock();
            out.write_all(report.as_bytes())
                .map_err(|e| format!("cannot write the report: {e}"))?;
        }
    }
    if args.fail_on_failure && failures > 0 {
        Ok(ExitCode::from(1))
    } else {
        Ok(ExitCode::SUCCESS)
    }
}

fn parse(words: impl Iterator<Item = String>) -> Result<Args, String> {
    let mut results = Vec::new();
    let mut reframe_reports = Vec::new();
    let mut properties = BTreeMap::new();
    let mut out = None;
    let mut fail_on_failure = false;
    let mut words = words.peekable();
    while let Some(word) = words.next() {
        match word.as_str() {
            "-h" | "--help" => {
                print!("{}", include_str!("eb-junit-help.txt"));
                std::process::exit(0);
            }
            "-V" | "--version" => {
                println!("eb-junit {}", env!("CARGO_PKG_VERSION"));
                std::process::exit(0);
            }
            "--fail-on-failure" => fail_on_failure = true,
            "--result" | "--reframe-report" | "--property" | "--out" => {
                let value = words
                    .next()
                    .filter(|v| !v.is_empty())
                    .ok_or_else(|| format!("{word} needs a value"))?;
                assign(
                    &word,
                    value,
                    &mut results,
                    &mut reframe_reports,
                    &mut properties,
                    &mut out,
                )?;
            }
            _ => {
                if let Some((flag, value)) = word.split_once('=') {
                    if matches!(
                        flag,
                        "--result" | "--reframe-report" | "--property" | "--out"
                    ) {
                        if value.is_empty() {
                            return Err(format!("{flag} needs a value"));
                        }
                        assign(
                            flag,
                            value.to_string(),
                            &mut results,
                            &mut reframe_reports,
                            &mut properties,
                            &mut out,
                        )?;
                        continue;
                    }
                }
                return Err(format!("does not know `{word}`"));
            }
        }
    }
    if results.is_empty() && reframe_reports.is_empty() {
        return Err("--result or --reframe-report is required".into());
    }
    Ok(Args {
        results,
        reframe_reports,
        properties,
        out,
        fail_on_failure,
    })
}

fn assign(
    flag: &str,
    value: String,
    results: &mut Vec<String>,
    reframe_reports: &mut Vec<String>,
    properties: &mut BTreeMap<String, String>,
    out: &mut Option<String>,
) -> Result<(), String> {
    match flag {
        "--result" => results.push(value),
        "--reframe-report" => reframe_reports.push(value),
        "--property" => {
            let (name, value) = value
                .split_once('=')
                .ok_or("--property expects NAME=VALUE")?;
            if name.is_empty() {
                return Err("--property name must not be empty".into());
            }
            properties.insert(name.to_string(), value.to_string());
        }
        "--out" => {
            if out.is_some() {
                return Err("--out given twice".into());
            }
            *out = Some(value);
        }
        _ => unreachable!(),
    }
    Ok(())
}

/// Render result objects, in order, as one JUnit document.
/// `label` is the path named in an error.
#[cfg(test)]
fn render_labeled(
    items: &[(String, Value)],
    reframe_reports: &[(String, Value)],
) -> Result<String, String> {
    render_labeled_counted(items, reframe_reports, &BTreeMap::new()).map(|(report, _)| report)
}

fn render_labeled_counted(
    items: &[(String, Value)],
    reframe_reports: &[(String, Value)],
    properties: &BTreeMap<String, String>,
) -> Result<(String, usize), String> {
    let mut suites = Vec::with_capacity(items.len() + reframe_reports.len());
    for (label, value) in items {
        suites.push(parse_suite(value).map_err(|e| format!("{label}: {e}"))?);
    }
    for (label, value) in reframe_reports {
        let parsed = parse_reframe_report(value, label).map_err(|e| format!("{label}: {e}"))?;
        suites.extend(parsed);
    }
    for suite in &mut suites {
        for (name, value) in properties {
            if let Some((_, current)) = suite.props.iter_mut().find(|(key, _)| key == name) {
                *current = value.clone();
            } else {
                suite.props.push((name.clone(), value.clone()));
            }
        }
    }
    let failures = failure_count(&suites);
    Ok((document(&suites), failures))
}

fn parse_reframe_report(value: &Value, label: &str) -> Result<Vec<Suite>, String> {
    let obj = object(value, "reframe report")?;
    let runs = if let Some(v) = obj.get("runs") {
        match v {
            Value::Array(items) => items.as_slice(),
            _ => return Err("`runs` must be an array".into()),
        }
    } else if obj.get("testcases").is_some() {
        std::slice::from_ref(value)
    } else {
        return Err("missing `runs`".into());
    };
    let mut out = Vec::with_capacity(runs.len());
    for (i, run) in runs.iter().enumerate() {
        let run_obj = object(run, &format!("runs[{i}]"))?;
        let testcases = match run_obj.get("testcases") {
            Some(Value::Array(items)) => items,
            Some(_) => return Err(format!("runs[{i}].testcases must be an array")),
            None => return Err(format!("runs[{i}] is missing `testcases`")),
        };
        let mut names = Vec::with_capacity(testcases.len());
        let mut cases = Vec::with_capacity(testcases.len());
        let mut time_ms = 0u64;
        let mut system = opt_str(run_obj, "system", &format!("runs[{i}]"))?.unwrap_or("reframe");
        if system.is_empty() {
            system = "reframe";
        }
        let partition = opt_str(run_obj, "partition", &format!("runs[{i}]"))?.unwrap_or("default");
        let environ = opt_str(run_obj, "environ", &format!("runs[{i}]"))?.unwrap_or("default");
        for (j, item) in testcases.iter().enumerate() {
            let case_obj = object(item, &format!("runs[{i}].testcases[{j}]"))?;
            let name = req_str(case_obj, "name", &format!("runs[{i}].testcases[{j}]"))?.to_string();
            let result =
                req_str(case_obj, "result", &format!("runs[{i}].testcases[{j}]"))?.to_string();
            let status = result.to_ascii_lowercase();
            let case_ms = first_seconds(
                case_obj,
                &["time_total", "duration_seconds", "duration", "time_elapsed"],
                &format!("runs[{i}].testcases[{j}]"),
            )?;
            if let Some(ms) = case_ms {
                time_ms = time_ms
                    .checked_add(ms)
                    .ok_or_else(|| format!("runs[{i}].testcases[{j}] duration is too large"))?;
            }
            let mut fields = vec![
                ("result".to_string(), result.clone()),
                ("name".to_string(), name.clone()),
            ];
            if let Some(reason) = opt_str(
                case_obj,
                "fail_reason",
                &format!("runs[{i}].testcases[{j}]"),
            )? {
                fields.push(("fail_reason".to_string(), reason.to_string()));
            }
            if let Some(stdout) = opt_str(case_obj, "stdout", &format!("runs[{i}].testcases[{j}]"))?
            {
                fields.push(("stdout".to_string(), stdout.to_string()));
            }
            if let Some(stderr) = opt_str(case_obj, "stderr", &format!("runs[{i}].testcases[{j}]"))?
            {
                fields.push(("stderr".to_string(), stderr.to_string()));
            }
            let body = if matches!(status.as_str(), "success" | "passed" | "pass" | "ok") {
                None
            } else if status == "skipped" {
                Some(Body {
                    skipped: true,
                    type_attr: String::new(),
                    message: result.clone(),
                    lines: fields,
                })
            } else {
                Some(Body {
                    skipped: false,
                    type_attr: result.clone(),
                    message: result.clone(),
                    lines: fields,
                })
            };
            names.push(name);
            cases.push(Case {
                name: String::new(),
                file: None,
                time_ms: case_ms,
                body,
            });
        }
        let display = disambiguate(&names);
        for (case, shown) in cases.iter_mut().zip(display) {
            case.name = shown;
        }
        let mut props = vec![
            ("source".to_string(), "reframe-report".to_string()),
            ("report".to_string(), label.to_string()),
            ("run_index".to_string(), (i + 1).to_string()),
            ("system".to_string(), system.to_string()),
            ("partition".to_string(), partition.to_string()),
            ("environ".to_string(), environ.to_string()),
        ];
        if let Some(version) = opt_str(obj, "report_version", "reframe report")? {
            props.push(("report_version".to_string(), version.to_string()));
        }
        let suite_name = format!("reframe/{system}/{partition}/{environ}");
        let timestamp = match opt_str(run_obj, "time_start", &format!("runs[{i}]"))? {
            Some(ts) => Some(ts.to_string()),
            None => opt_str(run_obj, "started_at", &format!("runs[{i}]"))?.map(str::to_string),
        };
        out.push(Suite {
            name: suite_name.clone(),
            class: suite_name,
            hostname: partition.to_string(),
            timestamp,
            time_ms,
            props,
            cases,
        });
    }
    Ok(out)
}

fn parse_suite(value: &Value) -> Result<Suite, String> {
    let obj = object(value, "result")?;
    let kind = req_str(obj, "kind", "result")?;
    if kind != "result" {
        return Err(format!("expected kind `result`, found `{kind}`"));
    }
    match obj.get("schema_version") {
        Some(Value::Number(n)) if n.as_u64() == Some(1) => {}
        Some(_) => return Err("schema_version must be 1".into()),
        None => return Err("missing `schema_version`".into()),
    }
    let site = req_str(obj, "site", "result")?.to_string();
    let target = req_str(obj, "target", "result")?.to_string();
    let build_type = req_str(obj, "build_type", "result")?.to_string();
    let run_id = req_str(obj, "run_id", "result")?.to_string();
    let exit_code = req_i64(obj, "exit_code", "result")?;
    let lines = match obj.get("lines") {
        Some(Value::Array(items)) => items,
        Some(_) => return Err("`lines` must be an array".into()),
        None => return Err("missing `lines`".into()),
    };

    let mut cases = Vec::with_capacity(lines.len());
    let mut names = Vec::with_capacity(lines.len());
    let mut line_ms = 0u64;
    let mut failures = 0usize;
    for (i, item) in lines.iter().enumerate() {
        let line = object(item, &format!("lines[{i}]"))?;
        let easyconfig = req_str(line, "easyconfig", &format!("lines[{i}]"))?.to_string();
        let status = req_str(line, "status", &format!("lines[{i}]"))?;
        if status.is_empty() {
            return Err(format!("lines[{i}].status must not be empty"));
        }
        let phase = req_str(line, "phase", &format!("lines[{i}]"))?.to_string();
        let unit = req_str(line, "unit", &format!("lines[{i}]"))?.to_string();
        let time_ms = opt_seconds(line, "duration_seconds", &format!("lines[{i}]"))?;
        if let Some(ms) = time_ms {
            line_ms = line_ms
                .checked_add(ms)
                .ok_or_else(|| format!("lines[{i}].duration_seconds is too large"))?;
        }
        let mut fields = vec![
            ("easyconfig".to_string(), easyconfig.clone()),
            ("status".to_string(), status.to_string()),
            ("phase".to_string(), phase),
            ("unit".to_string(), unit),
        ];
        if let Some(code) = opt_i64(line, "exit_code", &format!("lines[{i}]"))? {
            fields.push(("exit_code".to_string(), code.to_string()));
        }
        if let Some(code) = opt_i64(line, "translated_from", &format!("lines[{i}]"))? {
            fields.push(("translated_from".to_string(), code.to_string()));
        }
        if let Some(note) = opt_str(line, "note", &format!("lines[{i}]"))? {
            fields.push(("note".to_string(), note.to_string()));
        }
        if let Some(log) = opt_str(line, "log", &format!("lines[{i}]"))? {
            fields.push(("log".to_string(), log.to_string()));
        }
        let body = match status {
            "success" => None,
            "skipped" => Some(Body {
                skipped: true,
                type_attr: String::new(),
                message: status.to_string(),
                lines: fields,
            }),
            _ => {
                failures += 1;
                Some(Body {
                    skipped: false,
                    type_attr: status.to_string(),
                    message: status.to_string(),
                    lines: fields,
                })
            }
        };
        // GitLab keeps the first case of a repeated name and drops the rest,
        // and a pipeline merges every report, so the name carries the target.
        names.push(format!("{target}/{easyconfig}"));
        cases.push(Case {
            name: String::new(),
            file: Some(easyconfig),
            time_ms,
            body,
        });
    }

    if exit_code != 0 && failures == 0 {
        let mut fields = vec![("exit_code".to_string(), exit_code.to_string())];
        if let Some(why) = opt_str(obj, "interrupted_by", "result")? {
            fields.push(("interrupted_by".to_string(), why.to_string()));
        }
        if let Some(msg) = opt_str(obj, "message", "result")? {
            fields.push(("message".to_string(), msg.to_string()));
        }
        names.push(format!("{target}/run"));
        cases.push(Case {
            name: String::new(),
            file: None,
            time_ms: None,
            body: Some(Body {
                skipped: false,
                type_attr: "exit".to_string(),
                message: exit_code.to_string(),
                lines: fields,
            }),
        });
    }

    let display = disambiguate(&names);
    for (case, name) in cases.iter_mut().zip(display) {
        case.name = name;
    }

    let time_ms = match opt_seconds(obj, "duration_seconds", "result")? {
        Some(ms) => ms,
        None => line_ms,
    };
    let mut props = vec![
        ("schema_version".to_string(), "1".to_string()),
        ("site".to_string(), site.clone()),
        ("target".to_string(), target.clone()),
        ("build_type".to_string(), build_type),
        ("run_id".to_string(), run_id),
    ];
    if let Some(list) = opt_str(obj, "list", "result")? {
        props.push(("list".to_string(), list.to_string()));
    }
    if let Some(log_dir) = opt_str(obj, "log_dir", "result")? {
        props.push(("log_dir".to_string(), log_dir.to_string()));
    }
    if let Some(sharded) = opt_bool(obj, "sharded", "result")? {
        props.push((
            "sharded".to_string(),
            if sharded { "true" } else { "false" }.to_string(),
        ));
    }
    props.push(("exit_code".to_string(), exit_code.to_string()));
    if let Some(why) = opt_str(obj, "interrupted_by", "result")? {
        props.push(("interrupted_by".to_string(), why.to_string()));
    }
    if let Some(msg) = opt_str(obj, "message", "result")? {
        props.push(("message".to_string(), msg.to_string()));
    }

    Ok(Suite {
        name: format!("{site}/{target}"),
        class: format!("{site}/{target}"),
        hostname: target,
        timestamp: opt_str(obj, "started_at", "result")?.map(str::to_string),
        time_ms,
        props,
        cases,
    })
}

fn document(suites: &[Suite]) -> String {
    let tests: usize = suites.iter().map(|s| s.cases.len()).sum();
    let failures = failure_count(suites);
    let skipped = suites
        .iter()
        .flat_map(|s| &s.cases)
        .filter(|c| c.body.as_ref().is_some_and(|b| b.skipped))
        .count();

    let time_ms = suites
        .iter()
        .fold(0u64, |sum, s| sum.saturating_add(s.time_ms));
    let mut out = String::new();
    out.push_str("<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n");
    out.push_str(&format!(
        "<testsuites name=\"eb-pipeline\" tests=\"{tests}\" failures=\"{failures}\" errors=\"0\" skipped=\"{skipped}\" time=\"{}\">\n",
        fmt_ms(time_ms)
    ));
    for suite in suites {
        write_suite(&mut out, suite);
    }
    out.push_str("</testsuites>\n");
    out
}

fn failure_count(suites: &[Suite]) -> usize {
    suites
        .iter()
        .flat_map(|suite| &suite.cases)
        .filter(|case| case.body.as_ref().is_some_and(|body| !body.skipped))
        .count()
}

fn write_suite(out: &mut String, suite: &Suite) {
    let failures = suite
        .cases
        .iter()
        .filter(|c| c.body.as_ref().is_some_and(|b| !b.skipped))
        .count();
    let skipped = suite
        .cases
        .iter()
        .filter(|c| c.body.as_ref().is_some_and(|b| b.skipped))
        .count();
    let mut line = format!(
        "  <testsuite name=\"{}\" tests=\"{}\" failures=\"{failures}\" errors=\"0\" skipped=\"{skipped}\" time=\"{}\"",
        escape_attr(&suite.name),
        suite.cases.len(),
        fmt_ms(suite.time_ms)
    );
    if let Some(ts) = &suite.timestamp {
        line.push_str(" timestamp=\"");
        line.push_str(&escape_attr(ts));
        line.push('"');
    }
    line.push_str(" hostname=\"");
    line.push_str(&escape_attr(&suite.hostname));
    line.push_str("\">");
    out.push_str(&line);
    out.push('\n');
    out.push_str("    <properties>\n");
    for (name, value) in &suite.props {
        out.push_str("      <property name=\"");
        out.push_str(&escape_attr(name));
        out.push_str("\" value=\"");
        out.push_str(&escape_attr(value));
        out.push_str("\"/>\n");
    }
    out.push_str("    </properties>\n");
    for case in &suite.cases {
        write_case(out, &suite.class, case);
    }
    out.push_str("  </testsuite>\n");
}

fn write_case(out: &mut String, class: &str, case: &Case) {
    let mut line = format!(
        "    <testcase name=\"{}\" classname=\"{}\"",
        escape_attr(&case.name),
        escape_attr(class)
    );
    if let Some(file) = &case.file {
        line.push_str(" file=\"");
        line.push_str(&escape_attr(file));
        line.push('"');
    }
    if let Some(ms) = case.time_ms {
        line.push_str(" time=\"");
        line.push_str(&fmt_ms(ms));
        line.push('"');
    }
    match &case.body {
        None => {
            line.push_str("/>");
            out.push_str(&line);
            out.push('\n');
        }
        Some(body) => {
            line.push('>');
            out.push_str(&line);
            out.push('\n');
            let tag = if body.skipped { "skipped" } else { "failure" };
            out.push_str("      <");
            out.push_str(tag);
            if !body.skipped {
                out.push_str(" type=\"");
                out.push_str(&escape_attr(&body.type_attr));
                out.push('"');
            }
            out.push_str(" message=\"");
            out.push_str(&escape_attr(&body.message));
            out.push_str("\">\n");
            for (label, value) in &body.lines {
                out.push_str("        ");
                out.push_str(label);
                out.push_str(": ");
                out.push_str(&escape_text(value));
                out.push('\n');
            }
            out.push_str("      </");
            out.push_str(tag);
            out.push_str(">\n    </testcase>\n");
        }
    }
}

fn disambiguate(names: &[String]) -> Vec<String> {
    let mut seen = BTreeMap::<String, usize>::new();
    let mut out = Vec::with_capacity(names.len());
    for name in names {
        let n = seen.entry(name.clone()).or_insert(0);
        *n += 1;
        if *n == 1 {
            out.push(name.clone());
        } else {
            out.push(format!("{name} #{n}"));
        }
    }
    out
}

fn object<'a>(value: &'a Value, what: &str) -> Result<&'a Map<String, Value>, String> {
    value
        .as_object()
        .ok_or_else(|| format!("{what} must be a JSON object"))
}

fn req_str<'a>(obj: &'a Map<String, Value>, key: &str, what: &str) -> Result<&'a str, String> {
    match obj.get(key) {
        Some(Value::String(s)) => Ok(s),
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
        None => Ok(None),
        Some(Value::String(s)) => Ok(Some(s)),
        Some(_) => Err(format!("{what}.{key} must be a string")),
    }
}

fn req_i64(obj: &Map<String, Value>, key: &str, what: &str) -> Result<i64, String> {
    match obj.get(key) {
        Some(Value::Number(n)) => i64_of(n, &format!("{what}.{key}")),
        Some(_) => Err(format!("{what}.{key} must be an integer")),
        None => Err(format!("{what} is missing `{key}`")),
    }
}

fn opt_i64(obj: &Map<String, Value>, key: &str, what: &str) -> Result<Option<i64>, String> {
    match obj.get(key) {
        None => Ok(None),
        Some(Value::Number(n)) => i64_of(n, &format!("{what}.{key}")).map(Some),
        Some(_) => Err(format!("{what}.{key} must be an integer")),
    }
}

fn i64_of(n: &Number, what: &str) -> Result<i64, String> {
    n.as_i64()
        .ok_or_else(|| format!("{what} must be an integer"))
}

fn opt_bool(obj: &Map<String, Value>, key: &str, what: &str) -> Result<Option<bool>, String> {
    match obj.get(key) {
        None => Ok(None),
        Some(Value::Bool(b)) => Ok(Some(*b)),
        Some(_) => Err(format!("{what}.{key} must be a boolean")),
    }
}

fn opt_seconds(obj: &Map<String, Value>, key: &str, what: &str) -> Result<Option<u64>, String> {
    match obj.get(key) {
        None => Ok(None),
        Some(Value::Number(n)) => {
            let f = n
                .as_f64()
                .ok_or_else(|| format!("{what}.{key} must be a finite number of seconds"))?;
            to_millis(f, &format!("{what}.{key}")).map(Some)
        }
        Some(_) => Err(format!("{what}.{key} must be a number of seconds")),
    }
}

fn first_seconds(
    obj: &Map<String, Value>,
    keys: &[&str],
    what: &str,
) -> Result<Option<u64>, String> {
    for key in keys {
        if let Some(ms) = opt_seconds(obj, key, what)? {
            return Ok(Some(ms));
        }
    }
    Ok(None)
}

fn to_millis(v: f64, what: &str) -> Result<u64, String> {
    if !v.is_finite() || v < 0.0 {
        return Err(format!("{what} must be a finite number of seconds >= 0"));
    }
    let ms = (v * 1000.0).round();
    if ms > u64::MAX as f64 {
        return Err(format!("{what} is too large"));
    }
    Ok(ms as u64)
}

fn fmt_ms(ms: u64) -> String {
    format!("{}.{:03}", ms / 1000, ms % 1000)
}

fn escape_attr(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for c in s.chars() {
        match c {
            '\n' | '\r' | '\t' => out.push(' '),
            c if xml_char(c) => push_escaped(&mut out, c),
            _ => {}
        }
    }
    out
}

fn escape_text(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for c in s.chars() {
        if xml_char(c) {
            push_escaped(&mut out, c);
        }
    }
    out
}

fn push_escaped(out: &mut String, c: char) {
    match c {
        '&' => out.push_str("&amp;"),
        '<' => out.push_str("&lt;"),
        '>' => out.push_str("&gt;"),
        '"' => out.push_str("&quot;"),
        '\'' => out.push_str("&apos;"),
        c => out.push(c),
    }
}

fn xml_char(c: char) -> bool {
    matches!(
        c,
        '\u{9}' | '\u{A}' | '\u{D}' | '\u{20}'..='\u{D7FF}' | '\u{E000}'..='\u{FFFD}' | '\u{10000}'..='\u{10FFFF}'
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    fn render(text: &str) -> Result<String, String> {
        let value: Value = serde_json::from_str(text).unwrap();
        render_labeled(&[("result".to_string(), value)], &[])
    }

    #[test]
    fn success_omits_its_log_and_other_statuses_keep_their_names() {
        let text = r#"{
          "schema_version": 1,
          "kind": "result",
          "site": "example",
          "target": "cpu",
          "build_type": "production",
          "run_id": "nightly",
          "exit_code": 1,
          "duration_seconds": 1.5,
          "lines": [
            {"easyconfig": "Ok-1.0.eb", "unit": "all", "phase": "build", "status": "success", "log": "logs/ok.log", "duration_seconds": 0.5},
            {"easyconfig": "Bad & <odd>.eb", "unit": "all", "phase": "build", "status": "failed", "exit_code": 1, "log": "logs/bad.log", "note": "a < b & c", "duration_seconds": 0.25},
            {"easyconfig": "Wait.eb", "unit": "all", "phase": "build", "status": "pending", "exit_code": 75, "log": "logs/wait.log"},
            {"easyconfig": "No.eb", "unit": "all", "phase": "build", "status": "refused", "exit_code": 78, "translated_from": 2, "log": "logs/no.log"},
            {"easyconfig": "Later.eb", "unit": "all", "phase": "build", "status": "skipped", "log": "logs/later.log", "note": "admission refused the unit"},
            {"easyconfig": "Held.eb", "unit": "shard-a", "phase": "sources", "status": "held", "log": "logs/held.log"}
          ]
        }"#;
        let xml = render(text).unwrap();
        assert!(xml.starts_with("<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n"));
        assert!(
            xml.contains("tests=\"6\" failures=\"4\" errors=\"0\" skipped=\"1\" time=\"1.500\"")
        );
        assert!(xml.contains(
            "<testcase name=\"cpu/Ok-1.0.eb\" classname=\"example/cpu\" file=\"Ok-1.0.eb\" time=\"0.500\"/>"
        ));
        assert!(!xml.contains("logs/ok.log"), "{xml}");
        assert!(xml.contains("name=\"cpu/Bad &amp; &lt;odd&gt;.eb\""));
        assert!(xml.contains("file=\"Bad &amp; &lt;odd&gt;.eb\""));
        assert!(xml.contains("<failure type=\"failed\" message=\"failed\">"));
        assert!(xml.contains("note: a &lt; b &amp; c"));
        assert!(xml.contains("log: logs/bad.log"));
        assert!(xml.contains("<failure type=\"pending\" message=\"pending\">"));
        assert!(xml.contains("<failure type=\"refused\" message=\"refused\">"));
        assert!(xml.contains("translated_from: 2"));
        assert!(xml.contains("<skipped message=\"skipped\">"));
        assert!(xml.contains("<failure type=\"held\" message=\"held\">"));
        assert!(xml.contains("phase: sources"));
        assert!(xml.contains("unit: shard-a"));
        assert!(!xml.contains("<testcase name=\"cpu/run\""));
    }

    #[test]
    fn a_repeated_easyconfig_gets_a_distinct_case_name() {
        let text = r#"{
          "schema_version": 1, "kind": "result", "site": "example", "target": "cpu",
          "build_type": "production", "run_id": "n", "exit_code": 0,
          "lines": [
            {"easyconfig": "Ok-1.0.eb", "unit": "all", "phase": "build", "status": "success"},
            {"easyconfig": "Ok-1.0.eb", "unit": "all", "phase": "build", "status": "success"}
          ]
        }"#;
        let xml = render(text).unwrap();
        assert!(xml.contains("name=\"cpu/Ok-1.0.eb\""));
        assert!(xml.contains("name=\"cpu/Ok-1.0.eb #2\""));
    }

    #[test]
    fn a_run_that_stops_before_a_line_fails_is_one_exit_case() {
        let text = r#"{
          "schema_version": 1, "kind": "result", "site": "example", "target": "cpu",
          "build_type": "production", "run_id": "n", "exit_code": 78, "lines": [],
          "message": "the licence gate refused"
        }"#;
        let xml = render(text).unwrap();
        assert!(xml.contains("tests=\"1\" failures=\"1\" errors=\"0\" skipped=\"0\""));
        assert!(xml.contains("<testcase name=\"cpu/run\" classname=\"example/cpu\">"));
        assert!(xml.contains("<failure type=\"exit\" message=\"78\">"));
        assert!(xml.contains("message: the licence gate refused"));
    }

    #[test]
    fn successes_with_a_nonzero_exit_still_report_the_run() {
        let text = r#"{
          "schema_version": 1, "kind": "result", "site": "example", "target": "cpu",
          "build_type": "production", "run_id": "n", "exit_code": 130, "interrupted_by": "SIGINT",
          "lines": [
            {"easyconfig": "Ok-1.0.eb", "unit": "all", "phase": "build", "status": "success", "log": "logs/ok.log"},
            {"easyconfig": "Later.eb", "unit": "all", "phase": "build", "status": "skipped", "log": "logs/later.log"}
          ]
        }"#;
        let xml = render(text).unwrap();
        assert!(xml.contains("failures=\"1\""));
        assert!(xml.contains("skipped=\"1\""));
        assert!(xml.contains("type=\"exit\" message=\"130\""));
        assert!(xml.contains("interrupted_by: SIGINT"));
        assert!(!xml.contains("logs/ok.log"));
        assert!(xml.contains("logs/later.log"));
    }

    #[test]
    fn an_empty_successful_run_has_no_cases() {
        let text = r#"{
          "schema_version": 1, "kind": "result", "site": "example", "target": "cpu",
          "build_type": "production", "run_id": "n", "exit_code": 0, "lines": []
        }"#;
        let xml = render(text).unwrap();
        assert!(
            xml.contains("tests=\"0\" failures=\"0\" errors=\"0\" skipped=\"0\" time=\"0.000\"")
        );
        assert!(!xml.contains("<testcase"));
    }

    #[test]
    fn two_files_stay_in_order_and_a_plan_is_refused() {
        let a = r#"{"schema_version":1,"kind":"result","site":"example","target":"zen2","build_type":"production","run_id":"a","exit_code":0,"lines":[]}"#;
        let b = r#"{"schema_version":1,"kind":"result","site":"example","target":"zen4","build_type":"production","run_id":"b","exit_code":0,"started_at":"2026-11-17T00:00:00+00:00","lines":[]}"#;
        let va: Value = serde_json::from_str(a).unwrap();
        let vb: Value = serde_json::from_str(b).unwrap();
        let xml = render_labeled(&[("a.json".into(), va), ("b.json".into(), vb)], &[]).unwrap();
        let zen2 = xml.find("name=\"example/zen2\"").unwrap();
        let zen4 = xml.find("name=\"example/zen4\"").unwrap();
        assert!(zen2 < zen4, "{xml}");
        assert!(xml.contains("timestamp=\"2026-11-17T00:00:00+00:00\""));
        let plan: Value = serde_json::from_str(r#"{"kind":"plan","schema_version":1}"#).unwrap();
        let err = render_labeled(&[("plan.json".into(), plan)], &[]).unwrap_err();
        assert!(
            err.contains("plan.json: expected kind `result`, found `plan`"),
            "{err}"
        );
    }

    #[test]
    fn a_control_character_in_a_note_is_dropped() {
        let text = "{\"schema_version\":1,\"kind\":\"result\",\"site\":\"example\",\"target\":\"cpu\",\"build_type\":\"production\",\"run_id\":\"n\",\"exit_code\":1,\"lines\":[{\"easyconfig\":\"Bad.eb\",\"unit\":\"all\",\"phase\":\"build\",\"status\":\"failed\",\"note\":\"a\\u0001b\"}]}";
        let xml = render(text).unwrap();
        assert!(xml.contains("note: ab"));
        assert!(!xml.contains('\u{1}'));
    }

    #[test]
    fn reframe_testcases_fold_into_extra_suites() {
        let result: Value = serde_json::from_str(
            r#"{"schema_version":1,"kind":"result","site":"example","target":"cpu","build_type":"production","run_id":"n","exit_code":0,"lines":[]}"#,
        )
        .unwrap();
        let reframe: Value = serde_json::from_str(
            r#"{
  "report_version": "4.8",
  "runs": [
    {
      "system": "cluster",
      "partition": "cpu",
      "environ": "foss",
      "time_start": "2026-11-17T00:00:00+00:00",
      "testcases": [
        {"name": "checks/ok", "result": "success", "time_total": 1.2},
        {"name": "checks/skip", "result": "skipped", "fail_reason": "maint"},
        {"name": "checks/bad", "result": "failure", "fail_reason": "wrong answer"}
      ]
    }
  ]
}"#,
        )
        .unwrap();
        let xml = render_labeled(
            &[("result.json".into(), result)],
            &[("reframe.json".into(), reframe)],
        )
        .unwrap();
        assert!(xml.contains("name=\"reframe/cluster/cpu/foss\""), "{xml}");
        assert!(xml.contains("name=\"checks/ok\" classname=\"reframe/cluster/cpu/foss\""));
        assert!(xml.contains("<skipped message=\"skipped\">"));
        assert!(xml.contains("fail_reason: wrong answer"));
        assert!(xml.contains("<failure type=\"failure\" message=\"failure\">"));
        assert!(xml.contains("report_version"));
    }
}
