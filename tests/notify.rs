//! `[notify]` POSTs a run's summary.json to a webhook, here a local HTTP
//! server, and never prints the URL past its host.

mod common;
use common::{code, eb_pipeline, run, setup, stderr, stdout, STAMP};
use serde_json::Value;
use std::io::{Read, Write};
use std::net::{TcpListener, TcpStream};
use std::sync::mpsc::{channel, Receiver};
use std::time::Duration;

const LIST: &str = "Foo-1.0-foss-2025a.eb\n";
const SECRET: &str = "s3cr3t-token";

struct Request {
    line: String,
    headers: Vec<String>,
    body: Vec<u8>,
}

/// A server on 127.0.0.1 that answers each request with `status`.
fn serve(status: &'static str) -> (u16, Receiver<Request>) {
    let l = TcpListener::bind("127.0.0.1:0").unwrap();
    let port = l.local_addr().unwrap().port();
    let (tx, rx) = channel();
    std::thread::spawn(move || {
        for s in l.incoming() {
            let Ok(mut s) = s else { break };
            let req = read_request(&mut s);
            let _ = write!(
                s,
                "HTTP/1.1 {status}\r\nContent-Length: 0\r\nConnection: close\r\n\r\n"
            );
            if tx.send(req).is_err() {
                break;
            }
        }
    });
    (port, rx)
}

fn read_request(s: &mut TcpStream) -> Request {
    s.set_read_timeout(Some(Duration::from_secs(10))).unwrap();
    let mut buf = Vec::new();
    let mut chunk = [0u8; 4096];
    let head_end = loop {
        if let Some(i) = buf.windows(4).position(|w| w == b"\r\n\r\n") {
            break i + 4;
        }
        let n = s.read(&mut chunk).unwrap();
        assert!(n > 0, "connection closed inside the headers");
        buf.extend_from_slice(&chunk[..n]);
    };
    let head = String::from_utf8_lossy(&buf[..head_end]).into_owned();
    let mut lines = head.split("\r\n").filter(|l| !l.is_empty());
    let line = lines.next().unwrap_or_default().to_string();
    let headers: Vec<String> = lines.map(str::to_string).collect();
    let len: usize = headers
        .iter()
        .find_map(|h| {
            let (k, v) = h.split_once(':')?;
            k.eq_ignore_ascii_case("content-length")
                .then(|| v.trim().parse().ok())?
        })
        .unwrap_or(0);
    while buf.len() < head_end + len {
        let n = s.read(&mut chunk).unwrap();
        assert!(n > 0, "connection closed inside the body");
        buf.extend_from_slice(&chunk[..n]);
    }
    Request {
        line,
        headers,
        body: buf[head_end..head_end + len].to_vec(),
    }
}

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

#[test]
fn a_run_posts_its_summary_to_the_webhook() {
    let (port, rx) = serve("200 OK");
    let t = setup("[notify]\nwebhook_env = \"EB_TEST_WEBHOOK\"\n");
    let url = format!("http://hook:hunter2@127.0.0.1:{port}/hook/{SECRET}?key={SECRET}");
    let o = run(&t, "cpu", STAMP, LIST, &[("EB_TEST_WEBHOOK", &url)]);
    assert_eq!(code(&o), 0, "{}", stderr(&o));
    let req = rx.recv_timeout(Duration::from_secs(10)).unwrap();
    assert_eq!(
        req.line,
        format!("POST /hook/{SECRET}?key={SECRET} HTTP/1.1")
    );
    assert!(
        req.headers
            .iter()
            .any(|h| h.eq_ignore_ascii_case("content-type: application/json")),
        "{:?}",
        req.headers
    );
    let posted: Value = serde_json::from_slice(&req.body).unwrap();
    assert_eq!(posted, t.summary(STAMP));
    assert_eq!(posted["kind"], "summary");
    assert_eq!(posted["targets"][0]["name"], "cpu");
    assert!(
        stderr(&o).contains(&format!(
            "INFO [notify] cpu: summary.json posted to http://127.0.0.1:{port}/...\n"
        )),
        "{}",
        stderr(&o)
    );
    assert_no_secret(&t, &o);
}

#[test]
fn a_webhook_that_fails_is_a_warning() {
    let (port, rx) = serve("500 Internal Server Error");
    let t = setup("[notify]\nwebhook_env = \"EB_TEST_WEBHOOK\"\n");
    let url = format!("http://127.0.0.1:{port}/hook/{SECRET}");
    let o = run(&t, "cpu", STAMP, LIST, &[("EB_TEST_WEBHOOK", &url)]);
    assert_eq!(code(&o), 0, "{}", stderr(&o));
    assert!(rx.recv_timeout(Duration::from_secs(10)).is_ok());
    assert!(
        stderr(&o).contains(&format!(
            "WARN [notify] cpu: POST of summary.json to http://127.0.0.1:{port}/... failed: curl exited 22"
        )),
        "{}",
        stderr(&o)
    );
    assert_no_secret(&t, &o);
}

#[test]
fn an_unreachable_webhook_is_a_warning() {
    let port = {
        let l = TcpListener::bind("127.0.0.1:0").unwrap();
        l.local_addr().unwrap().port()
    };
    let t = setup(&format!(
        "[notify]\nwebhook = \"http://127.0.0.1:{port}/{SECRET}\"\ntimeout_seconds = 5\n"
    ));
    let o = run(
        &t,
        "cpu",
        STAMP,
        LIST,
        &[("STUB_EB_FAIL", "Foo-1.0-foss-2025a")],
    );
    assert_eq!(code(&o), 1, "{}", stderr(&o));
    assert!(
        stderr(&o).contains(&format!(
            "WARN [notify] cpu: POST of summary.json to http://127.0.0.1:{port}/... failed: curl exited 7"
        )),
        "{}",
        stderr(&o)
    );
    assert_no_secret(&t, &o);
}

#[test]
fn when_problems_posts_only_after_a_run_that_did_not_exit_zero() {
    let (port, rx) = serve("200 OK");
    let t = setup(&format!(
        "[notify]\nwebhook = \"http://127.0.0.1:{port}/{SECRET}\"\nwhen = \"problems\"\n"
    ));
    let o = run(&t, "cpu", STAMP, LIST, &[]);
    assert_eq!(code(&o), 0, "{}", stderr(&o));
    assert!(
        stderr(&o).contains(
            "INFO [notify] cpu: the run exited 0 and notify.when is problems; nothing sent"
        ),
        "{}",
        stderr(&o)
    );
    assert!(rx.recv_timeout(Duration::from_millis(300)).is_err());
    let o = run(
        &t,
        "gpu",
        STAMP,
        LIST,
        &[("STUB_EB_FAIL", "Foo-1.0-foss-2025a")],
    );
    assert_eq!(code(&o), 1, "{}", stderr(&o));
    let req = rx.recv_timeout(Duration::from_secs(10)).unwrap();
    let posted: Value = serde_json::from_slice(&req.body).unwrap();
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
