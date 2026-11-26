//! `[notify]` POSTs a run's summary.json through curl on `PATH`, and never
//! prints the webhook URL past its host.

mod common;
use common::{code, eb_pipeline, run, setup, stderr, stdout, write_exec, STAMP};
use serde_json::Value;

const LIST: &str = "Foo-1.0-foss-2025a.eb\n";
const SECRET: &str = "s3cr3t-token";

fn assert_no_secret(t: &common::T, out: &std::process::Output) {
    for text in [stderr(out), stdout(out)] {
        assert!(
            !text.contains(SECRET) && !text.contains("hunter2"),
            "{text}"
        );
    }
    for f in [
        t.run_dir(STAMP).join("summary.json"),
        t.run_dir(STAMP).join("summary.html"),
        t.root().join("result-cpu.json"),
    ] {
        let text = std::fs::read_to_string(&f).unwrap_or_default();
        assert!(!text.contains(SECRET), "{}", f.display());
    }
}

const CURL_STUB: &str = r#"#!/bin/sh
D="${STUB_DIR:?}"
n=$(( $(cat "$D/curl.count" 2>/dev/null || echo 0) + 1 ))
printf '%s\n' "$n" > "$D/curl.count"
printf '%s\0' "$@" > "$D/curl.$n.args"
cat > "$D/curl.$n.stdin"
prev=
for a in "$@"; do
  if [ "$prev" = "--data-binary" ]; then
    case "$a" in
      @*) cp "${a#@}" "$D/curl.$n.body" ;;
    esac
  fi
  prev=$a
done
exit "${STUB_CURL_EXIT:-0}"
"#;

fn install_curl(t: &common::T) {
    write_exec(&t.stubs.join("curl"), CURL_STUB);
}

fn curl_count(t: &common::T) -> usize {
    std::fs::read_to_string(t.root().join("curl.count"))
        .ok()
        .and_then(|text| text.trim().parse().ok())
        .unwrap_or(0)
}

fn curl_args(t: &common::T, n: usize) -> Vec<String> {
    let bytes = std::fs::read(t.root().join(format!("curl.{n}.args"))).unwrap_or_default();
    bytes
        .split(|byte| *byte == 0)
        .filter(|part| !part.is_empty())
        .map(|part| String::from_utf8(part.to_vec()).unwrap())
        .collect()
}

#[test]
fn a_run_posts_its_summary_to_the_webhook() {
    let t = setup("[notify]\nwebhook_env = \"EB_TEST_WEBHOOK\"\n");
    install_curl(&t);
    let url = format!("http://hook:hunter2@hooks.example/hook/{SECRET}?key={SECRET}");
    let o = run(&t, "cpu", STAMP, LIST, &[("EB_TEST_WEBHOOK", &url)]);
    assert_eq!(code(&o), 0, "{}", stderr(&o));
    assert_eq!(curl_count(&t), 1);
    let args = curl_args(&t, 1);
    assert!(
        args.iter()
            .any(|arg| arg.eq_ignore_ascii_case("Content-Type: application/json")),
        "{args:?}"
    );
    let body = std::fs::read(t.root().join("curl.1.body")).unwrap();
    let posted: Value = serde_json::from_slice(&body).unwrap();
    assert_eq!(posted, t.summary(STAMP));
    assert_eq!(posted["kind"], "summary");
    assert_eq!(posted["targets"][0]["name"], "cpu");
    let stdin = std::fs::read_to_string(t.root().join("curl.1.stdin")).unwrap();
    assert!(stdin.contains(&url), "{stdin}");
    assert!(
        stderr(&o).contains("INFO [notify] cpu: summary.json posted to http://hooks.example/...\n"),
        "{}",
        stderr(&o)
    );
    assert_no_secret(&t, &o);
}

#[test]
fn a_webhook_that_fails_is_a_warning() {
    let t = setup("[notify]\nwebhook_env = \"EB_TEST_WEBHOOK\"\n");
    install_curl(&t);
    let url = format!("http://hooks.example/hook/{SECRET}");
    let o = run(
        &t,
        "cpu",
        STAMP,
        LIST,
        &[("EB_TEST_WEBHOOK", &url), ("STUB_CURL_EXIT", "22")],
    );
    assert_eq!(code(&o), 0, "{}", stderr(&o));
    assert_eq!(curl_count(&t), 1);
    assert!(
        stderr(&o).contains(
            "WARN [notify] cpu: POST of summary.json to http://hooks.example/... failed: curl exited 22"
        ),
        "{}",
        stderr(&o)
    );
    assert_no_secret(&t, &o);
}

