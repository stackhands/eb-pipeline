//! Extract the unfinished lines of a build list from a run result or summary.
//!
//! Preserves original list order and per-line `eb` options (including options
//! parsed from an easystack), leaving out `success` and `deselected` entries.

use eb_pipeline::classify;
use eb_pipeline::easystack;
use eb_pipeline::exit;
use eb_pipeline::util::{self, ListLine};
use serde_json::Value;
use std::collections::{BTreeMap, BTreeSet, VecDeque};
use std::env;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, ExitCode, Stdio};
use std::time::{SystemTime, UNIX_EPOCH};

const KNOWN_STATUSES: &[&str] = &["failed", "refused", "pending", "skipped", "unreached"];
/// Sub-classes `stack bot-classify` reports for a failure worth retrying unchanged.
const RETRYABLE_CLASSES: &[&str] = &["interrupted", "resource", "timeout"];
/// The first line `stack oom` prints for a build that does not fit.
const OOM_MARK: &str = " may run out of memory on ";

struct Args {
    result: Option<String>,
    summary: Option<String>,
    target: Option<String>,
    list: Option<String>,
    recorded_list: bool,
    statuses: Option<BTreeSet<String>>,
    out: Option<String>,
    require_lines: bool,
    refuse_systemic_failure: bool,
    classify: bool,
    classes: Option<BTreeSet<String>>,
    node: Option<String>,
    drop_oom: bool,
    easyconfigs: Vec<String>,
    arches: Vec<String>,
}

#[derive(Clone)]
struct Outcome {
    status: String,
    log: Option<String>,
    resumed: bool,
}

struct RunOutcomes {
    exit_code: i64,
    list: Option<String>,
    outcomes: BTreeMap<String, VecDeque<Outcome>>,
    deselected: BTreeMap<String, usize>,
}

fn main() -> ExitCode {
    match run() {
        Ok(code) => code,
        Err(msg) => {
            eprintln!("eb-retry-list: {msg}");
            ExitCode::from(2)
        }
    }
}

fn run() -> Result<ExitCode, String> {
    let args = parse(env::args().skip(1))?;
    check_output_alias(
        args.out.as_deref(),
        &[
            ("--result", args.result.as_deref()),
            ("--summary", args.summary.as_deref()),
            ("--list", args.list.as_deref()),
        ],
    )?;
    let read_recorded = || match (&args.result, &args.summary, &args.target) {
        (Some(path), None, None) => read_result(Path::new(path)),
        (None, Some(path), Some(target)) => read_summary(Path::new(path), target),
        _ => unreachable!("validated in parse"),
    };
    let (lines, recorded, list_source) = if args.recorded_list {
        let recorded = read_recorded()?;
        let list = recorded
            .list
            .clone()
            .ok_or_else(|| "the selected record has no string `list`".to_string())?;
        check_output_alias(args.out.as_deref(), &[("--list", Some(&list))])?;
        (read_list(&list)?, recorded, list)
    } else {
        let list = args.list.as_deref().expect("validated in parse");
        (read_list(list)?, read_recorded()?, list.to_string())
    };
    if args.refuse_systemic_failure {
        if let Some((class, count)) = systemic_infrastructure_failure(&recorded)? {
            return Err(format!(
                "all {count} failed lines match infrastructure class `{class}`; fix the shared failure first, or rerun without --refuse-systemic-failure"
            ));
        }
    }

    let mut selected = select_retry_lines(&lines, &recorded, args.statuses.as_ref())?;
    if args.classify {
        let classes = args.classes.clone().unwrap_or_else(default_classes);
        selected = classify_retry_lines(selected, &classes)?;
    }
    let selected = match args.node.as_deref() {
        Some(node) => filter_oom(
            selected,
            node,
            args.drop_oom,
            &args.easyconfigs,
            &args.arches,
            &list_source,
        )?,
        None => selected.into_iter().map(|line| line.raw).collect(),
    };
    let mut text = selected.join("\n");
    if !selected.is_empty() {
        text.push('\n');
    }
    match args.out.as_deref() {
        Some(path) if path != "-" => {
            util::write_atomic(Path::new(path), text.as_bytes())
                .map_err(|e| format!("cannot write {path}: {e}"))?;
        }
        _ => {
            util::write_stdout(text.as_bytes())
                .map_err(|e| format!("cannot write retry list: {e}"))?;
        }
    }
    if args.require_lines && selected.is_empty() {
        Ok(ExitCode::from(1))
    } else {
        Ok(ExitCode::SUCCESS)
    }
}

