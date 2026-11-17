//! Extract the unfinished lines of a build list from a run result or summary.
//!
//! Preserves original list order and per-line `eb` options (including options
//! parsed from an easystack), leaving out `success` and `deselected` entries.

use eb_pipeline::easystack;
use eb_pipeline::util::{self, ListLine};
use serde_json::Value;
use std::collections::{BTreeMap, BTreeSet, VecDeque};
use std::env;
use std::fs;
use std::io::{self, Write};
use std::path::Path;
use std::process::ExitCode;

const KNOWN_STATUSES: &[&str] = &["failed", "refused", "pending", "skipped", "unreached"];

struct Args {
    result: Option<String>,
    summary: Option<String>,
    target: Option<String>,
    list: String,
    statuses: Option<BTreeSet<String>>,
    out: Option<String>,
}

struct RunOutcomes {
    exit_code: i64,
    outcomes: BTreeMap<String, VecDeque<String>>,
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
    let list_path = Path::new(&args.list);
    let list_text = fs::read_to_string(list_path)
        .map_err(|e| format!("cannot read list {}: {e}", args.list))?;
    let lines = if easystack::is_easystack(list_path) {
        easystack::parse(&list_text).map_err(|e| format!("easystack {}: {e}", args.list))?
    } else {
        util::parse_list(&list_text).map_err(|f| f.msg)?
    };
    if lines.is_empty() {
        return Err(format!("list {} holds no lines", args.list));
    }

    let recorded = match (&args.result, &args.summary, &args.target) {
        (Some(path), None, None) => read_result(Path::new(path))?,
        (None, Some(path), Some(target)) => read_summary(Path::new(path), target)?,
        _ => unreachable!("validated in parse"),
    };

    let selected = select_retry_lines(&lines, &recorded, args.statuses.as_ref());
    let mut text = selected.join("\n");
    if !selected.is_empty() {
        text.push('\n');
    }
    match args.out.as_deref() {
        Some(path) if path != "-" => {
            fs::write(path, &text).map_err(|e| format!("cannot write {path}: {e}"))?;
        }
        _ => {
            let mut out = io::stdout().lock();
            out.write_all(text.as_bytes())
                .map_err(|e| format!("cannot write retry list: {e}"))?;
        }
    }
    Ok(ExitCode::SUCCESS)
}

fn parse(words: impl Iterator<Item = String>) -> Result<Args, String> {
    let mut result = None;
    let mut summary = None;
    let mut target = None;
    let mut list = None;
    let mut statuses: Option<BTreeSet<String>> = None;
    let mut out = None;
    let mut words = words.peekable();

    while let Some(word) = words.next() {
        match word.as_str() {
            "-h" | "--help" => {
                print!("{}", include_str!("eb-retry-list-help.txt"));
                std::process::exit(0);
            }
            "-V" | "--version" => {
                println!("eb-retry-list {}", env!("CARGO_PKG_VERSION"));
                std::process::exit(0);
            }
            "--result" | "--summary" | "--target" | "--list" | "--status" | "--out" => {
                let value = words
                    .next()
                    .filter(|v| !v.is_empty())
                    .ok_or_else(|| format!("{word} needs a value"))?;
                assign_flag(
                    &word,
                    value,
                    &mut result,
                    &mut summary,
                    &mut target,
                    &mut list,
                    &mut statuses,
                    &mut out,
                )?;
            }
            _ => {
                if let Some((flag, value)) = word.split_once('=') {
                    if matches!(
                        flag,
                        "--result" | "--summary" | "--target" | "--list" | "--status" | "--out"
                    ) {
                        if value.is_empty() {
                            return Err(format!("{flag} needs a value"));
                        }
                        assign_flag(
                            flag,
                            value.to_string(),
                            &mut result,
                            &mut summary,
                            &mut target,
                            &mut list,
                            &mut statuses,
                            &mut out,
                        )?;
                        continue;
                    }
                }
                return Err(format!("does not know `{word}`"));
            }
        }
    }

    if result.is_some() && summary.is_some() {
        return Err("pass either --result or --summary, not both".into());
    }
    if result.is_none() && summary.is_none() {
        return Err("--result or --summary is required".into());
    }
    if result.is_some() && target.is_some() {
        return Err("--target is used with --summary, not --result".into());
    }
    if summary.is_some() && target.is_none() {
        return Err("--target is required with --summary".into());
    }
    Ok(Args {
        result,
        summary,
        target,
        list: list.ok_or_else(|| "--list is required".to_string())?,
        statuses,
        out,
    })
}

