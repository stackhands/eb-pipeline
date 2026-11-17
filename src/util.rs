//! Small helpers.

use crate::exit::Fail;

/// Split a string on whitespace, honouring single and double quotes.
pub fn split_args(s: &str) -> Result<Vec<String>, Fail> {
    let mut out = Vec::new();
    let mut cur = String::new();
    let mut quote: Option<char> = None;
    let mut have = false;
    for c in s.chars() {
        match quote {
            Some(q) if c == q => quote = None,
            Some(_) => cur.push(c),
            None if c == '\'' || c == '"' => {
                quote = Some(c);
                have = true;
            }
            None if c.is_whitespace() => {
                if have || !cur.is_empty() {
                    out.push(std::mem::take(&mut cur));
                    have = false;
                }
            }
            None => cur.push(c),
        }
    }
    if quote.is_some() {
        return Err(Fail::usage(format!("unterminated quote in `{s}`")));
    }
    if have || !cur.is_empty() {
        out.push(cur);
    }
    Ok(out)
}

/// Quote `a` for a shell, or for a build list line: as it is when it holds
/// only characters a shell leaves alone, else in single quotes.
pub fn quote(a: &str) -> String {
    if !a.is_empty()
        && a.chars()
            .all(|c| c.is_ascii_alphanumeric() || "-_./=:,@%+".contains(c))
    {
        a.to_string()
    } else {
        format!("'{}'", a.replace('\'', "'\\''"))
    }
}

/// Whether `s` is a log stamp: a UTC time as `YYYYMMDD_HHMMSS`, the name
/// `eb-keep-logs rotate` ages as a run directory.
pub fn is_log_stamp(s: &str) -> bool {
    let b = s.as_bytes();
    b.len() == 15
        && b[8] == b'_'
        && b[..8].iter().chain(&b[9..]).all(u8::is_ascii_digit)
        && chrono::NaiveDateTime::parse_from_str(s, "%Y%m%d_%H%M%S").is_ok()
}

/// Parse dotted numbers (`1.2.3`); a leading `v` is accepted.
pub fn parse_version(s: &str) -> Option<Vec<u64>> {
    let s = s.trim().trim_start_matches('v');
    if s.is_empty() {
        return None;
    }
    s.split('.').map(|p| p.parse::<u64>().ok()).collect()
}

/// Compare two dotted versions; missing components count as zero.
pub fn cmp_version(a: &[u64], b: &[u64]) -> std::cmp::Ordering {
    let n = a.len().max(b.len());
    for i in 0..n {
        let x = a.get(i).copied().unwrap_or(0);
        let y = b.get(i).copied().unwrap_or(0);
        if x != y {
            return x.cmp(&y);
        }
    }
    std::cmp::Ordering::Equal
}

/// First dotted-number token of a `--version` output.
pub fn find_version(output: &str) -> Option<Vec<u64>> {
    for tok in output.split(|c: char| c.is_whitespace() || c == ',' || c == '(' || c == ')') {
        let t = tok.trim_start_matches('v');
        if t.chars().next().is_some_and(|c| c.is_ascii_digit()) {
            let core: String = t
                .chars()
                .take_while(|c| c.is_ascii_digit() || *c == '.')
                .collect();
            if let Some(v) = parse_version(core.trim_end_matches('.')) {
                return Some(v);
            }
        }
    }
    None
}

/// One line of a build list.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ListLine {
    /// The line as written, trimmed.
    pub raw: String,
    /// Easyconfig file name or path.
    pub easyconfig: String,
    /// Remaining easybuild arguments.
    pub args: Vec<String>,
}

/// Read build list lines: blank lines and `#` comments are skipped.
pub fn parse_list(text: &str) -> Result<Vec<ListLine>, Fail> {
    let mut out = Vec::new();
    for l in text.lines() {
        let t = l.trim();
        if t.is_empty() || t.starts_with('#') {
            continue;
        }
        let mut parts = split_args(t)?;
        let easyconfig = parts.remove(0);
        out.push(ListLine {
            raw: t.to_string(),
            easyconfig,
            args: parts,
        });
    }
    Ok(out)
}

/// Package name and version of an easyconfig file name `Name-version[-toolchain].eb`.
pub fn package_of(easyconfig: &str) -> (String, Option<String>) {
    let base = easyconfig.rsplit('/').next().unwrap_or(easyconfig);
    let stem = base.strip_suffix(".eb").unwrap_or(base);
    let parts: Vec<&str> = stem.split('-').collect();
    let mut name = Vec::new();
    let mut i = 0;
    while i < parts.len() {
        if i > 0 && parts[i].chars().next().is_some_and(|c| c.is_ascii_digit()) {
            break;
        }
        name.push(parts[i]);
        i += 1;
    }
    (name.join("-"), parts.get(i).map(|s| s.to_string()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn split_quotes() {
        assert_eq!(
            split_args("-p batch --comment 'a b' \"\"").unwrap(),
            vec!["-p", "batch", "--comment", "a b", ""]
        );
        assert!(split_args("'open").is_err());
    }

    #[test]
    fn versions() {
        assert_eq!(find_version("eb-stack 0.7.4 (abc)"), Some(vec![0, 7, 4]));
        assert_eq!(find_version("tool v1.2"), Some(vec![1, 2]));
        assert_eq!(cmp_version(&[1, 2], &[1, 2, 0]), std::cmp::Ordering::Equal);
        assert_eq!(cmp_version(&[0, 7, 4], &[0, 10]), std::cmp::Ordering::Less);
    }

    #[test]
    fn package_names() {
        assert_eq!(
            package_of("GROMACS-2025.2-foss-2025a.eb"),
            ("GROMACS".into(), Some("2025.2".into()))
        );
        assert_eq!(
            package_of("/x/y/R-bundle-Bioconductor-3.20-foss-2025a.eb"),
            ("R-bundle-Bioconductor".into(), Some("3.20".into()))
        );
    }

    #[test]
    fn list_lines() {
        let l = parse_list("# c\n\nFoo-1.0-foss-2025a.eb --rebuild\n").unwrap();
        assert_eq!(l.len(), 1);
        assert_eq!(l[0].args, vec!["--rebuild"]);
    }

    #[test]
    fn log_stamps() {
        for ok in ["20261116_024713", "20240229_235959", "19700101_000000"] {
            assert!(is_log_stamp(ok), "{ok}");
        }
        for bad in [
            "",
            "2026-11-16",
            "20261116",
            "20261116_02471",
            "20261116-024713",
            "20261116_0247130",
            "20261301_000000",
            "20250229_000000",
            "20261116_240000",
            "20261116_006000",
            "+2026116_024713",
            " 2026116_024713",
            "../20261116_0247",
        ] {
            assert!(!is_log_stamp(bad), "{bad:?}");
        }
    }
}