fn systemic_infrastructure_failure(
    recorded: &RunOutcomes,
) -> Result<Option<(String, usize)>, String> {
    let failed: Vec<_> = recorded
        .outcomes
        .values()
        .flatten()
        .filter(|outcome| outcome.status == "failed")
        .collect();
    if failed.len() < 2 {
        return Ok(None);
    }
    let mut shared: Option<String> = None;
    for outcome in &failed {
        let Some(log) = outcome.log.as_deref() else {
            return Ok(None);
        };
        let path = Path::new(log);
        let text = util::read_text_file(path)
            .map_err(|e| format!("cannot read {log} for --refuse-systemic-failure: {e}"))?;
        let job = classify::classify_log(path)?;
        let Some(class) = classify::infrastructure_class(&job, &text) else {
            return Ok(None);
        };
        match &shared {
            None => shared = Some(class.to_string()),
            Some(existing) if existing == class => {}
            Some(_) => return Ok(None),
        }
    }
    Ok(shared.map(|class| (class, failed.len())))
}

fn check_output_alias(out: Option<&str>, inputs: &[(&str, Option<&str>)]) -> Result<(), String> {
    let paths: Vec<_> = inputs
        .iter()
        .map(|(option, path)| (*option, path.map(Path::new)))
        .collect();
    util::refuse_output_alias("--out", out.map(Path::new), &paths)
}

fn read_list(path: &str) -> Result<Vec<ListLine>, String> {
    let list_path = Path::new(path);
    let lines = if easystack::is_easystack(list_path) {
        easystack::read(list_path)?
    } else {
        let list_text =
            fs::read_to_string(list_path).map_err(|e| format!("cannot read list {path}: {e}"))?;
        util::parse_list(&list_text).map_err(|f| f.msg)?
    };
    if lines.is_empty() {
        return Err(format!("list {path} holds no lines"));
    }
    Ok(lines)
}

struct Parsed {
    result: Option<String>,
    summary: Option<String>,
    target: Option<String>,
    list: Option<String>,
    recorded_list: bool,
    statuses: Option<BTreeSet<String>>,
    out: Option<String>,
    require_lines: bool,
    refuse_systemic_failure: bool,
    classify: bool,
    classes: Option<BTreeSet<String>>,
    node: Option<String>,
    drop_oom: bool,
    easyconfigs: Vec<String>,
    arches: Vec<String>,
}

