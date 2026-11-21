//! Build list lines from an EasyBuild easystack: the file `eb --easystack`
//! builds from, which `eb-stack rebuild-set`, `eb-stack stack order
//! --easystack-out` and `eb-stack stack easystack` write.
//!
//! Each entry becomes the line of the build EasyBuild 5 makes of it: a name
//! without `.eb` gets it, and the entry's `options` become `eb` options as
//! EasyBuild's `opts_dict_to_eb_opts` writes them, in the order of their
//! names. Only `easyconfigs` is read; EasyBuild ignores the other top-level
//! keys as well.

use crate::util::{list_word, ListLine};
use serde_yaml::{Mapping, Value};
use std::io::Read;
use std::path::Path;

struct Diagnostic {
    message: String,
    location: Option<(usize, usize)>,
}

/// Whether the build list `path` is an easystack: a `.yml` or `.yaml` file.
pub fn is_easystack(path: &Path) -> bool {
    path.extension()
        .and_then(|extension| extension.to_str())
        .is_some_and(|extension| {
            extension.eq_ignore_ascii_case("yml") || extension.eq_ignore_ascii_case("yaml")
        })
}

/// Read and parse an easystack, with source locations in its diagnostics.
pub fn read(path: &Path) -> Result<Vec<ListLine>, String> {
    let mut file = crate::util::open_regular_file(path)
        .map_err(|e| format!("cannot read list {}: {e}", path.display()))?;
    let mut bytes = Vec::new();
    file.read_to_end(&mut bytes)
        .map_err(|e| format!("cannot read list {}: {e}", path.display()))?;
    let text = std::str::from_utf8(&bytes).map_err(|e| {
        let (line, column) = byte_location(&bytes, e.valid_up_to());
        format!(
            "easystack {}:{line}:{column}: input is not valid UTF-8",
            path.display()
        )
    })?;
    parse_diagnostic(text).map_err(|error| match error.location {
        Some((line, column)) => format!(
            "easystack {}:{line}:{column}: {}",
            path.display(),
            error.message
        ),
        None => format!("easystack {}: {}", path.display(), error.message),
    })
}

/// The build list lines of the easystack `text`, in file order.
pub fn parse(text: &str) -> Result<Vec<ListLine>, String> {
    parse_diagnostic(text).map_err(|error| error.message)
}

fn parse_diagnostic(text: &str) -> Result<Vec<ListLine>, Diagnostic> {
    let mut doc: Value =
        serde_yaml::from_str(text).map_err(|error| yaml_diagnostic(text, error))?;
    doc.apply_merge()
        .map_err(|error| yaml_diagnostic(text, error))?;
    let entries = match doc.get("easyconfigs") {
        Some(Value::Sequence(entries)) => entries,
        Some(_) => {
            return Err(Diagnostic {
                message: "`easyconfigs` is not a list; start each entry with `- `".into(),
                location: None,
            })
        }
        None => {
            return Err(Diagnostic {
                message: "there is no top-level `easyconfigs` key".into(),
                location: None,
            })
        }
    };
    entries
        .iter()
        .enumerate()
        .map(|(i, item)| {
            entry(item).map_err(|message| Diagnostic {
                message: format!("entry {}: {message}", i + 1),
                location: None,
            })
        })
        .collect()
}

fn yaml_diagnostic(text: &str, error: serde_yaml::Error) -> Diagnostic {
    let parser_location = error
        .location()
        .map(|location| (location.line(), location.column()));
    let mut message = error.to_string();
    if let Some((line, column)) = parser_location {
        message = message
            .strip_suffix(&format!(" at line {line} column {column}"))
            .unwrap_or(&message)
            .to_string();
    }
    let mut location = parser_location;
    if let Some(duplicate) = duplicate_key_location(text, &message) {
        location = Some(duplicate);
    }
    Diagnostic { message, location }
}

fn duplicate_key_location(text: &str, message: &str) -> Option<(usize, usize)> {
    let encoded = message
        .rsplit(": ")
        .next()?
        .strip_prefix("duplicate entry with key ")?;
    let key: String = serde_json::from_str(encoded).ok()?;
    let mut scopes = Vec::<(usize, usize)>::new();
    let mut seen = std::collections::BTreeSet::new();
    for (index, line) in text.lines().enumerate() {
        let line = line.trim_end_matches('\r');
        let indent = line.len() - line.trim_start_matches(' ').len();
        let content = &line[indent..];
        if content.is_empty() || content.starts_with('#') {
            continue;
        }
        while scopes.last().is_some_and(|(level, _)| *level >= indent) {
            scopes.pop();
        }
        let parent = scopes.last().map(|(_, line)| *line);
        if let Some((token, _)) = content.split_once(':') {
            let token = token.trim();
            if token.trim_matches(['\'', '"']) == key && !seen.insert((parent, indent)) {
                return Some((index + 1, indent + 1));
            }
        }
        scopes.push((indent, index));
    }
    None
}

