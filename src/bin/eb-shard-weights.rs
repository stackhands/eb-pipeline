//! Build an `eb-stack stack shard --weights` TSV from observed `eb-pipeline` run durations.
//!
//! Reads succeeded `build` line durations from `result.json`, `summary.json`,
//! or recent run directories under `--log-root`, optionally merges an existing
//! weights table, and writes deterministic `MODULE<TAB>SECONDS` lines.

use eb_pipeline::plan::module_rel;
use eb_pipeline::summary::{self, Summary};
use eb_pipeline::{exit, util};
use serde_json::Value;
use std::collections::BTreeMap;
use std::ffi::OsString;
use std::path::{Path, PathBuf};
use std::process::ExitCode;

#[derive(Clone, Copy, PartialEq, Eq)]
enum Mode {
    Max,
    Latest,
    Mean,
}

struct Args {
    results: Vec<PathBuf>,
    summaries: Vec<PathBuf>,
    log_root: Option<PathBuf>,
    last: Option<usize>,
    target: Option<String>,
    existing: Option<PathBuf>,
    min_samples: usize,
    mode: Mode,
    out: Option<PathBuf>,
}

struct Observation {
    module: String,
    seconds: f64,
    ended_at: String,
    seq: usize,
}

fn main() -> ExitCode {
    match run() {
        Ok(code) => code,
        Err(msg) => {
            util::write_stderr_line(&format!("eb-shard-weights: {msg}"));
            ExitCode::from(2)
        }
    }
}

fn run() -> Result<ExitCode, String> {
    let args = parse(util::os_args().into_iter())?;
    let inputs: Vec<_> = args
        .results
        .iter()
        .map(|path| ("--result", Some(path.as_path())))
        .chain(
            args.summaries
                .iter()
                .map(|path| ("--summary", Some(path.as_path()))),
        )
        .chain(
            args.existing
                .iter()
                .map(|path| ("--existing", Some(path.as_path()))),
        )
        .collect();
    util::refuse_output_alias("--out", args.out.as_deref(), &inputs)?;
    let mut weights = match args.existing.as_deref() {
        Some(path) => read_existing(path)?,
        None => BTreeMap::new(),
    };

    let mut observations = Vec::new();
    let mut seq = 0usize;
    let mut target_matched = args.target.is_none();
    for path in &args.results {
        target_matched |=
            collect_result(path, args.target.as_deref(), &mut observations, &mut seq)?;
    }
    for path in &args.summaries {
        let (_, value) = exit::read_json_output(path, "eb-pipeline run", exit::USAGE)
            .map_err(|error| error.msg)?;
        let s = parse_summary_json(value, path)?;
        target_matched |= collect_summary(
            &s,
            path,
            args.target.as_deref(),
            &mut observations,
            &mut seq,
        )?;
    }
    if let Some(root) = args.log_root.as_deref() {
        let last = args.last.unwrap_or(10);
        let entries = summary::runs(root, args.target.as_deref(), last)?;
        // `summary::runs` returns newest first; collect chronologically so `latest`
        // falls back cleanly when timestamps match.
        for entry in entries.into_iter().rev() {
            let path = entry.dir.join("summary.json");
            match entry.summary {
                Ok(s) => {
                    target_matched |= collect_summary(
                        &s,
                        &path,
                        args.target.as_deref(),
                        &mut observations,
                        &mut seq,
                    )?;
                }
                Err(error) if path.exists() => return Err(error),
                Err(_) => {}
            }
        }
    }
    if !target_matched {
        return Err(format!(
            "no input data matched target `{}`",
            args.target.as_deref().unwrap_or_default()
        ));
    }
    if args.min_samples > 1 {
        let mut counts = BTreeMap::new();
        for observation in &observations {
            *counts.entry(observation.module.clone()).or_insert(0usize) += 1;
        }
        observations.retain(|observation| {
            counts
                .get(&observation.module)
                .is_some_and(|count| *count >= args.min_samples)
        });
    }

    merge_weights(&mut weights, &observations, args.mode);

    let mut text = String::new();
    for (module, secs) in &weights {
        text.push_str(&format!("{module}\t{secs}\n"));
    }
    match args.out.as_deref() {
        Some(path) if path != Path::new("-") => {
            util::write_atomic(path, text.as_bytes())
                .map_err(|e| format!("cannot write {}: {e}", path.display()))?;
        }
        _ => {
            util::write_stdout_checked(text.as_bytes())?;
        }
    }
    Ok(ExitCode::SUCCESS)
}