fn parse(words: impl Iterator<Item = String>) -> Result<Args, String> {
    let mut parsed = Parsed {
        result: None,
        summary: None,
        target: None,
        list: None,
        recorded_list: false,
        statuses: None,
        out: None,
        require_lines: false,
        refuse_systemic_failure: false,
        classify: false,
        classes: None,
        node: None,
        drop_oom: false,
        easyconfigs: Vec::new(),
        arches: Vec::new(),
    };
    let mut words = words.peekable();

    while let Some(word) = words.next() {
        match word.as_str() {
            "-h" | "--help" => {
                util::write_stdout(include_str!("eb-retry-list-help.txt").as_bytes())
                    .map_err(|error| format!("cannot write help: {error}"))?;
                std::process::exit(0);
            }
            "-V" | "--version" => {
                let version = format!("eb-retry-list {}\n", env!("CARGO_PKG_VERSION"));
                util::write_stdout(version.as_bytes())
                    .map_err(|error| format!("cannot write version: {error}"))?;
                std::process::exit(0);
            }
            "--classify" => parsed.classify = true,
            "--recorded-list" => parsed.recorded_list = true,
            "--require-lines" => parsed.require_lines = true,
            "--refuse-systemic-failure" => parsed.refuse_systemic_failure = true,
            "--drop-oom" => parsed.drop_oom = true,
            "--result" | "--summary" | "--target" | "--list" | "--status" | "--out" | "--class"
            | "--node" | "--easyconfigs" | "--arch" => {
                let value = util::separate_value(&mut words, &format!("{word} needs a value"))?;
                assign_flag(&word, value, &mut parsed)?;
            }
            _ => {
                if let Some((flag, value)) = word.split_once('=') {
                    if matches!(
                        flag,
                        "--result"
                            | "--summary"
                            | "--target"
                            | "--list"
                            | "--status"
                            | "--out"
                            | "--class"
                            | "--node"
                            | "--easyconfigs"
                            | "--arch"
                    ) {
                        if value.is_empty() {
                            return Err(format!("{flag} needs a value"));
                        }
                        assign_flag(flag, value.to_string(), &mut parsed)?;
                        continue;
                    }
                }
                return Err(format!("does not know `{word}`"));
            }
        }
    }

    if parsed.result.is_none() && parsed.summary.is_none() {
        parsed.result = util::env_value("EB_PIPELINE_RESULT");
    }
    if parsed.target.is_none() && parsed.summary.is_some() {
        parsed.target = util::env_value("EB_PIPELINE_TARGET");
    }
    if parsed.list.is_none() && !parsed.recorded_list {
        parsed.list = util::env_value("EB_PIPELINE_LIST");
    }

    if parsed.result.is_some() && parsed.summary.is_some() {
        return Err("pass either --result or --summary, not both".into());
    }
    if parsed.result.is_none() && parsed.summary.is_none() {
        return Err("--result or --summary is required".into());
    }
    if parsed.result.is_some() && parsed.target.is_some() {
        return Err("--target is used with --summary, not --result".into());
    }
    if parsed.summary.is_some() && parsed.refuse_systemic_failure {
        return Err("--refuse-systemic-failure needs --result".into());
    }
    if parsed.summary.is_some() && parsed.target.is_none() {
        return Err("--target is required with --summary".into());
    }
    for (flag, set) in [
        ("--class", parsed.classes.is_some()),
        ("--node", parsed.node.is_some()),
        ("--drop-oom", parsed.drop_oom),
        ("--easyconfigs", !parsed.easyconfigs.is_empty()),
        ("--arch", !parsed.arches.is_empty()),
    ] {
        if set && !parsed.classify {
            return Err(format!("{flag} requires --classify"));
        }
    }
    if parsed.drop_oom && parsed.node.is_none() {
        return Err("--drop-oom requires --node".into());
    }
    for (flag, set) in [
        ("--easyconfigs", !parsed.easyconfigs.is_empty()),
        ("--arch", !parsed.arches.is_empty()),
    ] {
        if set && parsed.node.is_none() {
            return Err(format!("{flag} requires --node"));
        }
    }
    if let Some(node) = &parsed.node {
        check_node(node)?;
    }
    if parsed.list.is_some() && parsed.recorded_list {
        return Err("pass either --list or --recorded-list, not both".into());
    }
    if parsed.list.is_none() && !parsed.recorded_list {
        return Err("--list is required".into());
    }
    Ok(Args {
        result: parsed.result,
        summary: parsed.summary,
        target: parsed.target,
        list: parsed.list,
        recorded_list: parsed.recorded_list,
        statuses: parsed.statuses,
        out: parsed.out,
        require_lines: parsed.require_lines,
        refuse_systemic_failure: parsed.refuse_systemic_failure,
        classify: parsed.classify,
        classes: parsed.classes,
        node: parsed.node,
        drop_oom: parsed.drop_oom,
        easyconfigs: parsed.easyconfigs,
        arches: parsed.arches,
    })
}

fn assign_once(slot: &mut Option<String>, flag: &str, value: String) -> Result<(), String> {
    if slot.is_some() {
        return Err(format!("{flag} was given twice"));
    }
    *slot = Some(value);
    Ok(())
}

fn assign_flag(flag: &str, value: String, parsed: &mut Parsed) -> Result<(), String> {
    match flag {
        "--result" => assign_once(&mut parsed.result, flag, value)?,
        "--summary" => assign_once(&mut parsed.summary, flag, value)?,
        "--target" => assign_once(&mut parsed.target, flag, value)?,
        "--list" => assign_once(&mut parsed.list, flag, value)?,
        "--out" => assign_once(&mut parsed.out, flag, value)?,
        "--node" => assign_once(&mut parsed.node, flag, value)?,
        "--easyconfigs" => parsed.easyconfigs.push(value),
        "--arch" => parsed.arches.push(value),
        "--class" => {
            let set = parsed.classes.get_or_insert_with(BTreeSet::new);
            let mut any = false;
            for part in value.split(',') {
                let class = part.trim();
                if class.is_empty() {
                    return Err("--class needs a class name".into());
                }
                set.insert(class.to_string());
                any = true;
            }
            if !any {
                return Err("--class needs a class name".into());
            }
        }
        "--status" => {
            let set = parsed.statuses.get_or_insert_with(BTreeSet::new);
            for part in value.split(',') {
                let s = part.trim();
                if s.is_empty() || !KNOWN_STATUSES.contains(&s) {
                    return Err(format!(
                        "unknown status `{s}`; expected one of: {}",
                        KNOWN_STATUSES.join(", ")
                    ));
                }
                set.insert(s.to_string());
            }
        }
        _ => unreachable!(),
    }
    Ok(())
}