#[allow(clippy::too_many_arguments)]
fn assign_flag(
    flag: &str,
    value: String,
    result: &mut Option<String>,
    summary: &mut Option<String>,
    target: &mut Option<String>,
    list: &mut Option<String>,
    statuses: &mut Option<BTreeSet<String>>,
    out: &mut Option<String>,
) -> Result<(), String> {
    match flag {
        "--result" => *result = Some(value),
        "--summary" => *summary = Some(value),
        "--target" => *target = Some(value),
        "--list" => *list = Some(value),
        "--out" => *out = Some(value),
        "--status" => {
            let set = statuses.get_or_insert_with(BTreeSet::new);
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

fn read_json(path: &Path) -> Result<Value, String> {
    let text =
        fs::read_to_string(path).map_err(|e| format!("cannot read {}: {e}", path.display()))?;
    serde_json::from_str(&text).map_err(|e| format!("cannot parse {}: {e}", path.display()))
}

fn read_result(path: &Path) -> Result<RunOutcomes, String> {
    let v = read_json(path)?;
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
        outcomes,
        deselected,
    })
}

fn read_summary(path: &Path, target: &str) -> Result<RunOutcomes, String> {
    let v = read_json(path)?;
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
        outcomes,
        deselected,
    })
}

fn collect_outcomes(
    items: &[Value],
    path: &Path,
    field: &str,
) -> Result<BTreeMap<String, VecDeque<String>>, String> {
    let mut map: BTreeMap<String, VecDeque<String>> = BTreeMap::new();
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
        map.entry(ec.to_string())
            .or_default()
            .push_back(status.to_string());
    }
    Ok(map)
}

fn select_retry_lines(
    lines: &[ListLine],
    recorded: &RunOutcomes,
    explicit_statuses: Option<&BTreeSet<String>>,
) -> Vec<String> {
    let mut outcomes = recorded.outcomes.clone();
    let mut deselected = recorded.deselected.clone();
    let mut out = Vec::new();
    for line in lines {
        if let Some(status) = outcomes
            .get_mut(&line.easyconfig)
            .and_then(VecDeque::pop_front)
        {
            let keep = match explicit_statuses {
                Some(set) => set.contains(&status),
                None => status != "success",
            };
            if keep {
                out.push(line.raw.clone());
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
                out.push(line.raw.clone());
            }
        }
    }
    out
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
        outcomes.insert("A.eb".into(), VecDeque::from(["success".into()]));
        outcomes.insert("B.eb".into(), VecDeque::from(["failed".into()]));
        outcomes.insert("C.eb".into(), VecDeque::from(["refused".into()]));
        let mut deselected = BTreeMap::new();
        deselected.insert("Skip.eb".into(), 1);
        let rec = RunOutcomes {
            exit_code: 1,
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
            select_retry_lines(&lines, &rec, None),
            vec!["B.eb --rebuild", "C.eb", "D.eb --from-pr=9"]
        );
    }

    #[test]
    fn explicit_status_filters_to_that_status_only() {
        let mut outcomes = BTreeMap::new();
        outcomes.insert("A.eb".into(), VecDeque::from(["failed".into()]));
        outcomes.insert("B.eb".into(), VecDeque::from(["skipped".into()]));
        let rec = RunOutcomes {
            exit_code: 1,
            outcomes,
            deselected: BTreeMap::new(),
        };
        let lines = vec![line("A.eb", "A.eb"), line("B.eb", "B.eb")];
        let only_failed = BTreeSet::from(["failed".into()]);
        assert_eq!(
            select_retry_lines(&lines, &rec, Some(&only_failed)),
            vec!["A.eb"]
        );
    }
}