fn parse(words: impl Iterator<Item = OsString>) -> Result<Args, String> {
    let mut results = Vec::new();
    let mut summaries = Vec::new();
    let mut log_root = None;
    let mut last: Option<usize> = None;
    let mut target = None;
    let mut existing = None;
    let mut min_samples: Option<usize> = None;
    let mut mode: Option<Mode> = None;
    let mut out = None;
    let mut words = words.peekable();

    while let Some(word) = words.next() {
        if let Some((flag, value)) = util::split_os_flag_value(word.as_os_str()) {
            if matches!(
                flag.as_str(),
                "--result"
                    | "--summary"
                    | "--log-root"
                    | "--last"
                    | "--target"
                    | "--existing"
                    | "--min-samples"
                    | "--mode"
                    | "--out"
            ) {
                if value.as_os_str().is_empty() {
                    return Err(format!("{flag} needs a value"));
                }
                assign_flag(
                    &flag,
                    value,
                    &mut results,
                    &mut summaries,
                    &mut log_root,
                    &mut last,
                    &mut target,
                    &mut existing,
                    &mut min_samples,
                    &mut mode,
                    &mut out,
                )?;
                continue;
            }
        }
        let word = util::os_to_string(word)?;
        match word.as_str() {
            "-h" | "--help" => {
                util::write_stdout_checked(include_str!("eb-shard-weights-help.txt").as_bytes())?;
                std::process::exit(0);
            }
            "-V" | "--version" => {
                let version = format!("eb-shard-weights {}\n", env!("CARGO_PKG_VERSION"));
                util::write_stdout_checked(version.as_bytes())?;
                std::process::exit(0);
            }
            "--result" | "--summary" | "--log-root" | "--last" | "--target" | "--existing"
            | "--min-samples" | "--mode" | "--out" => {
                let value = util::separate_os_value(&mut words, &format!("{word} needs a value"))?;
                assign_flag(
                    &word,
                    value,
                    &mut results,
                    &mut summaries,
                    &mut log_root,
                    &mut last,
                    &mut target,
                    &mut existing,
                    &mut min_samples,
                    &mut mode,
                    &mut out,
                )?;
            }
            _ => return Err(format!("does not know `{word}`")),
        }
    }

    if results.is_empty() && summaries.is_empty() && log_root.is_none() {
        if let Some(result) = eb_pipeline::util::env_value("EB_PIPELINE_RESULT") {
            results.push(PathBuf::from(result));
        }
    }
    if target.is_none() {
        target = eb_pipeline::util::env_value("EB_PIPELINE_TARGET");
    }

    if results.is_empty() && summaries.is_empty() && log_root.is_none() {
        return Err("--result, --summary or --log-root is required".into());
    }
    if last.is_some() && log_root.is_none() {
        return Err("--last requires --log-root".into());
    }
    Ok(Args {
        results,
        summaries,
        log_root,
        last,
        target,
        existing,
        min_samples: min_samples.unwrap_or(1),
        mode: mode.unwrap_or(Mode::Max),
        out,
    })
}

fn assign_once<T>(slot: &mut Option<T>, flag: &str, value: T) -> Result<(), String> {
    if slot.is_some() {
        return Err(format!("{flag} was given twice"));
    }
    *slot = Some(value);
    Ok(())
}