/// `CORES:MEMORY_MIB` or `CORES:MEMORY_MIB:TARGET`, the form `stack oom --node` takes.
fn check_node(text: &str) -> Result<(), String> {
    let Some((cores, rest)) = text.split_once(':') else {
        return Err(format!(
            "--node {text} is CORES:MEMORY_MIB or CORES:MEMORY_MIB:TARGET"
        ));
    };
    let (memory, target) = match rest.split_once(':') {
        Some((memory, target)) => (memory, Some(target)),
        None => (rest, None),
    };
    let cores: u32 = cores
        .parse()
        .map_err(|_| format!("--node {text} cores are not an integer"))?;
    let memory_mib: u64 = memory
        .parse()
        .map_err(|_| format!("--node {text} memory is not an integer number of MiB"))?;
    if cores == 0 || memory_mib == 0 {
        return Err(format!("--node {text} needs positive cores and memory"));
    }
    if target.is_some_and(str::is_empty) {
        return Err(format!("--node {text} has an empty target"));
    }
    Ok(())
}

fn read_result(path: &Path) -> Result<RunOutcomes, String> {
    let (_, v) =
        exit::read_json_output(path, "eb-pipeline run", exit::USAGE).map_err(|error| error.msg)?;
    let obj = v
        .as_object()
        .ok_or_else(|| format!("{} must be a JSON object", path.display()))?;
    let kind = obj
        .get("kind")
        .and_then(Value::as_str)
        .ok_or_else(|| format!("{}: missing string `kind`", path.display()))?;
    if kind != "result" {
        return Err(format!(
            "{}: expected kind `result`, found `{kind}`",
            path.display()
        ));
    }
    let exit_code = obj
        .get("exit_code")
        .and_then(Value::as_i64)
        .ok_or_else(|| format!("{}: `exit_code` must be an integer", path.display()))?;
    let lines = obj
        .get("lines")
        .and_then(Value::as_array)
        .ok_or_else(|| format!("{}: `lines` must be an array", path.display()))?;
    let outcomes = collect_outcomes(lines, path, "lines")?;
    let mut deselected = BTreeMap::new();
    if let Some(arr) = obj.get("deselected") {
        let items = arr
            .as_array()
            .ok_or_else(|| format!("{}: `deselected` must be an array", path.display()))?;
        for (i, item) in items.iter().enumerate() {
            let ec = item
                .get("easyconfig")
                .and_then(Value::as_str)
                .ok_or_else(|| {
                    format!(
                        "{}: deselected[{i}].easyconfig must be a string",
                        path.display()
                    )
                })?;
            *deselected.entry(ec.to_string()).or_insert(0) += 1;
        }
    }
    Ok(RunOutcomes {
        exit_code,
        list: obj.get("list").and_then(Value::as_str).map(str::to_string),
        outcomes,
        deselected,
    })
}

fn read_summary(path: &Path, target: &str) -> Result<RunOutcomes, String> {
    let (_, v) = exit::read_json_output(path, "eb-pipeline status", exit::USAGE)
        .map_err(|error| error.msg)?;
    let obj = v
        .as_object()
        .ok_or_else(|| format!("{} must be a JSON object", path.display()))?;
    let kind = obj
        .get("kind")
        .and_then(Value::as_str)
        .ok_or_else(|| format!("{}: missing string `kind`", path.display()))?;
    if kind != "summary" {
        return Err(format!(
            "{}: expected kind `summary`, found `{kind}`",
            path.display()
        ));
    }
    let targets = obj
        .get("targets")
        .and_then(Value::as_array)
        .ok_or_else(|| format!("{}: `targets` must be an array", path.display()))?;
    let entry = targets
        .iter()
        .find(|t| t.get("name").and_then(Value::as_str) == Some(target))
        .ok_or_else(|| format!("{}: target `{target}` not found", path.display()))?;
    let t_obj = entry
        .as_object()
        .ok_or_else(|| format!("{}: target `{target}` must be an object", path.display()))?;
    let exit_code = t_obj
        .get("exit_code")
        .and_then(Value::as_i64)
        .ok_or_else(|| {
            format!(
                "{}: target `{target}` `exit_code` must be an integer",
                path.display()
            )
        })?;
    let packages = t_obj
        .get("packages")
        .and_then(Value::as_array)
        .ok_or_else(|| {
            format!(
                "{}: target `{target}` `packages` must be an array",
                path.display()
            )
        })?;
    let outcomes = collect_outcomes(packages, path, "packages")?;
    let mut deselected = BTreeMap::new();
    if let Some(arr) = t_obj.get("deselected") {
        let items = arr
            .as_array()
            .ok_or_else(|| format!("{}: `deselected` must be an array", path.display()))?;
        for (i, item) in items.iter().enumerate() {
            let ec = item
                .as_str()
                .ok_or_else(|| format!("{}: deselected[{i}] must be a string", path.display()))?;
            *deselected.entry(ec.to_string()).or_insert(0) += 1;
        }
    }
    Ok(RunOutcomes {
        exit_code,
        list: t_obj
            .get("list")
            .and_then(Value::as_str)
            .map(str::to_string),
        outcomes,
        deselected,
    })
}

