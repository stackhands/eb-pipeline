//! Name the tests that cover the easyconfigs in a build list.
//!
//! A toolchain easyconfig has no test of its own. An application with no
//! row in the map is a refusal, unless `--allow-unmapped` is set. The
//! names on stdout are what a site's test command, such as ReFrame, runs.

use std::collections::BTreeSet;
use std::env;
use std::fs;
use std::io::{self, Write};
use std::path::Path;
use std::process::ExitCode;

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
    map: String,
    list: String,
    extra_toolchains: BTreeSet<String>,
    allow_unmapped: bool,
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
    let map =
        fs::read_to_string(&args.map).map_err(|e| format!("cannot read {}: {e}", args.map))?;
    let list =
        fs::read_to_string(&args.list).map_err(|e| format!("cannot read {}: {e}", args.list))?;
    let rows = parse_map(&map)?;
    let mut tests = BTreeSet::new();
    let mut missing = Vec::new();
    for (line_no, line) in list.lines().enumerate() {
        let line = strip_comment(line);
        if line.is_empty() {
            continue;
        }
        let name = package_name(line).map_err(|e| format!("list line {}: {e}", line_no + 1))?;
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
            eprintln!("eb-test-map: {name} has no test in {}", args.map);
        }
        return Ok(ExitCode::from(2));
    }
    if args.allow_unmapped {
        for name in &missing {
            eprintln!("eb-test-map: {name} has no test in {}; allowed", args.map);
        }
    }
    let mut out = io::stdout().lock();
    for test in &tests {
        writeln!(out, "{test}").map_err(|e| format!("cannot write the test names: {e}"))?;
    }
    Ok(ExitCode::SUCCESS)
}

fn parse(words: impl Iterator<Item = String>) -> Result<Args, String> {
    let mut map = None;
    let mut list = None;
    let mut extra_toolchains = BTreeSet::new();
    let mut allow_unmapped = false;
    let mut words = words.peekable();
    while let Some(word) = words.next() {
        match word.as_str() {
            "-h" | "--help" => {
                print!("{}", include_str!("eb-test-map-help.txt"));
                std::process::exit(0);
            }
            "--allow-unmapped" => allow_unmapped = true,
            "--map" | "--list" | "--toolchain" => {
                let value = words
                    .next()
                    .filter(|v| !v.is_empty())
                    .ok_or_else(|| format!("{word} needs a value"))?;
                match word.as_str() {
                    "--map" => map = Some(value),
                    "--list" => list = Some(value),
                    "--toolchain" => {
                        extra_toolchains.insert(value);
                    }
                    _ => unreachable!(),
                }
            }
            _ if word.starts_with("--map=") => {
                map = Some(need_value("--map", &word["--map=".len()..])?)
            }
            _ if word.starts_with("--list=") => {
                list = Some(need_value("--list", &word["--list=".len()..])?)
            }
            _ if word.starts_with("--toolchain=") => {
                extra_toolchains.insert(need_value("--toolchain", &word["--toolchain=".len()..])?);
            }
            _ => return Err(format!("does not know `{word}`")),
        }
    }
    Ok(Args {
        map: map.ok_or_else(|| "--map is required".to_string())?,
        list: list.ok_or_else(|| "--list is required".to_string())?,
        extra_toolchains,
        allow_unmapped,
    })
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

/// The package of an easyconfig path: the part of the file name before the version.
pub fn package_name(line: &str) -> Result<String, String> {
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
    for (i, part) in stem.split('-').enumerate() {
        if part.is_empty() {
            return Err(format!("`{file}` is not an easyconfig"));
        }
        if part.chars().next().is_some_and(|c| c.is_ascii_digit()) {
            if i == 0 {
                return Err(format!("`{file}` is not an easyconfig"));
            }
            break;
        }
        if i > 0 {
            name.push('-');
        }
        name.push_str(part);
    }
    if name.is_empty() {
        return Err(format!("`{file}` is not an easyconfig"));
    }
    Ok(name)
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
}