fn byte_location(bytes: &[u8], offset: usize) -> (usize, usize) {
    let before = &bytes[..offset];
    let line = before.iter().filter(|byte| **byte == b'\n').count() + 1;
    let column = before
        .iter()
        .rposition(|byte| *byte == b'\n')
        .map_or(offset + 1, |newline| offset - newline);
    (line, column)
}

fn entry(e: &Value) -> Result<ListLine, String> {
    let (name, options) = match e {
        Value::String(name) => (name, None),
        Value::Mapping(m) if m.len() == 1 => {
            let (k, v) = m.iter().next().expect("one key");
            let Value::String(name) = k else {
                return Err(format!("the easyconfig name `{}` is not a string", show(k)));
            };
            (name, options_of(name, v)?)
        }
        Value::Mapping(m) if m.len() > 1 => {
            let keys: Vec<String> = m.keys().map(show).collect();
            return Err(format!(
                "the entry has {} keys, {}: indent `options` under the easyconfig name",
                m.len(),
                keys.join(", ")
            ));
        }
        _ => {
            return Err(
                "an entry is an easyconfig name, or a mapping of one name to its options".into(),
            )
        }
    };
    if name.is_empty() {
        return Err("the easyconfig name is empty".into());
    }
    let easyconfig = if name.ends_with(".eb") {
        name.clone()
    } else {
        format!("{name}.eb")
    };
    let args = match options {
        Some(o) => eb_args(o).map_err(|m| format!("{easyconfig}: {m}"))?,
        None => Vec::new(),
    };
    let mut raw = Vec::with_capacity(1 + args.len());
    for w in std::iter::once(&easyconfig).chain(&args) {
        raw.push(list_word(w).map_err(|e| format!("{easyconfig}: {e}"))?);
    }
    Ok(ListLine {
        raw: raw.join(" "),
        easyconfig,
        args,
    })
}

/// The `options` of the entry `name`, the only key EasyBuild takes there.
fn options_of<'a>(name: &str, v: &'a Value) -> Result<Option<&'a Mapping>, String> {
    let spec = match v {
        Value::Null => return Ok(None),
        Value::Mapping(spec) => spec,
        _ => return Err(format!("{name}: put its options under `options:`")),
    };
    let mut other: Vec<String> = spec
        .keys()
        .filter(|k| k.as_str() != Some("options"))
        .map(show)
        .collect();
    if !other.is_empty() {
        other.sort();
        return Err(format!(
            "{name}: only `options` can follow the name, not {}",
            other.join(", ")
        ));
    }
    match spec.get("options") {
        None | Some(Value::Null) => Ok(None),
        Some(Value::Mapping(o)) => Ok(Some(o)),
        Some(_) => Err(format!(
            "{name}: `options` is not a mapping of option names to values"
        )),
    }
}

/// The `eb` options of an entry, as `opts_dict_to_eb_opts` writes them:
/// sorted by name, `-x` for a name of one letter and `--name` for a longer
/// one; true, no value or the number 1 gives the bare option, false
/// `--disable-name`, a list its items joined with `,`, and anything else
/// `--name=value`, except that `amend` and `try-amend` take one option per
/// item.
fn eb_args(options: &Mapping) -> Result<Vec<String>, String> {
    let mut named = Vec::new();
    for (k, v) in options {
        let Value::String(k) = k else {
            return Err(format!("the option name `{}` is not a string", show(k)));
        };
        if k.is_empty() || k.starts_with('-') {
            return Err(format!(
                "the option `{k}`: write an option's name without its dashes"
            ));
        }
        named.push((k.as_str(), v));
    }
    named.sort_by_key(|(k, _)| *k);
    let mut args = Vec::new();
    for (k, v) in named {
        let short = k.chars().count() == 1;
        let opt = if short {
            format!("-{k}")
        } else {
            format!("--{k}")
        };
        if k == "amend" || k == "try-amend" {
            let items: Vec<&Value> = match v {
                Value::Sequence(s) => s.iter().collect(),
                v => vec![v],
            };
            for x in items {
                args.push(format!("{opt}={}", text(k, x)?));
            }
            continue;
        }
        match flag(v) {
            Some(true) => args.push(opt),
            Some(false) if short => {
                return Err(format!(
                    "the option `{k}`: a one-letter option has no --disable- form"
                ))
            }
            Some(false) => args.push(format!("--disable-{k}")),
            None if short => {
                return Err(format!(
                    "the option `{k}`: give a one-letter option that takes a value by its long name"
                ))
            }
            None => {
                let value = match v {
                    Value::Sequence(s) => s
                        .iter()
                        .map(|x| text(k, x))
                        .collect::<Result<Vec<_>, _>>()?
                        .join(","),
                    v => text(k, v)?,
                };
                args.push(format!("{opt}={value}"));
            }
        }
    }
    Ok(args)
}