fn collect_outcomes(
    items: &[Value],
    path: &Path,
    field: &str,
) -> Result<BTreeMap<String, VecDeque<Outcome>>, String> {
    let mut map: BTreeMap<String, VecDeque<Outcome>> = BTreeMap::new();
    for (i, item) in items.iter().enumerate() {
        let obj = item
            .as_object()
            .ok_or_else(|| format!("{}: {field}[{i}] must be an object", path.display()))?;
        let ec = obj
            .get("easyconfig")
            .and_then(Value::as_str)
            .ok_or_else(|| {
                format!(
                    "{}: {field}[{i}].easyconfig must be a string",
                    path.display()
                )
            })?;
        let status = obj
            .get("status")
            .and_then(Value::as_str)
            .ok_or_else(|| format!("{}: {field}[{i}].status must be a string", path.display()))?;
        let log = obj.get("log").and_then(Value::as_str).map(str::to_string);
        let resumed = ["note", "reason"].iter().any(|key| {
            obj.get(*key)
                .and_then(Value::as_str)
                .is_some_and(|note| note.starts_with("resumed:"))
        });
        map.entry(ec.to_string()).or_default().push_back(Outcome {
            status: status.to_string(),
            log,
            resumed,
        });
    }
    Ok(map)
}

struct RetryLine {
    raw: String,
    easyconfig: String,
    args: Vec<String>,
    log: Option<String>,
}

fn select_retry_lines(
    lines: &[ListLine],
    recorded: &RunOutcomes,
    explicit_statuses: Option<&BTreeSet<String>>,
) -> Result<Vec<RetryLine>, String> {
    let mut outcomes = recorded.outcomes.clone();
    let mut deselected = recorded.deselected.clone();
    let mut out = Vec::new();
    for line in lines {
        if let Some(outcome) = outcomes
            .get_mut(&line.easyconfig)
            .and_then(VecDeque::pop_front)
        {
            let keep = match explicit_statuses {
                Some(set) => set.contains(&outcome.status),
                None => outcome.status != "success" && !outcome.resumed,
            };
            if keep {
                out.push(RetryLine {
                    raw: line.raw.clone(),
                    easyconfig: line.easyconfig.clone(),
                    args: line.args.clone(),
                    log: outcome.log,
                });
            }
            continue;
        }
        if let Some(count) = deselected.get_mut(&line.easyconfig).filter(|c| **c > 0) {
            *count -= 1;
            continue;
        }
        if recorded.exit_code != 0 {
            let keep = match explicit_statuses {
                Some(set) => set.contains("unreached"),
                None => true,
            };
            if keep {
                out.push(RetryLine {
                    raw: line.raw.clone(),
                    easyconfig: line.easyconfig.clone(),
                    args: line.args.clone(),
                    log: None,
                });
            }
        }
    }
    if let Some((easyconfig, _)) = outcomes.iter().find(|(easyconfig, queue)| {
        !queue.is_empty()
            && lines.iter().any(|line| {
                line.easyconfig.as_str() != easyconfig.as_str()
                    && basename(&line.easyconfig) == basename(easyconfig)
            })
    }) {
        return Err(format!(
            "recorded `easyconfig` `{easyconfig}` has no matching build-list line"
        ));
    }
    Ok(out)
}

fn default_classes() -> BTreeSet<String> {
    RETRYABLE_CLASSES
        .iter()
        .map(|class| (*class).to_string())
        .collect()
}

fn classify_retry_lines(
    lines: Vec<RetryLine>,
    classes: &BTreeSet<String>,
) -> Result<Vec<RetryLine>, String> {
    let mut retryable = Vec::new();
    for line in lines {
        let log = line
            .log
            .as_deref()
            .ok_or_else(|| format!("cannot classify `{}`: no log was recorded", line.raw))?;
        let job = classify::classify_log(Path::new(log))?;
        if classes.contains(&job.class) || classes.contains(&job.sub_class) {
            retryable.push(line);
        }
    }
    Ok(retryable)
}