#[test]
fn an_unreachable_webhook_is_a_warning() {
    let t = setup(&format!(
        "[notify]\nwebhook = \"http://hooks.example/{SECRET}\"\ntimeout_seconds = 5\n"
    ));
    install_curl(&t);
    let o = run(
        &t,
        "cpu",
        STAMP,
        LIST,
        &[
            ("STUB_EB_FAIL", "Foo-1.0-foss-2025a"),
            ("STUB_CURL_EXIT", "7"),
        ],
    );
    assert_eq!(code(&o), 1, "{}", stderr(&o));
    assert_eq!(curl_count(&t), 1);
    assert!(
        stderr(&o).contains(
            "WARN [notify] cpu: POST of summary.json to http://hooks.example/... failed: curl exited 7"
        ),
        "{}",
        stderr(&o)
    );
    assert_no_secret(&t, &o);
}

#[test]
fn when_problems_posts_only_after_a_run_that_did_not_exit_zero() {
    let t = setup(&format!(
        "[notify]\nwebhook = \"http://hooks.example/{SECRET}\"\nwhen = \"problems\"\n"
    ));
    install_curl(&t);
    let o = run(&t, "cpu", STAMP, LIST, &[]);
    assert_eq!(code(&o), 0, "{}", stderr(&o));
    assert_eq!(curl_count(&t), 0);
    assert!(
        stderr(&o).contains(
            "INFO [notify] cpu: the run exited 0 and notify.when is problems; nothing sent"
        ),
        "{}",
        stderr(&o)
    );
    let o = run(
        &t,
        "gpu",
        STAMP,
        LIST,
        &[("STUB_EB_FAIL", "Foo-1.0-foss-2025a")],
    );
    assert_eq!(code(&o), 1, "{}", stderr(&o));
    assert_eq!(curl_count(&t), 1);
    let body = std::fs::read(t.root().join("curl.1.body")).unwrap();
    let posted: Value = serde_json::from_slice(&body).unwrap();
    assert_eq!(posted["targets"].as_array().unwrap().len(), 2);
    assert_eq!(posted["exit_code"], 1);
    assert_no_secret(&t, &o);

    // the site file holds the URL; explain and check do not print it
    let site = t.site.display().to_string();
    for args in [
        vec!["explain", "--site", &site],
        vec!["check", "--site", &site],
    ] {
        let o = eb_pipeline(&t, &args, &[]);
        assert_eq!(code(&o), 0, "{}", stderr(&o));
        assert_no_secret(&t, &o);
    }
}

#[test]
fn an_unset_webhook_variable_sends_nothing() {
    let t = setup("[notify]\nwebhook_env = \"EB_TEST_WEBHOOK_UNSET\"\n");
    let o = run(&t, "cpu", STAMP, LIST, &[]);
    assert_eq!(code(&o), 0, "{}", stderr(&o));
    assert!(
        stderr(&o)
            .contains("WARN [notify] cpu: EB_TEST_WEBHOOK_UNSET is unset or empty; nothing sent"),
        "{}",
        stderr(&o)
    );
}

#[test]
fn notify_settings_are_checked_without_repeating_the_url() {
    for (table, why) in [
        (
            format!("webhook = \"ftp://{SECRET}.example.org/x\""),
            "notify.webhook: must start with http:// or https://",
        ),
        (
            format!("webhook = \"https://:443/{SECRET}\""),
            "notify.webhook: names no host",
        ),
        (
            format!("webhook = \"https://h.example.org/{SECRET}\"\nwebhook_env = \"X\""),
            "notify: set webhook or webhook_env, not both",
        ),
        (
            "when = \"weekly\"".to_string(),
            "notify: set webhook or webhook_env",
        ),
        (
            "webhook_env = \"X\"\nwhen = \"weekly\"".to_string(),
            "notify.when: expected always or problems, found `weekly`",
        ),
        (
            "webhook_env = \"1X\"".to_string(),
            "notify.webhook_env: `1X` is not a variable name",
        ),
    ] {
        let t = setup(&format!("[notify]\n{table}\n"));
        let site = t.site.display().to_string();
        let o = eb_pipeline(&t, &["check", "--site", &site], &[]);
        assert_eq!(code(&o), 2, "{table}: {}", stderr(&o));
        assert!(stderr(&o).contains(why), "{table}: {}", stderr(&o));
        assert!(!stderr(&o).contains(SECRET), "{}", stderr(&o));
    }
}

#[test]
fn an_unbracketed_ipv6_webhook_is_refused() {
    let t = setup(&format!(
        "[notify]\nwebhook = \"https://2001:db8::1:8443/{SECRET}\"\n"
    ));
    let site = t.site.display().to_string();
    let o = eb_pipeline(&t, &["check", "--site", &site], &[]);
    assert_eq!(code(&o), 2, "{}", stderr(&o));
    assert!(
        stderr(&o).contains("notify.webhook: an IPv6 address must be enclosed in brackets"),
        "{}",
        stderr(&o)
    );
    assert_no_secret(&t, &o);
}
