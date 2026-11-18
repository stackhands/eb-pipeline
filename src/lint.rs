//! Placeholder lint for example site files.
//!
//! Examples must carry neutral values only: absolute paths under `/opt`,
//! `/srv` or `/scratch`, angle-bracket placeholders, and host names under
//! `example.org` or `.invalid`. An operator who keeps a private site file next
//! to the templates can name extra substrings to refuse through
//! `EB_PIPELINE_LINT_DENY` (comma-separated); the repository itself names none.

const ROOTS: [&str; 3] = ["/opt", "/srv", "/scratch"];
const FILE_EXTS: [&str; 16] = [
    "eb", "json", "toml", "list", "lock", "sh", "log", "txt", "py", "yaml", "yml", "cfg", "conf",
    "md", "gz", "tar",
];

fn path_ok(p: &str) -> bool {
    ROOTS
        .iter()
        .any(|r| p == *r || p.starts_with(&format!("{r}/")))
}

fn host_ok(h: &str) -> bool {
    h == "example.org" || h.ends_with(".example.org") || h == "invalid" || h.ends_with(".invalid")
}

fn looks_like_host(t: &str) -> bool {
    if t.contains('/') || !t.contains('.') {
        return false;
    }
    if !t
        .chars()
        .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '.' || c == '-')
    {
        return false;
    }
    let last = t.rsplit('.').next().unwrap_or("");
    last.len() >= 2
        && last.chars().all(|c| c.is_ascii_lowercase())
        && !FILE_EXTS.contains(&last)
        && !t.starts_with('-')
        && !t.starts_with('.')
}

fn check_token(key: &str, raw: &str, out: &mut Vec<String>) {
    let t = raw.trim_matches(|c| matches!(c, '\'' | '"' | '(' | ')' | '[' | ']'));
    if t.is_empty() || (t.starts_with('<') && t.ends_with('>')) {
        return;
    }
    let t = t.rsplit('@').next().unwrap_or(t);
    if t.starts_with('/') {
        if !path_ok(t) {
            out.push(format!(
                "{key}: path `{t}` is not under /opt, /srv or /scratch and not a placeholder"
            ));
        }
    } else if looks_like_host(t) && !host_ok(t) {
        out.push(format!(
            "{key}: host `{t}` is not under example.org or .invalid"
        ));
    }
}

fn check_value(key: &str, v: &str, deny: &[String], out: &mut Vec<String>) {
    for d in deny {
        if v.contains(d.as_str()) {
            out.push(format!("{key}: value contains the denied text `{d}`"));
        }
    }
    for word in v.split_whitespace() {
        if let Some((_, rest)) = word.split_once("://") {
            let authority = rest.split(['/', '?', '#']).next().unwrap_or("");
            let host_port = authority
                .rsplit_once('@')
                .map_or(authority, |(_, host)| host);
            let host = host_port.split(':').next().unwrap_or("");
            if !host_ok(host) {
                out.push(format!(
                    "{key}: host `{host}` is not under example.org or .invalid"
                ));
            }
            continue;
        }
        for raw in word.split([':', ',', '=', ';']) {
            check_token(key, raw, out);
        }
    }
}

fn walk(key: &str, v: &toml::Value, deny: &[String], out: &mut Vec<String>) {
    match v {
        toml::Value::String(s) => check_value(key, s, deny, out),
        toml::Value::Array(a) => a.iter().for_each(|x| walk(key, x, deny, out)),
        toml::Value::Table(t) => t.iter().for_each(|(k, x)| {
            let kk = if key.is_empty() {
                k.clone()
            } else {
                format!("{key}.{k}")
            };
            walk(&kk, x, deny, out)
        }),
        _ => {}
    }
}

/// Return one message per value that looks site-specific.
pub fn lint_example(text: &str) -> Vec<String> {
    lint_example_with(text, &[])
}

/// As [`lint_example`], and also refuse any string value containing one of
/// `deny`.
pub fn lint_example_with(text: &str, deny: &[String]) -> Vec<String> {
    let mut out = Vec::new();
    match toml::from_str::<toml::Value>(text) {
        Ok(v) => walk("", &v, deny, &mut out),
        Err(e) => out.push(format!("not valid TOML: {e}")),
    }
    out
}

/// The extra substrings named by `EB_PIPELINE_LINT_DENY`, comma-separated;
/// blank entries are dropped.
pub fn deny_from_env() -> Vec<String> {
    std::env::var("EB_PIPELINE_LINT_DENY")
        .unwrap_or_default()
        .split(',')
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(str::to_string)
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn accepts_placeholders() {
        let t = r#"
a = "/opt/eb/sources"
b = ["/scratch/x:/srv/y"]
c = "<scratch>/tmp"
d = "-p batch -N 1 -t 15:00"
e = "https://example.org/x"
f = "user@node.example.org"
g = "foss/2026.1 GCC:-march=x86-64-v3 Foo-1.0-foss-2025a.eb"
h = "https://user:password@hooks.example.org/x"
"#;
        assert!(lint_example(t).is_empty(), "{:?}", lint_example(t));
    }

    #[test]
    fn rejects_site_values() {
        let t = r#"
a = "/home/builder/eb"
b = "login.cluster.net"
c = "https://git.corp.example.com/x"
d = "-o /projects/lab/x"
"#;
        assert_eq!(lint_example(t).len(), 4, "{:?}", lint_example(t));
    }

    #[test]
    fn deny_list_refuses_named_text() {
        let t = "a = \"/opt/eb/layout-x/cpu\"\nb = [\"/opt/eb/ok\"]\n";
        assert!(lint_example(t).is_empty());
        let deny = vec!["layout-x".to_string()];
        let hits = lint_example_with(t, &deny);
        assert_eq!(hits.len(), 1, "{hits:?}");
        assert!(hits[0].starts_with("a:"), "{hits:?}");
    }
}