#[derive(Debug)]
struct OomHit {
    file: String,
    required_by: Vec<String>,
    line: String,
}

struct TempFile(PathBuf);

impl Drop for TempFile {
    fn drop(&mut self) {
        let _ = fs::remove_file(&self.0);
    }
}

/// Ask `stack oom` which of `lines` it warns about, and drop those when asked.
fn filter_oom(
    lines: Vec<RetryLine>,
    node: &str,
    drop: bool,
    easyconfigs: &[String],
    arches: &[String],
    list_source: &str,
) -> Result<Vec<String>, String> {
    if lines.is_empty() {
        return Ok(Vec::new());
    }
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or(0);
    let path = env::temp_dir().join(format!("eb-retry-list-{}-{nanos}.yml", std::process::id()));
    let temp = TempFile(path);
    write_easystack(&temp.0, &lines)?;
    let mut command = Command::new("eb-stack");
    command
        .args(["stack", "oom", "--node", node, "--easystack"])
        .arg(&temp.0);
    for dir in easyconfigs {
        command.arg("--easyconfigs").arg(dir);
    }
    for arch in arches {
        command.arg("--arch").arg(arch);
    }
    let output = command
        .stdin(Stdio::null())
        .output()
        .map_err(|e| format!("cannot run eb-stack stack oom: {e}"))?;
    if !output.status.success() {
        let code = output
            .status
            .code()
            .map_or_else(|| output.status.to_string(), |code| code.to_string());
        let stderr = String::from_utf8_lossy(&output.stderr);
        let generated = temp.0.to_string_lossy();
        let source = format!("generated retry easystack from {list_source}");
        let detail = stderr.trim().replace(generated.as_ref(), &source);
        let detail = if detail.is_empty() {
            String::new()
        } else {
            format!(": {detail}")
        };
        return Err(format!("eb-stack stack oom exited {code}{detail}"));
    }
    let stdout = String::from_utf8_lossy(&output.stdout);
    let hits = parse_oom_report(&stdout)?;
    for hit in &hits {
        if !lines.iter().any(|line| hit_names(hit, line)) {
            return Err(format!(
                "eb-stack stack oom warned about `{}`, which is not a selected line",
                hit.file
            ));
        }
    }
    for hit in &hits {
        eprintln!("eb-retry-list: {}", hit.line);
    }
    Ok(lines
        .into_iter()
        .filter(|line| !(drop && hits.iter().any(|hit| hit_names(hit, line))))
        .map(|line| line.raw)
        .collect())
}

fn write_easystack(path: &Path, lines: &[RetryLine]) -> Result<(), String> {
    let mut seen = BTreeMap::new();
    let mut easyconfigs = Vec::new();
    for line in lines {
        let name = basename(&line.easyconfig).to_string();
        if let Some(previous) = seen.get(&name) {
            if previous != &line.raw {
                return Err(format!(
                    "cannot check `{name}` for memory with more than one set of eb options"
                ));
            }
            continue;
        }
        seen.insert(name.clone(), line.raw.clone());
        easyconfigs.push(easystack_entry(&name, &line.args)?);
    }
    let mut document = serde_yaml::Mapping::new();
    document.insert(
        serde_yaml::Value::String("easyconfigs".into()),
        serde_yaml::Value::Sequence(easyconfigs),
    );
    let value = serde_yaml::Value::Mapping(document);
    let text = serde_yaml::to_string(&value).map_err(|e| format!("cannot make easystack: {e}"))?;
    fs::write(path, text).map_err(|e| format!("cannot write {}: {e}", path.display()))
}

