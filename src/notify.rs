//! The `[notify]` webhook: a POST of a run directory's `summary.json` after
//! a run has written it. The URL is a secret; eb-pipeline prints only its
//! scheme and host, and passes it to curl on standard input.

use crate::run::log;
use crate::schema::{Notify, Site};
use std::io::Write;
use std::path::Path;
use std::process::{Command, Stdio};

/// Why a webhook URL cannot be used. The reason never holds the URL.
pub fn check_url(url: &str) -> Result<(), String> {
    let rest = url
        .strip_prefix("https://")
        .or_else(|| url.strip_prefix("http://"))
        .ok_or("must start with http:// or https://")?;
    if url
        .chars()
        .any(|c| c.is_whitespace() || c.is_control() || c == '"' || c == '\\')
    {
        return Err("must not hold a space, a quote, a backslash or a control character".into());
    }
    if host(rest).is_empty() {
        return Err("names no host".into());
    }
    Ok(())
}

/// Host and port of what follows `scheme://`, without any user name.
fn host(rest: &str) -> &str {
    let end = rest.find(['/', '?', '#']).unwrap_or(rest.len());
    let authority = &rest[..end];
    authority.rsplit_once('@').map_or(authority, |(_, h)| h)
}

/// The URL as eb-pipeline prints it: scheme and host only.
pub fn redacted(url: &str) -> String {
    match url.split_once("://") {
        Some((scheme, rest)) => format!("{scheme}://{}/...", host(rest)),
        None => "(webhook)".into(),
    }
}

/// `text` with the URL, and the part of it after the host, replaced.
fn scrub(text: &str, url: &str) -> String {
    let mut out = text.replace(url, &redacted(url));
    if let Some((_, rest)) = url.split_once("://") {
        let tail = &rest[rest.find(['/', '?', '#']).unwrap_or(rest.len())..];
        if tail.len() > 1 {
            out = out.replace(tail, "/...");
        }
        if let Some((user, _)) = rest[..rest.len() - tail.len()].rsplit_once('@') {
            out = out.replace(user, "...");
        }
    }
    out
}

/// POST `summary` when `[notify]` asks for it after a target's run that
/// ended with `exit_code`. Each outcome is an event on stderr, and none
/// changes the run's exit code.
pub fn send(site: &Site, target: &str, exit_code: i32, summary: &Path, wrapper: &[String]) {
    let Some(n) = site.notify_on() else {
        return;
    };
    if n.when == "problems" && exit_code == 0 {
        log(
            "INFO",
            "notify",
            target,
            "the run exited 0 and notify.when is problems; nothing sent",
        );
        return;
    }
    let url = match (&n.webhook, &n.webhook_env) {
        (Some(u), _) => u.clone(),
        (None, Some(var)) => match std::env::var(var) {
            Ok(v) if !v.is_empty() => v,
            _ => {
                log(
                    "WARN",
                    "notify",
                    target,
                    &format!("{var} is unset or empty; nothing sent"),
                );
                return;
            }
        },
        (None, None) => return,
    };
    if let Err(why) = check_url(&url) {
        let from = n.webhook_env.as_deref().unwrap_or("notify.webhook");
        log(
            "WARN",
            "notify",
            target,
            &format!("the URL in {from} {why}; nothing sent"),
        );
        return;
    }
    let shown = redacted(&url);
    let name = summary.file_name().unwrap_or_default().to_string_lossy();
    match post(n, &url, summary, wrapper) {
        Ok(()) => log(
            "INFO",
            "notify",
            target,
            &format!("{name} posted to {shown}"),
        ),
        Err(e) => log(
            "WARN",
            "notify",
            target,
            &format!("POST of {name} to {shown} failed: {}", scrub(&e, &url)),
        ),
    }
}

fn post(n: &Notify, url: &str, body: &Path, wrapper: &[String]) -> Result<(), String> {
    let mut argv = wrapper.to_vec();
    argv.push(n.curl.clone());
    argv.extend(
        [
            "--silent",
            "--show-error",
            "--fail",
            "--max-time",
            &n.timeout_seconds.to_string(),
            "--request",
            "POST",
            "--header",
            "Content-Type: application/json",
            "--header",
            "Expect:",
            "--data-binary",
        ]
        .map(str::to_string),
    );
    argv.push(format!("@{}", body.display()));
    argv.extend(["--config".into(), "-".into()]);
    let program = argv.remove(0);
    let mut child = Command::new(&program)
        .args(argv)
        .stdin(Stdio::piped())
        .stdout(Stdio::null())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|e| format!("cannot start {program}: {e}"))?;
    // check_url refused the quote and backslash that would need escaping here
    let written = child
        .stdin
        .take()
        .map(|mut i| writeln!(i, "url = \"{url}\""));
    let out = child
        .wait_with_output()
        .map_err(|e| format!("wait for {}: {e}", n.curl))?;
    if let Some(Err(e)) = written {
        return Err(format!("cannot give {} the URL: {e}", n.curl));
    }
    if out.status.success() {
        return Ok(());
    }
    let code = out
        .status
        .code()
        .map_or_else(|| "on a signal".into(), |c| c.to_string());
    let err = String::from_utf8_lossy(&out.stderr).trim().to_string();
    Err(if err.is_empty() {
        format!("curl exited {code}")
    } else {
        format!("curl exited {code}: {err}")
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_printed_url_keeps_only_its_scheme_and_host() {
        assert_eq!(
            redacted("https://bot:pw@hooks.example.org:8443/p/s3cr3t?t=1"),
            "https://hooks.example.org:8443/..."
        );
        assert_eq!(redacted("http://h.invalid"), "http://h.invalid/...");
    }

    #[test]
    fn curl_output_loses_the_url_path_and_user() {
        let url = "https://bot:pw@hooks.example.org/p/s3cr3t?t=1";
        let s = scrub(&format!("error at {url} or /p/s3cr3t?t=1 by bot:pw"), url);
        assert!(!s.contains("s3cr3t") && !s.contains("pw"), "{s}");
    }

    #[test]
    fn urls_are_checked_without_being_repeated() {
        for (bad, why) in [
            (
                "ftp://s3cr3t.example.org/x",
                "must start with http:// or https://",
            ),
            ("https:///s3cr3t", "names no host"),
            ("https://h.example.org/a b", "must not hold a space"),
            ("https://h.example.org/\"x", "must not hold a space"),
        ] {
            let e = check_url(bad).unwrap_err();
            assert!(e.starts_with(why), "{bad}: {e}");
            assert!(!e.contains("s3cr3t") && !e.contains("example"), "{e}");
        }
        assert!(check_url("https://hooks.example.org/x?y=1").is_ok());
    }
}
