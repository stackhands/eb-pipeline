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

/// Tokens of every occurrence of the option on a line, and whether it occurs.
fn tokens(sel: &Select, line: &ListLine) -> Result<(bool, Vec<String>), Fail> {
    let mut found = false;
    let mut out = Vec::new();
    let eq = format!("{}=", sel.option);
    let mut i = 0;
    while i < line.args.len() {
        let a = &line.args[i];
        let value = if a == &sel.option {
            found = true;
            i += 1;
            match line.args.get(i) {
                Some(value) if !value.starts_with('-') => Some(value.as_str()),
                _ => {
                    return Err(Fail::usage(format!(
                        "selector option `{}` needs a value in `{}`",
                        sel.option, line.raw
                    )));
                }
            }
        } else if let Some(v) = a.strip_prefix(&eq) {
            found = true;
            if v.is_empty() {
                return Err(Fail::usage(format!(
                    "selector option `{}` needs a value in `{}`",
                    sel.option, line.raw
                )));
            }
            Some(v)
        } else {
            None
        };
        if let Some(v) = value {
            let tokens = v
                .split(sel.separator.as_str())
                .map(str::trim)
                .filter(|token| !token.is_empty())
                .map(str::to_string)
                .collect::<Vec<_>>();
            if tokens.is_empty() {
                return Err(Fail::usage(format!(
                    "selector option `{}` needs a value in `{}`",
                    sel.option, line.raw
                )));
            }
            out.extend(tokens);
        }
        i += 1;
    }
    Ok((found, out))
}

fn stripped(sel: &Select, args: &[String]) -> Vec<String> {
    let eq = format!("{}=", sel.option);
    let mut out = Vec::new();
    let mut i = 0;
    while i < args.len() {
        if args[i] == sel.option {
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

/// Decide whether `line` is planned for the target that owns `sel`.
pub fn select_line(sel: &Select, line: &ListLine) -> Result<Selected, Fail> {
    let (found, toks) = tokens(sel, line)?;
    let planned = if found {
        toks.iter().any(|t| t == &sel.value)
    } else {
        sel.when_absent != "none"
    };
    let args = if sel.strip {
        stripped(sel, &line.args)
    } else {
        line.args.clone()
    };
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
}