/// `Some(true)` for a value that gives the bare option, and `Some(false)`
/// for false. EasyBuild reads YAML 1.1, where yes, on, no and off are
/// booleans too, and gives the bare option for each value in `[True,
/// None]`, which in Python holds the numbers 1 and 1.0 as well.
fn flag(v: &Value) -> Option<bool> {
    match v {
        Value::Null => Some(true),
        Value::Bool(b) => Some(*b),
        Value::String(s) => yaml11_bool(s),
        Value::Number(n) if n.as_f64() == Some(1.0) => Some(true),
        _ => None,
    }
}

fn yaml11_bool(s: &str) -> Option<bool> {
    match s {
        "yes" | "Yes" | "YES" | "on" | "On" | "ON" => Some(true),
        "no" | "No" | "NO" | "off" | "Off" | "OFF" => Some(false),
        _ => None,
    }
}

/// A value of the option `k` as EasyBuild writes it, with Python's `str()`.
fn text(k: &str, v: &Value) -> Result<String, String> {
    match v {
        Value::String(s) => Ok(match yaml11_bool(s) {
            Some(b) => python_bool(b).into(),
            None => s.clone(),
        }),
        Value::Bool(b) => Ok(python_bool(*b).into()),
        Value::Null => Ok("None".into()),
        Value::Number(n) if n.is_i64() || n.is_u64() => Ok(n.to_string()),
        Value::Number(n) => {
            Ok(python_float(n.as_f64().ok_or_else(|| {
                format!("the option `{k}`: `{n}` is not a finite number")
            })?))
        }
        _ => Err(format!(
            "the option `{k}`: a value is text, a number, a boolean or a list of them"
        )),
    }
}

/// Format a binary float as Python's `str(float)` does.
fn python_float(value: f64) -> String {
    if value.is_nan() {
        return "nan".into();
    }
    if value == f64::INFINITY {
        return "inf".into();
    }
    if value == f64::NEG_INFINITY {
        return "-inf".into();
    }

    let lexical = value.to_string();
    let (negative, unsigned) = lexical
        .strip_prefix('-')
        .map_or((false, lexical.as_str()), |s| (true, s));
    let (mantissa, explicit_exponent) = unsigned
        .split_once(['e', 'E'])
        .map_or((unsigned, 0), |(m, e)| (m, e.parse::<i32>().unwrap()));
    let (integer, fraction) = mantissa.split_once('.').unwrap_or((mantissa, ""));
    let raw_digits = format!("{integer}{fraction}");

    if raw_digits.bytes().all(|b| b == b'0') {
        return if negative { "-0.0" } else { "0.0" }.into();
    }

    let leading = raw_digits.bytes().take_while(|b| *b == b'0').count();
    let mut digits = raw_digits[leading..].trim_end_matches('0').to_string();
    let exponent = if integer.bytes().any(|b| b != b'0') {
        integer.len() as i32 - integer.bytes().take_while(|b| *b == b'0').count() as i32 - 1
    } else {
        -(fraction.bytes().take_while(|b| *b == b'0').count() as i32) - 1
    } + explicit_exponent;

    let mut result = String::new();
    if negative {
        result.push('-');
    }
    if !(-4..16).contains(&exponent) {
        result.push(digits.remove(0));
        if !digits.is_empty() {
            result.push('.');
            result.push_str(&digits);
        }
        result.push('e');
        result.push(if exponent < 0 { '-' } else { '+' });
        result.push_str(&format!("{:02}", exponent.unsigned_abs()));
    } else if exponent < 0 {
        result.push_str("0.");
        result.extend(std::iter::repeat_n('0', (-exponent - 1) as usize));
        result.push_str(&digits);
    } else {
        let decimal = exponent as usize + 1;
        if digits.len() <= decimal {
            result.push_str(&digits);
            result.extend(std::iter::repeat_n('0', decimal - digits.len()));
            result.push_str(".0");
        } else {
            result.push_str(&digits[..decimal]);
            result.push('.');
            result.push_str(&digits[decimal..]);
        }
    }
    result
}

