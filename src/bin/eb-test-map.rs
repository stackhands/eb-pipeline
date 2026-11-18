//! Name the tests that cover the easyconfigs in a build list.
//!
//! A toolchain easyconfig has no test of its own. An application with no
//! row in the map is a refusal, unless `--allow-unmapped` is set. The
//! names on stdout are what a site's test command, such as ReFrame, runs.
//!
//! `--format eessi-modules` does not read a map. It prints one
//! `--setvar modules=Name/Version,...` argument for the applications in
//! the list.

use eb_pipeline::util;
use std::collections::BTreeSet;
use std::env;
use std::fs;
use std::path::Path;
use std::process::ExitCode;

use eb_pipeline::easystack;
use serde_json::{Map, Value};

/// Toolchains a build list carries that are not applications.
const TOOLCHAINS: &[&str] = &[
    "GCC",
    "GCCcore",
    "foss",
    "gfbf",
    "gomkl",
    "gompi",
    "iimpi",
    "intel",
    "intel-compilers",
];

struct Args {
    map: Option<String>,
    list: Option<String>,
    result: Option<String>,
    extra_toolchains: BTreeSet<String>,
    allow_unmapped: bool,
    include_resumed: bool,
    format: OutputFormat,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum OutputFormat {
    Plain,
    Reframe,
    EessiModules,
}

struct Stem {
    file: String,
    name: String,
    /// The easyconfig version, including the toolchain and any suffix.
    version: String,
}

fn main() -> ExitCode {
    match run() {
        Ok(code) => code,
        Err(msg) => {
            eprintln!("eb-test-map: {msg}");
            ExitCode::from(2)
        }
    }
}

fn run() -> Result<ExitCode, String> {
    let args = parse(env::args().skip(1))?;
    match args.format {
        OutputFormat::EessiModules => eessi_modules(&args),
        OutputFormat::Plain | OutputFormat::Reframe => text_names(&args),
    }
}

fn text_names(args: &Args) -> Result<ExitCode, String> {
    let map_path = args
        .map
        .as_deref()
        .ok_or_else(|| "--map is required".to_string())?;
    let map = fs::read_to_string(map_path).map_err(|e| format!("cannot read {map_path}: {e}"))?;
    let rows = parse_map(&map)?;
    let packages = package_names(args)?;
    let mut tests = BTreeSet::new();
    let mut missing = Vec::new();
    for name in packages {
        if is_toolchain(&name, &args.extra_toolchains) {
            continue;
        }
        let found: Vec<&str> = rows
            .iter()
            .filter(|(software, _)| software == &name)
            .map(|(_, test)| test.as_str())
            .collect();
        if found.is_empty() {
            missing.push(name);
        } else {
            tests.extend(found.into_iter().map(str::to_string));
        }
    }
    if !missing.is_empty() && !args.allow_unmapped {
        for name in &missing {
            eprintln!("eb-test-map: {name} has no test in {map_path}");
        }
        return Ok(ExitCode::from(2));
    }
    if args.allow_unmapped {
        for name in &missing {
            eprintln!("eb-test-map: {name} has no test in {map_path}; allowed");
        }
    }
    let mut out_text = String::new();
    for test in &tests {
        let line = match args.format {
            OutputFormat::Plain => test.clone(),
            OutputFormat::Reframe => format!("-n ^{}$", escape_regex_literal(test)),
            OutputFormat::EessiModules => unreachable!(),
        };
        out_text.push_str(&line);
        out_text.push('\n');
    }
    util::write_stdout(out_text.as_bytes())
        .map_err(|e| format!("cannot write the test names: {e}"))?;
    Ok(ExitCode::SUCCESS)
}

/// One ReFrame argument. `hooks.set_modules` keeps a test when each of its
/// module names is in the list ReFrame built from this value.
fn eessi_modules(args: &Args) -> Result<ExitCode, String> {
    let paths = easyconfig_paths(args)?;
    let mut modules = BTreeSet::new();
    for (line_no, line) in paths.iter().enumerate() {
        let name = package_name(line).map_err(|e| format!("list line {}: {e}", line_no + 1))?;
        if is_toolchain(&name, &args.extra_toolchains) {
            continue;
        }
        let coord = module_coord(line).map_err(|e| format!("list line {}: {e}", line_no + 1))?;
        modules.insert(coord);
    }
    if modules.is_empty() {
        return Ok(ExitCode::SUCCESS);
    }
    let joined = modules.into_iter().collect::<Vec<_>>().join(",");
    let line = format!("--setvar modules={joined}\n");
    util::write_stdout(line.as_bytes())
        .map_err(|e| format!("cannot write the module list: {e}"))?;
    Ok(ExitCode::SUCCESS)
}

fn parse(words: impl Iterator<Item = String>) -> Result<Args, String> {
    let mut map = None;
    let mut list: Option<String> = None;
    let mut result: Option<String> = None;
    let mut extra_toolchains = BTreeSet::new();
    let mut allow_unmapped = false;
    let mut include_resumed = false;
    let mut format: Option<OutputFormat> = None;
    let mut words = words.peekable();
    while let Some(word) = words.next() {
        match word.as_str() {
            "-h" | "--help" => {
                print!("{}", include_str!("eb-test-map-help.txt"));
                std::process::exit(0);
            }
            "-V" | "--version" => {
                println!("eb-test-map {}", env!("CARGO_PKG_VERSION"));
                std::process::exit(0);
            }
            "--allow-unmapped" => allow_unmapped = true,
            "--include-resumed" => include_resumed = true,
            "--map" | "--list" | "--result" | "--toolchain" | "--format" => {
                let value = words
                    .next()
                    .filter(|v| !v.is_empty())
                    .ok_or_else(|| format!("{word} needs a value"))?;
                match word.as_str() {
                    "--map" => assign_once(&mut map, "--map", value)?,
                    "--list" => assign_once(&mut list, "--list", value)?,
                    "--result" => assign_once(&mut result, "--result", value)?,
                    "--toolchain" => {
                        extra_toolchains.insert(value);
                    }
                    "--format" => assign_once(&mut format, "--format", parse_format(&value)?)?,
                    _ => unreachable!(),
                }
            }
            _ if word.starts_with("--map=") => {
                let value = need_value("--map", &word["--map=".len()..])?;
                assign_once(&mut map, "--map", value)?;
            }
            _ if word.starts_with("--list=") => {
                let value = need_value("--list", &word["--list=".len()..])?;
                assign_once(&mut list, "--list", value)?;
            }
            _ if word.starts_with("--result=") => {
                let value = need_value("--result", &word["--result=".len()..])?;
                assign_once(&mut result, "--result", value)?;
            }
            _ if word.starts_with("--toolchain=") => {
                extra_toolchains.insert(need_value("--toolchain", &word["--toolchain=".len()..])?);
            }
            _ if word.starts_with("--format=") => {
                let value = need_value("--format", &word["--format=".len()..])?;
                assign_once(&mut format, "--format", parse_format(&value)?)?;
            }
            _ => return Err(format!("does not know `{word}`")),
        }
    }
    if list.is_none() && result.is_none() {
        list = eb_pipeline::util::env_value("EB_PIPELINE_LIST");
        result = eb_pipeline::util::env_value("EB_PIPELINE_RESULT");
    }
    let format = format.unwrap_or(OutputFormat::Plain);
    if list.is_some() && result.is_some() {
        return Err("pass either --list or --result, not both".into());
    }
    if list.is_none() && result.is_none() {
        return Err("--list or --result is required".into());
    }
    if format != OutputFormat::EessiModules && map.is_none() {
        return Err("--map is required".to_string());
    }
    if include_resumed && result.is_none() {
        return Err("--include-resumed requires --result".into());
    }
    Ok(Args {
        map,
        list,
        result,
        extra_toolchains,
        allow_unmapped,
        include_resumed,
        format,
    })
}

fn assign_once<T>(slot: &mut Option<T>, flag: &str, value: T) -> Result<(), String> {
    if slot.is_some() {
        return Err(format!("{flag} was given twice"));
    }
    *slot = Some(value);
    Ok(())
}

fn need_value(flag: &str, value: &str) -> Result<String, String> {
    if value.is_empty() {
        Err(format!("{flag} needs a value"))
    } else {
        Ok(value.to_string())
    }
}

fn strip_comment(line: &str) -> &str {
    let line = line.trim();
    if line.starts_with('#') {
        ""
    } else {
        line
    }
}

fn parse_format(value: &str) -> Result<OutputFormat, String> {
    match value {
        "text" => Ok(OutputFormat::Plain),
        "reframe" => Ok(OutputFormat::Reframe),
        "eessi-modules" => Ok(OutputFormat::EessiModules),
        _ => Err(format!(
            "unknown --format `{value}`; expected `text`, `reframe`, or `eessi-modules`"
        )),
    }
}

fn package_names(args: &Args) -> Result<Vec<String>, String> {
    if let Some(path) = &args.list {
        package_names_from_list(path)
    } else if let Some(path) = &args.result {
        package_names_from_result(path, args.include_resumed)
    } else {
        unreachable!("validated in parse")
    }
}

fn easyconfig_paths(args: &Args) -> Result<Vec<String>, String> {
    if let Some(path) = &args.list {
        easyconfigs_from_list(path)
    } else if let Some(path) = &args.result {
        easyconfigs_from_result(path, args.include_resumed)
    } else {
        unreachable!("validated in parse")
    }
}

fn package_names_from_list(path: &str) -> Result<Vec<String>, String> {
    easyconfigs_from_list(path)?
        .into_iter()
        .map(|line| package_name(&line))
        .collect()
}

fn easyconfigs_from_list(path: &str) -> Result<Vec<String>, String> {
    if easystack::is_easystack(Path::new(path)) {
        let lines = easystack::read(Path::new(path))?;
        Ok(lines.into_iter().map(|line| line.easyconfig).collect())
    } else {
        let list_text = fs::read_to_string(path).map_err(|e| format!("cannot read {path}: {e}"))?;
        Ok(util::parse_list(&list_text)
            .map_err(|error| format!("{path}: {}", error.msg))?
            .into_iter()
            .map(|line| line.easyconfig)
            .collect())
    }
}

fn package_names_from_result(path: &str, include_resumed: bool) -> Result<Vec<String>, String> {
    easyconfigs_from_result(path, include_resumed)?
        .into_iter()
        .map(|line| package_name(&line))
        .collect()
}

fn easyconfigs_from_result(path: &str, include_resumed: bool) -> Result<Vec<String>, String> {
    let text = fs::read_to_string(path).map_err(|e| format!("cannot read {path}: {e}"))?;
    let value: Value =
        serde_json::from_str(&text).map_err(|e| format!("cannot parse {path}: {e}"))?;
    let obj = object(&value, "result")?;
    let kind = req_str(obj, "kind", "result")?;
    if kind != "result" {
        return Err(format!("expected kind `result`, found `{kind}`"));
    }
    match obj.get("schema_version") {
        Some(Value::Number(n)) if n.as_u64() == Some(1) => {}
        Some(_) => return Err("schema_version must be 1".into()),
        None => return Err("missing `schema_version`".into()),
    }
    let lines = match obj.get("lines") {
        Some(Value::Array(items)) => items,
        Some(_) => return Err("`lines` must be an array".into()),
        None => return Err("missing `lines`".into()),
    };
    let mut names = Vec::new();
    for (i, item) in lines.iter().enumerate() {
        let line = object(item, &format!("lines[{i}]"))?;
        let status = req_str(line, "status", &format!("lines[{i}]"))?;
        let resumed = include_resumed
            && status == "skipped"
            && line
                .get("note")
                .and_then(Value::as_str)
                .is_some_and(|note| note.starts_with("resumed:"));
        if status == "success" || resumed {
            let easyconfig = req_str(line, "easyconfig", &format!("lines[{i}]"))?;
            names.push(easyconfig.to_string());
        }
    }
    Ok(names)
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

fn escape_regex_literal(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for c in s.chars() {
        if matches!(
            c,
            '\\' | '.' | '^' | '$' | '|' | '?' | '*' | '+' | '(' | ')' | '[' | ']' | '{' | '}'
        ) {
            out.push('\\');
        }
        out.push(c);
    }
    out
}

/// `software` and `test` rows. A `#` line is a comment.
fn parse_map(text: &str) -> Result<Vec<(String, String)>, String> {
    let mut rows = Vec::new();
    for (line_no, raw) in text.lines().enumerate() {
        let line = strip_comment(raw);
        if line.is_empty() {
            continue;
        }
        let mut fields = line.split_whitespace();
        let software = fields.next().ok_or_else(|| {
            format!(
                "map line {}: expected a software name and a test",
                line_no + 1
            )
        })?;
        let test = fields
            .next()
            .ok_or_else(|| format!("map line {}: {software} has no test name", line_no + 1))?;
        if fields.next().is_some() {
            return Err(format!(
                "map line {}: {software} has more than one test name; use another line",
                line_no + 1
            ));
        }
        rows.push((software.to_string(), test.to_string()));
    }
    Ok(rows)
}

/// Split an easyconfig path the way `package_name` always has: the name is
/// every `-` component before the first one that starts with a digit.
fn split_stem(line: &str) -> Result<Stem, String> {
    let file = Path::new(line.trim())
        .file_name()
        .and_then(|n| n.to_str())
        .ok_or_else(|| format!("`{line}` is not an easyconfig"))?;
    let stem = file
        .strip_suffix(".eb")
        .ok_or_else(|| format!("`{file}` is not an easyconfig"))?;
    if stem.is_empty() || stem.contains('/') {
        return Err(format!("`{file}` is not an easyconfig"));
    }
    let mut name = String::new();
    let mut version = String::new();
    let mut seen_version = false;
    for (i, part) in stem.split('-').enumerate() {
        if part.is_empty() {
            return Err(format!("`{file}` is not an easyconfig"));
        }
        if !seen_version && part.chars().next().is_some_and(|c| c.is_ascii_digit()) {
            if i == 0 {
                return Err(format!("`{file}` is not an easyconfig"));
            }
            seen_version = true;
        }
        if seen_version {
            if !version.is_empty() {
                version.push('-');
            }
            version.push_str(part);
        } else {
            if i > 0 {
                name.push('-');
            }
            name.push_str(part);
        }
    }
    if name.is_empty() {
        return Err(format!("`{file}` is not an easyconfig"));
    }
    Ok(Stem {
        file: file.to_string(),
        name,
        version,
    })
}

/// The package of an easyconfig path: the part of the file name before the version.
pub fn package_name(line: &str) -> Result<String, String> {
    Ok(split_stem(line)?.name)
}

/// `Name/VersionFull` for one easyconfig. ReFrame splits the `--setvar`
/// value on commas and does not trim, so a comma or a space cannot be one
/// entry of that list.
fn module_coord(line: &str) -> Result<String, String> {
    let stem = split_stem(line)?;
    if stem.version.is_empty() {
        return Err(format!("`{}` has no version", stem.file));
    }
    if stem.name.contains(',')
        || stem.version.contains(',')
        || stem.name.chars().any(char::is_whitespace)
        || stem.version.chars().any(char::is_whitespace)
    {
        return Err(format!(
            "`{}` has a comma or a space in its module name",
            stem.file
        ));
    }
    let name = stem.name;
    let version = stem.version;
    Ok(format!("{name}/{version}"))
}

fn is_toolchain(name: &str, extra: &BTreeSet<String>) -> bool {
    TOOLCHAINS.contains(&name) || extra.contains(name)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_version_starts_at_the_first_numeric_component() {
        assert_eq!(
            package_name("GROMACS-2024.1-foss-2023b.eb").unwrap(),
            "GROMACS"
        );
        assert_eq!(
            package_name("/lists/Python-3.11.5-GCCcore-12.3.0.eb").unwrap(),
            "Python"
        );
        assert_eq!(package_name("libfoo-bar-1.2.eb").unwrap(), "libfoo-bar");
        assert_eq!(package_name("GCCcore-13.2.0.eb").unwrap(), "GCCcore");
        assert!(package_name("notes.txt").is_err());
    }

    #[test]
    fn a_module_name_keeps_the_version_toolchain_and_suffix() {
        assert_eq!(
            module_coord("GROMACS-2024.1-foss-2023b.eb").unwrap(),
            "GROMACS/2024.1-foss-2023b"
        );
        assert_eq!(
            module_coord("/lists/Python-3.11.5-GCCcore-12.3.0.eb").unwrap(),
            "Python/3.11.5-GCCcore-12.3.0"
        );
        assert_eq!(module_coord("zlib-1.3.1.eb").unwrap(), "zlib/1.3.1");
        assert_eq!(module_coord("libfoo-bar-1.2.eb").unwrap(), "libfoo-bar/1.2");
        assert_eq!(
            module_coord("GROMACS-2024.1-foss-2023b-CUDA-12.1.0.eb").unwrap(),
            "GROMACS/2024.1-foss-2023b-CUDA-12.1.0"
        );
        assert!(module_coord("Foo-bar.eb")
            .unwrap_err()
            .contains("no version"));
        assert!(module_coord("Foo-1,2.eb")
            .unwrap_err()
            .contains("comma or a space"));
    }

    #[test]
    fn a_map_keeps_comments_out_and_refuses_a_row_with_no_test() {
        let rows = parse_map("# comment\n\nGROMACS gromacs\nFFTW fftw\n").unwrap();
        assert_eq!(
            rows,
            vec![
                ("GROMACS".into(), "gromacs".into()),
                ("FFTW".into(), "fftw".into())
            ]
        );
        assert!(parse_map("GROMACS\n").unwrap_err().contains("no test name"));
    }

    #[test]
    fn reframe_selectors_escape_regex_characters() {
        assert_eq!(
            escape_regex_literal("gromacs+gpu(v2)"),
            "gromacs\\+gpu\\(v2\\)"
        );
    }
}