fn easystack_entry(name: &str, args: &[String]) -> Result<serde_yaml::Value, String> {
    if args.is_empty() {
        return Ok(serde_yaml::Value::String(name.to_string()));
    }
    let mut options = serde_yaml::Mapping::new();
    let mut index = 0;
    while index < args.len() {
        let argument = &args[index];
        let option = if let Some(long) = argument.strip_prefix("--") {
            long
        } else if let Some(short) = argument.strip_prefix('-') {
            let key = short.split_once('=').map_or(short, |(key, _)| key);
            if key.chars().count() != 1 {
                return Err(format!(
                    "cannot represent eb option `{argument}` in the generated easystack"
                ));
            }
            short
        } else {
            return Err(format!(
                "cannot represent eb option `{argument}` in the generated easystack"
            ));
        };
        let (key, value, consumed) = if let Some((key, value)) = option.split_once('=') {
            (key, serde_yaml::Value::String(value.to_string()), 1)
        } else if args
            .get(index + 1)
            .is_some_and(|value| !value.starts_with('-'))
        {
            (
                option,
                serde_yaml::Value::String(args[index + 1].clone()),
                2,
            )
        } else {
            (option, serde_yaml::Value::Bool(true), 1)
        };
        if key.is_empty() {
            return Err("cannot represent an empty eb option in the generated easystack".into());
        }
        let option_key = serde_yaml::Value::String(key.to_string());
        if let Some(previous) = options.remove(&option_key) {
            if key != "amend" && key != "try-amend" {
                return Err(format!(
                    "cannot represent repeated eb option `--{key}` in the generated easystack"
                ));
            }
            let mut values = match previous {
                serde_yaml::Value::Sequence(values) => values,
                previous => vec![previous],
            };
            values.push(value);
            options.insert(option_key, serde_yaml::Value::Sequence(values));
        } else {
            options.insert(option_key, value);
        }
        index += consumed;
    }
    let mut spec = serde_yaml::Mapping::new();
    spec.insert(
        serde_yaml::Value::String("options".into()),
        serde_yaml::Value::Mapping(options),
    );
    let mut entry = serde_yaml::Mapping::new();
    entry.insert(
        serde_yaml::Value::String(name.to_string()),
        serde_yaml::Value::Mapping(spec),
    );
    Ok(serde_yaml::Value::Mapping(entry))
}

fn parse_oom_report(stdout: &str) -> Result<Vec<OomHit>, String> {
    let mut count: Option<usize> = None;
    let mut hits = Vec::new();
    for line in stdout.lines() {
        if let Some(raw) = line
            .split_whitespace()
            .next()
            .and_then(|word| word.strip_prefix("oom="))
        {
            if count.is_some() {
                return Err("eb-stack stack oom printed more than one `oom=N` line".into());
            }
            let parsed = raw
                .parse()
                .map_err(|_| format!("eb-stack stack oom printed an invalid count `oom={raw}`"))?;
            count = Some(parsed);
            continue;
        }
        if count.is_some() {
            continue;
        }
        let Some(at) = line.find(OOM_MARK) else {
            continue;
        };
        let file = line[..at].trim();
        if file.is_empty() {
            return Err("eb-stack stack oom printed a warning with no easyconfig".into());
        }
        hits.push(OomHit {
            file: file.to_string(),
            required_by: required_by(line),
            line: line.to_string(),
        });
    }
    let Some(oom) = count else {
        return Err("eb-stack stack oom printed no `oom=N` line".into());
    };
    if hits.len() != oom {
        return Err(format!(
            "eb-stack stack oom printed oom={oom} but {} warning line(s)",
            hits.len()
        ));
    }
    Ok(hits)
}

fn required_by(line: &str) -> Vec<String> {
    let Some((_, rest)) = line.rsplit_once(" (required by ") else {
        return Vec::new();
    };
    let names = rest.strip_suffix(')').unwrap_or(rest);
    names
        .split(',')
        .map(str::trim)
        .filter(|name| !name.is_empty())
        .map(str::to_string)
        .collect()
}

fn hit_names(hit: &OomHit, line: &RetryLine) -> bool {
    let file = basename(&line.easyconfig);
    basename(&hit.file) == file || hit.required_by.iter().any(|name| basename(name) == file)
}