#[allow(clippy::too_many_arguments)]
fn assign_flag(
    flag: &str,
    value: OsString,
    results: &mut Vec<PathBuf>,
    summaries: &mut Vec<PathBuf>,
    log_root: &mut Option<PathBuf>,
    last: &mut Option<usize>,
    target: &mut Option<String>,
    existing: &mut Option<PathBuf>,
    min_samples: &mut Option<usize>,
    mode: &mut Option<Mode>,
    out: &mut Option<PathBuf>,
) -> Result<(), String> {
    match flag {
        "--result" => results.push(PathBuf::from(value)),
        "--summary" => summaries.push(PathBuf::from(value)),
        "--log-root" => assign_once(log_root, flag, PathBuf::from(value))?,
        "--target" => assign_once(target, flag, util::os_to_string(value)?)?,
        "--existing" => assign_once(existing, flag, PathBuf::from(value))?,
        "--min-samples" => {
            if min_samples.is_some() {
                return Err(format!("{flag} was given twice"));
            }
            let value = util::os_to_string(value)?;
            let n: u16 = value.parse().map_err(|_| {
                format!("--min-samples `{value}` must be an integer from 1 to 65535")
            })?;
            if n == 0 {
                return Err("--min-samples must be at least 1".into());
            }
            *min_samples = Some(usize::from(n));
        }
        "--out" => assign_once(out, flag, PathBuf::from(value))?,
        "--last" => {
            if last.is_some() {
                return Err(format!("{flag} was given twice"));
            }
            let value = util::os_to_string(value)?;
            let n: u16 = value
                .parse()
                .map_err(|_| format!("--last `{value}` must be an integer from 1 to 65535"))?;
            if n == 0 {
                return Err("--last must be at least 1".into());
            }
            *last = Some(usize::from(n));
        }
        "--mode" => {
            if mode.is_some() {
                return Err(format!("{flag} was given twice"));
            }
            let value = util::os_to_string(value)?;
            let parsed = match value.as_str() {
                "max" => Mode::Max,
                "latest" => Mode::Latest,
                "mean" => Mode::Mean,
                _ => {
                    return Err(format!(
                        "unknown mode `{value}`; expected max, latest or mean"
                    ))
                }
            };
            *mode = Some(parsed);
        }
        _ => unreachable!(),
    }
    Ok(())
}

fn read_existing(path: &Path) -> Result<BTreeMap<String, u64>, String> {
    let text =
        util::read_text_file(path).map_err(|e| format!("cannot read {}: {e}", path.display()))?;
    let mut out = BTreeMap::new();
    let mut first_lines = BTreeMap::new();
    for (i, raw) in text.lines().enumerate() {
        let line = raw.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        let (module, value) = line
            .split_once('\t')
            .or_else(|| line.split_once(char::is_whitespace))
            .ok_or_else(|| {
                format!(
                    "{} line {}: expected module<TAB>seconds, got `{line}`",
                    path.display(),
                    i + 1
                )
            })?;
        let secs: f64 = value.trim().parse().map_err(|_| {
            format!(
                "{} line {}: `{}` is not a number of seconds",
                path.display(),
                i + 1,
                value.trim()
            )
        })?;
        if util::checked_duration_seconds(secs, None).is_err() {
            return Err(format!(
                "{} line {}: `{}` is not a number of seconds",
                path.display(),
                i + 1,
                value.trim()
            ));
        }
        let module = module.trim();
        if let Some(first) = first_lines.insert(module.to_string(), i + 1) {
            return Err(format!(
                "{} line {}: module `{module}` was already set on line {first}",
                path.display(),
                i + 1
            ));
        }
        out.insert(module.to_string(), ceil_seconds(secs));
    }
    Ok(out)
}

