//! Per-target selection of build list lines.

use crate::exit::Fail;
use crate::schema::Select;
use crate::util::ListLine;

/// Outcome of applying a selector to one line.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Selected {
    /// Whether the line is planned for the target.
    pub planned: bool,
    /// The line's arguments, with the selector option removed when `strip` is set.
    pub args: Vec<String>,
}

/// Tokens of every occurrence of `option` on a line, and whether it occurs.
fn tokens(option: &str, separator: &str, line: &ListLine) -> Result<(bool, Vec<String>), Fail> {
    let needs_value = || {
        Fail::usage(format!(
            "selector option `{option}` needs a value in `{}`",
            line.raw
        ))
    };
    let mut found = false;
    let mut out = Vec::new();
    let eq = format!("{option}=");
    let mut i = 0;
    while i < line.args.len() {
        let a = &line.args[i];
        let value = if a == option {
            found = true;
            i += 1;
            match line.args.get(i) {
                Some(value) if !value.starts_with('-') => Some(value.as_str()),
                _ => return Err(needs_value()),
            }
        } else if let Some(v) = a.strip_prefix(&eq) {
            found = true;
            if v.is_empty() {
                return Err(needs_value());
            }
            Some(v)
        } else {
            None
        };
        if let Some(v) = value {
            let tokens = v
                .split(separator)
                .map(str::trim)
                .filter(|token| !token.is_empty())
                .map(str::to_string)
                .collect::<Vec<_>>();
            if tokens.is_empty() {
                return Err(needs_value());
            }
            out.extend(tokens);
        }
        i += 1;
    }
    Ok((found, out))
}

/// `args` without any occurrence of `option`, in either form.
fn stripped(option: &str, args: &[String]) -> Vec<String> {
    let eq = format!("{option}=");
    let mut out = Vec::new();
    let mut i = 0;
    while i < args.len() {
        if args[i] == option {
            i += 2;
            continue;
        }
        if !args[i].starts_with(&eq) {
            out.push(args[i].clone());
        }
        i += 1;
    }
    out
}

/// Whether one of the tokens names the target: its `value` or one of its `aliases`.
fn names_target(sel: &Select, tokens: &[String]) -> bool {
    tokens
        .iter()
        .any(|t| t == &sel.value || sel.aliases.iter().any(|alias| alias == t))
}

