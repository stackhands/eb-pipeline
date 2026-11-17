//! Build list lines from an EasyBuild easystack: the file `eb --easystack`
//! builds from, which `eb-stack rebuild-set`, `eb-stack stack order
//! --easystack-out` and `eb-stack stack easystack` write.
//!
//! Each entry becomes the line of the build EasyBuild 5 makes of it: a name
//! without `.eb` gets it, and the entry's `options` become `eb` options as
//! EasyBuild's `opts_dict_to_eb_opts` writes them, in the order of their
//! names. Only `easyconfigs` is read; EasyBuild ignores the other top-level
//! keys as well.

use crate::util::{quote, ListLine};
use serde_yaml::{Mapping, Value};
use std::path::Path;

/// Whether the build list `path` is an easystack: a `.yml` or `.yaml` file.
pub fn is_easystack(path: &Path) -> bool {
    path.extension().is_some_and(|e| e == "yml" || e == "yaml")
}

/// The build list lines of the easystack `text`, in file order.
pub fn parse(text: &str) -> Result<Vec<ListLine>, String> {
    let doc: Value = serde_yaml::from_str(text).map_err(|e| e.to_string())?;
    let entries = match doc.get("easyconfigs") {
        Some(Value::Sequence(s)) => s,
        Some(_) => return Err("`easyconfigs` is not a list; start each entry with `- `".into()),
        None => return Err("there is no top-level `easyconfigs` key".into()),
    };
    entries
        .iter()
        .enumerate()
        .map(|(i, e)| entry(e).map_err(|m| format!("entry {}: {m}", i + 1)))
        .collect()
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
    let words: Vec<&String> = std::iter::once(&easyconfig).chain(&args).collect();
    if let Some(w) = words.iter().find(|w| w.contains(['\'', '\n', '\r'])) {
        return Err(format!(
            "{easyconfig}: `{w}` holds a single quote or a line break, which a build list line cannot"
        ));
    }
    Ok(ListLine {
        raw: words.iter().map(|w| quote(w)).collect::<Vec<_>>().join(" "),
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
/// None]`, which in Python holds the number 1 as well.
fn flag(v: &Value) -> Option<bool> {
    match v {
        Value::Null => Some(true),
        Value::Bool(b) => Some(*b),
        Value::String(s) => yaml11_bool(s),
        Value::Number(n) if n.as_u64() == Some(1) => Some(true),
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
        Value::Number(n) => Err(format!(
            "the option `{k}`: put `{n}` in quotes; Python and eb-pipeline write a number with a point differently"
        )),
        _ => Err(format!(
            "the option `{k}`: a value is text, a number, a boolean or a list of them"
        )),
    }
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
    fn yaml_1_1_booleans_are_booleans_as_in_easybuild() {
        let l =
            &parse("easyconfigs:\n  - x.eb: {options: {rebuild: on, robot: off, a-b: [yes, 1]}}\n")
                .unwrap()[0];
        assert_eq!(l.args, ["--a-b=True,1", "--rebuild", "--disable-robot"]);
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
                "easyconfigs:\n  - x.eb: {options: {parallel: 1.5}}\n",
                "put `1.5` in quotes",
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
                "easyconfigs:\n  - x.eb: {options: {a-b: \"it's\"}}\n",
                "holds a single quote",
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
            ("x.list", false),
            ("x.yml.list", false),
            ("yml", false),
        ] {
            assert_eq!(is_easystack(Path::new(p)), want, "{p}");
        }
    }
}