fn collect_result(
    path: &Path,
    target_filter: Option<&str>,
    out: &mut Vec<Observation>,
    seq: &mut usize,
) -> Result<bool, String> {
    let max_weight_seconds = u64::MAX as f64;
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
    if let Some(want) = target_filter {
        let got = obj
            .get("target")
            .and_then(Value::as_str)
            .ok_or_else(|| format!("{}: missing string `target`", path.display()))?;
        if got != want {
            return Ok(false);
        }
    }
    let ended_at = obj
        .get("ended_at")
        .and_then(Value::as_str)
        .unwrap_or("")
        .to_string();
    let lines = obj
        .get("lines")
        .and_then(Value::as_array)
        .ok_or_else(|| format!("{}: `lines` must be an array", path.display()))?;
    for (i, item) in lines.iter().enumerate() {
        let line = item
            .as_object()
            .ok_or_else(|| format!("{}: lines[{i}] must be an object", path.display()))?;
        let phase = line.get("phase").and_then(Value::as_str).unwrap_or("");
        let status = line.get("status").and_then(Value::as_str).unwrap_or("");
        if phase != "build" || status != "success" {
            continue;
        }
        let secs = line
            .get("duration_seconds")
            .and_then(Value::as_f64)
            .ok_or_else(|| {
                format!(
                    "{}: lines[{i}].duration_seconds must be a non-negative number",
                    path.display()
                )
            })?;
        match util::checked_duration_seconds(secs, Some(max_weight_seconds)) {
            Ok(_) => {}
            Err(util::DurationError::Invalid) => {
                return Err(format!(
                    "{}: lines[{i}].duration_seconds must be a non-negative number",
                    path.display()
                ));
            }
            Err(util::DurationError::TooLarge) => {
                return Err(format!(
                    "{}: lines[{i}].duration_seconds exceeds {} seconds",
                    path.display(),
                    u64::MAX
                ));
            }
        }
        let ec = line
            .get("easyconfig")
            .and_then(Value::as_str)
            .ok_or_else(|| format!("{}: lines[{i}].easyconfig must be a string", path.display()))?;
        let module = module_key(None, ec);
        *seq += 1;
        out.push(Observation {
            module,
            seconds: secs,
            ended_at: ended_at.clone(),
            seq: *seq,
        });
    }
    Ok(true)
}

fn parse_summary_json(v: Value, path: &Path) -> Result<Summary, String> {
    let kind = v
        .get("kind")
        .and_then(Value::as_str)
        .ok_or_else(|| format!("{}: missing string `kind`", path.display()))?;
    if kind != "summary" {
        return Err(format!(
            "{}: expected kind `summary`, found `{kind}`",
            path.display()
        ));
    }
    serde_json::from_value(v).map_err(|e| format!("{}: {e}", path.display()))
}

fn collect_summary(
    s: &Summary,
    path: &Path,
    target_filter: Option<&str>,
    out: &mut Vec<Observation>,
    seq: &mut usize,
) -> Result<bool, String> {
    let max_weight_seconds = u64::MAX as f64;
    let mut matched = target_filter.is_none();
    for (target_index, t) in s.targets.iter().enumerate() {
        if let Some(want) = target_filter {
            if t.name != want {
                continue;
            }
            matched = true;
        }
        for (package_index, p) in t.packages.iter().enumerate() {
            if p.phase != "build" || p.status != "success" {
                continue;
            }
            let secs = p.duration_seconds.ok_or_else(|| {
                format!(
                    "{}: targets[{target_index}].packages[{package_index}].duration_seconds must be a non-negative number",
                    path.display()
                )
            })?;
            match util::checked_duration_seconds(secs, Some(max_weight_seconds)) {
                Ok(_) => {}
                Err(util::DurationError::Invalid) => {
                    return Err(format!(
                        "{}: targets[{target_index}].packages[{package_index}].duration_seconds must be a non-negative number",
                        path.display()
                    ));
                }
                Err(util::DurationError::TooLarge) => {
                    return Err(format!(
                        "{}: targets[{target_index}].packages[{package_index}].duration_seconds exceeds {} seconds",
                        path.display(),
                        u64::MAX
                    ));
                }
            }
            let module = module_key(p.module.as_deref(), &p.easyconfig);
            *seq += 1;
            out.push(Observation {
                module,
                seconds: secs,
                ended_at: t.ended_at.clone(),
                seq: *seq,
            });
        }
    }
    Ok(matched)
}