/// Decide whether `line` is planned for the target that owns `sel`.
///
/// The line is included when `option` names the target, or, without the option, when
/// `when_absent` is `all`. An `exclude_option` that names the target then leaves the line
/// out: an exclusion takes priority over an inclusion.
pub fn select_line(sel: &Select, line: &ListLine) -> Result<Selected, Fail> {
    let (found, included) = tokens(&sel.option, &sel.separator, line)?;
    let mut planned = if found {
        names_target(sel, &included)
    } else {
        sel.when_absent != "none"
    };
    let mut args = line.args.clone();
    if sel.strip {
        args = stripped(&sel.option, &args);
    }
    if let Some(exclude) = &sel.exclude_option {
        let (_, excluded) = tokens(exclude, &sel.separator, line)?;
        if names_target(sel, &excluded) {
            planned = false;
        }
        if sel.strip {
            args = stripped(exclude, &args);
        }
    }
    Ok(Selected { planned, args })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sel(strip: bool, when_absent: &str, separator: &str) -> Select {
        Select {
            option: "--only-targets".into(),
            value: "a".into(),
            separator: separator.into(),
            when_absent: when_absent.into(),
            strip,
            aliases: Vec::new(),
            exclude_option: None,
        }
    }

    /// A selector for target `a` of group `site`, with an exclude option.
    fn grouped(strip: bool) -> Select {
        Select {
            aliases: vec!["site".into()],
            exclude_option: Some("--not-targets".into()),
            ..sel(strip, "all", ",")
        }
    }

    fn line(args: &[&str]) -> ListLine {
        ListLine {
            raw: std::iter::once("Foo-1.0.eb")
                .chain(args.iter().copied())
                .collect::<Vec<_>>()
                .join(" "),
            easyconfig: "Foo-1.0.eb".into(),
            args: args.iter().map(|s| s.to_string()).collect(),
        }
    }

    fn selected(sel: &Select, line: &ListLine) -> Selected {
        select_line(sel, line).unwrap()
    }

    #[test]
    fn absent_option_follows_when_absent() {
        assert!(selected(&sel(false, "all", ","), &line(&["--rebuild"])).planned);
        assert!(!selected(&sel(false, "none", ","), &line(&["--rebuild"])).planned);
    }

    #[test]
    fn matching_and_non_matching_tokens() {
        let s = sel(false, "all", ",");
        assert!(selected(&s, &line(&["--only-targets=a"])).planned);
        assert!(!selected(&s, &line(&["--only-targets=b"])).planned);
        // a different option sharing a prefix does not match
        assert!(selected(&s, &line(&["--only-targets-x=b"])).planned);
    }

    #[test]
    fn multiple_tokens_and_separator() {
        let s = sel(false, "none", ",");
        assert!(selected(&s, &line(&["--only-targets=b,c,a"])).planned);
        assert!(!selected(&s, &line(&["--only-targets=b,c"])).planned);
        let s = sel(false, "none", "+");
        assert!(selected(&s, &line(&["--only-targets=b+a"])).planned);
        assert!(!selected(&s, &line(&["--only-targets=b,a"])).planned);
    }

    #[test]
    fn separate_value_token_form() {
        let s = sel(true, "none", ",");
        let r = selected(&s, &line(&["--rebuild", "--only-targets", "b,a", "-f"]));
        assert!(r.planned);
        assert_eq!(r.args, ["--rebuild", "-f"]);
    }

    #[test]
    fn separate_option_without_a_value_is_refused() {
        let s = sel(true, "all", ",");
        for args in [
            &["--only-targets"][..],
            &["--only-targets", "--rebuild"][..],
            &["--only-targets="][..],
            &["--only-targets=,"][..],
            &["--only-targets", ","][..],
        ] {
            let err = select_line(&s, &line(args)).unwrap_err();
            assert_eq!(err.code, 2);
            assert!(err.msg.contains("--only-targets"), "{}", err.msg);
        }
    }

    #[test]
    fn strip_removes_the_option_only_when_set() {
        let l = line(&["--rebuild", "--only-targets=a", "--x"]);
        assert_eq!(
            selected(&sel(true, "all", ","), &l).args,
            ["--rebuild", "--x"]
        );
        assert_eq!(selected(&sel(false, "all", ","), &l).args, l.args);
    }

    #[test]
    fn an_alias_selects_like_the_value() {
        let s = grouped(false);
        assert!(selected(&s, &line(&["--only-targets=site"])).planned);
        assert!(selected(&s, &line(&["--only-targets=b,site"])).planned);
        assert!(!selected(&s, &line(&["--only-targets=b,other-site"])).planned);
        // without aliases the group name selects nothing
        assert!(!selected(&sel(false, "all", ","), &line(&["--only-targets=site"])).planned);
    }

    #[test]
    fn an_exclusion_takes_priority_over_an_inclusion() {
        let s = grouped(false);
        // named by value or alias under the exclude option: left out
        assert!(!selected(&s, &line(&["--not-targets=a"])).planned);
        assert!(!selected(&s, &line(&["--not-targets=site"])).planned);
        assert!(!selected(&s, &line(&["--only-targets=a", "--not-targets=a"])).planned);
        assert!(!selected(&s, &line(&["--only-targets=site", "--not-targets=a"])).planned);
        assert!(!selected(&s, &line(&["--only-targets=a", "--not-targets", "b,site"])).planned);
        // an exclusion of other targets changes nothing
        assert!(selected(&s, &line(&["--not-targets=b"])).planned);
        assert!(selected(&s, &line(&["--only-targets=a", "--not-targets=b,c"])).planned);
        assert!(!selected(&s, &line(&["--only-targets=b", "--not-targets=c"])).planned);
    }

    #[test]
    fn an_exclusion_is_ignored_without_exclude_option() {
        let s = sel(false, "all", ",");
        assert!(selected(&s, &line(&["--not-targets=a"])).planned);
    }

    #[test]
    fn strip_removes_the_exclude_option_too() {
        let l = line(&["--rebuild", "--only-targets=a", "--not-targets", "b", "--x"]);
        assert_eq!(selected(&grouped(true), &l).args, ["--rebuild", "--x"]);
        assert_eq!(selected(&grouped(false), &l).args, l.args);
    }

    #[test]
    fn exclude_option_without_a_value_is_refused() {
        let s = grouped(false);
        for args in [&["--not-targets"][..], &["--not-targets="][..], &["--not-targets=,"][..]] {
            let err = select_line(&s, &line(args)).unwrap_err();
            assert_eq!(err.code, 2);
            assert!(err.msg.contains("--not-targets"), "{}", err.msg);
        }
    }
}