fn basename(path: &str) -> &str {
    path.rsplit(['/', '\\']).next().unwrap_or(path)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn line(ec: &str, raw: &str) -> ListLine {
        ListLine {
            raw: raw.into(),
            easyconfig: ec.into(),
            args: Vec::new(),
        }
    }

    #[test]
    fn default_statuses_keep_every_unfinished_line_and_drop_success() {
        let mut outcomes = BTreeMap::new();
        outcomes.insert("A.eb".into(), VecDeque::from([outcome("success")]));
        outcomes.insert("B.eb".into(), VecDeque::from([outcome("failed")]));
        outcomes.insert("C.eb".into(), VecDeque::from([outcome("refused")]));
        let mut deselected = BTreeMap::new();
        deselected.insert("Skip.eb".into(), 1);
        let rec = RunOutcomes {
            exit_code: 1,
            list: None,
            outcomes,
            deselected,
        };
        let lines = vec![
            line("A.eb", "A.eb --robot"),
            line("Skip.eb", "Skip.eb --only-targets=gpu"),
            line("B.eb", "B.eb --rebuild"),
            line("C.eb", "C.eb"),
            line("D.eb", "D.eb --from-pr=9"),
        ];
        assert_eq!(
            select_retry_lines(&lines, &rec, None)
                .unwrap()
                .into_iter()
                .map(|line| line.raw)
                .collect::<Vec<_>>(),
            vec!["B.eb --rebuild", "C.eb", "D.eb --from-pr=9"]
        );
    }

    #[test]
    fn explicit_status_filters_to_that_status_only() {
        let mut outcomes = BTreeMap::new();
        outcomes.insert("A.eb".into(), VecDeque::from([outcome("failed")]));
        outcomes.insert("B.eb".into(), VecDeque::from([outcome("skipped")]));
        let rec = RunOutcomes {
            exit_code: 1,
            list: None,
            outcomes,
            deselected: BTreeMap::new(),
        };
        let lines = vec![line("A.eb", "A.eb"), line("B.eb", "B.eb")];
        let only_failed = BTreeSet::from(["failed".into()]);
        assert_eq!(
            select_retry_lines(&lines, &rec, Some(&only_failed))
                .unwrap()
                .into_iter()
                .map(|line| line.raw)
                .collect::<Vec<_>>(),
            vec!["A.eb"]
        );
    }

    #[test]
    fn oom_report_names_the_easyconfig_and_who_requires_it() {
        let stdout = "\
Big.eb may run out of memory on x86_64/amd/zen4 (label): 10.0 GiB, the job has 1.0 GiB
  parallel 8: 8 cores
  remedy: fewer cores
Dep.eb may run out of memory on x86_64/amd/zen4 (label): 9.0 GiB, the job has 1.0 GiB (required by Parent.eb, Other.eb)
oom=2 fits=1 builds=3 nodes=1

  libxc: current 4 (hook)
";
        let hits = parse_oom_report(stdout).unwrap();
        assert_eq!(hits.len(), 2);
        assert_eq!(hits[0].file, "Big.eb");
        assert!(hits[0].required_by.is_empty());
        assert_eq!(hits[1].required_by, vec!["Parent.eb", "Other.eb"]);
    }

    #[test]
    fn oom_report_refuses_a_count_that_does_not_match_the_warnings() {
        let err = parse_oom_report("oom=1 fits=0 builds=1 nodes=1\n").unwrap_err();
        assert!(err.contains("oom=1 but 0 warning"), "{err}");
        let err = parse_oom_report("no summary\n").unwrap_err();
        assert!(err.contains("no `oom=N` line"), "{err}");
    }

    #[test]
    fn node_form_matches_the_eb_stack_flag() {
        assert!(check_node("32:65536").is_ok());
        assert!(check_node("32:65536:x86_64/amd/zen4").is_ok());
        assert!(check_node("cores:65536").is_err());
        assert!(check_node("0:65536").is_err());
        assert!(check_node("32:65536:").is_err());
        assert!(check_node("32").is_err());
    }

    #[test]
    fn generated_easystacks_keep_short_easybuild_options() {
        let entry = easystack_entry("Big-1.0.eb", &["-f".into()]).unwrap();
        let document = serde_yaml::Value::Mapping(serde_yaml::Mapping::from_iter([(
            serde_yaml::Value::String("easyconfigs".into()),
            serde_yaml::Value::Sequence(vec![entry]),
        )]));
        let text = serde_yaml::to_string(&document).unwrap();
        let lines = easystack::parse(&text).unwrap();
        assert_eq!(lines[0].args, ["-f"]);
    }

    #[test]
    fn generated_easystacks_keep_repeated_amend_options() {
        let entry = easystack_entry(
            "Big-1.0.eb",
            &[
                "--try-amend=versionsuffix=-a".into(),
                "--try-amend".into(),
                "toolchain=foss,2025a".into(),
            ],
        )
        .unwrap();
        let document = serde_yaml::Value::Mapping(serde_yaml::Mapping::from_iter([(
            serde_yaml::Value::String("easyconfigs".into()),
            serde_yaml::Value::Sequence(vec![entry]),
        )]));
        let text = serde_yaml::to_string(&document).unwrap();
        let lines = easystack::parse(&text).unwrap();
        assert_eq!(
            lines[0].args,
            [
                "--try-amend=versionsuffix=-a",
                "--try-amend=toolchain=foss,2025a"
            ]
        );
    }

    fn outcome(status: &str) -> Outcome {
        Outcome {
            status: status.into(),
            log: None,
            resumed: false,
        }
    }
}