fn python_bool(b: bool) -> &'static str {
    if b {
        "True"
    } else {
        "False"
    }
}

fn show(v: &Value) -> String {
    serde_yaml::to_string(v)
        .map(|s| s.trim_end().to_string())
        .unwrap_or_else(|_| "?".into())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn lines(text: &str) -> Vec<String> {
        parse(text).unwrap().into_iter().map(|l| l.raw).collect()
    }

    fn refused(text: &str) -> String {
        parse(text).unwrap_err()
    }

    #[test]
    fn names_get_eb_and_keep_their_order() {
        assert_eq!(
            lines("easyconfigs:\n  - b-1.0\n  - a-1.0.eb\n  - c-1.0.eb:\n"),
            ["b-1.0.eb", "a-1.0.eb", "c-1.0.eb"]
        );
    }

    #[test]
    fn ordinary_easyconfig_bytes_stay_unchanged() {
        let line = &parse("easyconfigs:\n  - Foo-1.0-foss-2025a.eb\n").unwrap()[0];
        assert_eq!(line.raw.as_bytes(), b"Foo-1.0-foss-2025a.eb");
        assert_eq!(line.easyconfig.as_bytes(), b"Foo-1.0-foss-2025a.eb");
    }

    #[test]
    fn options_become_eb_options_sorted_by_name() {
        let text = "easyconfigs:
  - x-1.0.eb:
      options:
        rebuild: true
        from-pr: 12345
        robot: false
        include-easyblocks: [a.py, b.py]
        amend: [versionsuffix=-a, toolchain=b]
        force:
        f: yes
        accept-eula-for: 'A B'
";
        let l = &parse(text).unwrap()[0];
        assert_eq!(
            l.args,
            [
                "--accept-eula-for=A B",
                "--amend=versionsuffix=-a",
                "--amend=toolchain=b",
                "-f",
                "--force",
                "--from-pr=12345",
                "--include-easyblocks=a.py,b.py",
                "--rebuild",
                "--disable-robot",
            ]
        );
        assert_eq!(
            l.raw,
            "x-1.0.eb '--accept-eula-for=A B' --amend=versionsuffix=-a --amend=toolchain=b -f --force --from-pr=12345 --include-easyblocks=a.py,b.py --rebuild --disable-robot"
        );
        assert_eq!(crate::util::parse_list(&l.raw).unwrap()[0], *l);
    }

    #[test]
    fn yaml_merge_keys_supply_easyconfig_options_as_in_easybuild() {
        let text = r#"
defaults: &defaults
  options:
    rebuild: true
    robot: false
easyconfigs:
  - x-1.0.eb:
      <<: *defaults
"#;
        assert_eq!(lines(text), ["x-1.0.eb --rebuild --disable-robot"],);
    }

    #[test]
    fn ordinary_easystack_line_bytes_remain_unchanged() {
        let parsed =
            parse("easyconfigs:\n  - x-1.0.eb: {options: {rebuild: true, robot: false}}\n")
                .unwrap();
        assert_eq!(
            parsed[0].raw.as_bytes(),
            b"x-1.0.eb --rebuild --disable-robot"
        );
    }

    #[test]
    fn yaml_1_1_booleans_are_booleans_as_in_easybuild() {
        let l =
            &parse("easyconfigs:\n  - x.eb: {options: {rebuild: on, robot: off, a-b: [yes, 1]}}\n")
                .unwrap()[0];
        assert_eq!(l.args, ["--a-b=True,1", "--rebuild", "--disable-robot"]);
    }

    #[test]
    fn an_apostrophe_and_a_shell_character_stay_one_argument() {
        let l = &parse(
            "easyconfigs:\n  - x.eb: {options: {amend: \"it's\", hooks: \"/tmp/a(b).py\"}}\n",
        )
        .unwrap()[0];
        assert_eq!(l.args, ["--amend=it's", "--hooks=/tmp/a(b).py"]);
        assert_eq!(l.raw, "x.eb \"--amend=it's\" --hooks=/tmp/a(b).py");
        assert_eq!(crate::util::parse_list(&l.raw).unwrap()[0], *l);
    }

    #[test]
    fn a_plain_one_gives_the_bare_option_as_in_easybuild() {
        let l = &parse(
            "easyconfigs:\n  - x.eb: {options: {rebuild: 1, parallel: '1', a-b: [1], amend: 1, c-d: 0}}\n",
        )
        .unwrap()[0];
        assert_eq!(
            l.args,
            [
                "--a-b=1",
                "--amend=1",
                "--c-d=0",
                "--parallel=1",
                "--rebuild"
            ]
        );
    }

    #[test]
    fn a_floating_one_gives_the_bare_option_as_in_easybuild() {
        let l = &parse("easyconfigs:\n  - x.eb: {options: {rebuild: 1.0, parallel: '1.0'}}\n")
            .unwrap()[0];
        assert_eq!(l.args, ["--parallel=1.0", "--rebuild"]);
    }

    #[test]
    fn decimal_option_values_are_written_as_in_easybuild() {
        let l = &parse(
            "easyconfigs:\n  - x.eb: {options: {amend: 2.5, large: 1.0e+20, nan: .nan, negative: -.inf, parallel: 1.5, positive: .inf, ratios: [2.5, 3.25], small: 0.000001}}\n",
        )
        .unwrap()[0];
        assert_eq!(
            l.args,
            [
                "--amend=2.5",
                "--large=1e+20",
                "--nan=nan",
                "--negative=-inf",
                "--parallel=1.5",
                "--positive=inf",
                "--ratios=2.5,3.25",
                "--small=1e-06"
            ]
        );
    }

    #[test]
    fn ordinary_option_output_keeps_its_exact_bytes() {
        let l = &parse(
            "easyconfigs:\n  - x.eb: {options: {rebuild: true, parallel: 4, robot: false}}\n",
        )
        .unwrap()[0];
        assert_eq!(
            l.raw.as_bytes(),
            b"x.eb --parallel=4 --rebuild --disable-robot"
        );
    }

    #[test]
    fn other_top_level_keys_are_ignored_as_eb_ignores_them() {
        assert_eq!(
            lines("robot: true\neasybuild_version: 5.4.2\neasyconfigs: [x-1.0.eb]\n"),
            ["x-1.0.eb"]
        );
    }

    #[test]
    fn what_eb_would_not_build_as_written_is_refused() {
        for (text, needed) in [
            ("easyconfig: [x.eb]\n", "no top-level `easyconfigs` key"),
            ("easyconfigs: x.eb\n", "`easyconfigs` is not a list"),
            ("easyconfigs:\n  x.eb: {}\n", "`easyconfigs` is not a list"),
            (
                "easyconfigs:\n  - [x.eb]\n",
                "entry 1: an entry is an easyconfig name",
            ),
            (
                "easyconfigs:\n  - a.eb:\n    options:\n      rebuild: true\n",
                "entry 1: the entry has 2 keys, a.eb, options: indent `options` under the easyconfig name",
            ),
            (
                "easyconfigs:\n  - x.eb\n  - y.eb: {opts: {}}\n",
                "entry 2: y.eb: only `options` can follow the name, not opts",
            ),
            (
                "easyconfigs:\n  - x.eb: {options: [rebuild]}\n",
                "`options` is not a mapping",
            ),
            (
                "easyconfigs:\n  - x.eb: {options: {--rebuild: true}}\n",
                "without its dashes",
            ),
            (
                "easyconfigs:\n  - x.eb: {options: {hooks: {a: b}}}\n",
                "a value is text",
            ),
            (
                "easyconfigs:\n  - x.eb: {options: {r: false}}\n",
                "no --disable- form",
            ),
            (
                "easyconfigs:\n  - x.eb: {options: {r: /x}}\n",
                "by its long name",
            ),
            (
                "easyconfigs:\n  - x.eb: {options: {a-b: \"a\\nb\"}}\n",
                "line break",
            ),
            ("easyconfigs:\n  - ''\n", "the easyconfig name is empty"),
            ("easyconfigs: [\n", "while parsing"),
        ] {
            let e = refused(text);
            assert!(e.contains(needed), "{text:?}: {e}");
        }
    }

    #[test]
    fn only_yml_and_yaml_files_are_easystacks() {
        for (p, want) in [
            ("rebuilds/x.yml", true),
            ("x.yaml", true),
            ("rebuilds/x.YML", true),
            ("x.YAML", true),
            ("x.list", false),
            ("x.yml.list", false),
            ("yml", false),
        ] {
            assert_eq!(is_easystack(Path::new(p)), want, "{p}");
        }
    }
}