fn module_key(module: Option<&str>, easyconfig: &str) -> String {
    if let Some(m) = module.filter(|m| !m.is_empty()) {
        return m.to_string();
    }
    if let Some(m) = module_rel(easyconfig) {
        return m;
    }
    easyconfig
        .rsplit('/')
        .next()
        .unwrap_or(easyconfig)
        .to_string()
}

fn ceil_seconds(secs: f64) -> u64 {
    (secs.ceil() as u64).max(1)
}

fn merge_weights(weights: &mut BTreeMap<String, u64>, obs: &[Observation], mode: Mode) {
    match mode {
        Mode::Max => {
            for o in obs {
                let w = ceil_seconds(o.seconds);
                weights
                    .entry(o.module.clone())
                    .and_modify(|cur| *cur = (*cur).max(w))
                    .or_insert(w);
            }
        }
        Mode::Latest => {
            let mut newest: BTreeMap<&str, (&str, usize, u64)> = BTreeMap::new();
            for o in obs {
                let w = ceil_seconds(o.seconds);
                let replace = match newest.get(o.module.as_str()) {
                    None => true,
                    Some((ts, prev_seq, _)) => (o.ended_at.as_str(), o.seq) >= (*ts, *prev_seq),
                };
                if replace {
                    newest.insert(o.module.as_str(), (o.ended_at.as_str(), o.seq, w));
                }
            }
            for (module, (_, _, w)) in newest {
                weights.insert(module.to_string(), w);
            }
        }
        Mode::Mean => {
            let mut sum: BTreeMap<&str, (f64, usize)> = BTreeMap::new();
            for o in obs {
                let entry = sum.entry(o.module.as_str()).or_insert((0.0, 0));
                entry.0 += o.seconds;
                entry.1 += 1;
            }
            for (module, (total, count)) in sum {
                weights.insert(module.to_string(), ceil_seconds(total / count as f64));
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn obs(module: &str, seconds: f64, ended_at: &str, seq: usize) -> Observation {
        Observation {
            module: module.into(),
            seconds,
            ended_at: ended_at.into(),
            seq,
        }
    }

    #[test]
    fn subsecond_durations_round_up_to_at_least_one_second() {
        assert_eq!(ceil_seconds(0.0), 1);
        assert_eq!(ceil_seconds(0.2), 1);
        assert_eq!(ceil_seconds(12.0), 12);
        assert_eq!(ceil_seconds(12.1), 13);
    }

    #[test]
    fn modes_combine_observations_with_existing_seed() {
        let observations = vec![
            obs("A/1.0", 40.0, "2026-11-16T01:00:00Z", 1),
            obs("A/1.0", 20.0, "2026-11-17T01:00:00Z", 2),
        ];

        let mut max_w = BTreeMap::from([("A/1.0".into(), 10), ("Keep/1.0".into(), 5)]);
        merge_weights(&mut max_w, &observations, Mode::Max);
        assert_eq!(max_w["A/1.0"], 40);
        assert_eq!(max_w["Keep/1.0"], 5);

        let mut latest_w = BTreeMap::from([("A/1.0".into(), 50), ("Keep/1.0".into(), 5)]);
        merge_weights(&mut latest_w, &observations, Mode::Latest);
        assert_eq!(latest_w["A/1.0"], 20);
        assert_eq!(latest_w["Keep/1.0"], 5);

        let mut mean_w = BTreeMap::from([("A/1.0".into(), 50), ("Keep/1.0".into(), 5)]);
        merge_weights(&mut mean_w, &observations, Mode::Mean);
        assert_eq!(mean_w["A/1.0"], 30);
        assert_eq!(mean_w["Keep/1.0"], 5);
    }
}
