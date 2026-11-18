//! The site file reference, `docs/orgmode/reference/site-file.org`, against
//! the schema: one table per section of `schema::SECTIONS`, with a row per
//! key in declaration order, and the type and default `eb-pipeline schema`
//! prints.

use eb_pipeline::schema::SECTIONS;
use std::path::PathBuf;
use std::process::Command;

fn page() -> String {
    let p = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("docs/orgmode/reference/site-file.org");
    std::fs::read_to_string(&p).unwrap_or_else(|e| panic!("{}: {e}", p.display()))
}

/// The heading of a section: `[site.tools.TOOL]`, `[[targets]]`,
/// `[targets.phases]` and the like, in verbatim, or `Top level`.
fn heading(path: &str) -> String {
    if path.is_empty() {
        return "Top level".into();
    }
    let p = path.replace("<tool>", "TOOL").replace("<name>", "NAME");
    match p.strip_suffix("[]") {
        Some(table) => format!("=[[{table}]]="),
        None => format!("=[{}]=", p.replace("[]", "")),
    }
}

/// The rows of the first table under `heading`, before the next heading,
/// as cells without the rule lines.
fn table_under<'a>(lines: &[&'a str], heading: &str) -> Vec<Vec<&'a str>> {
    let at = lines
        .iter()
        .position(|l| l.starts_with('*') && l.trim_start_matches('*').trim() == heading)
        .unwrap_or_else(|| panic!("site-file.org has no heading {heading}"));
    lines[at + 1..]
        .iter()
        .take_while(|l| !l.starts_with('*'))
        .skip_while(|l| !l.starts_with('|'))
        .take_while(|l| l.starts_with('|'))
        .filter(|l| !l.starts_with("|-"))
        .map(|l| l.trim_matches('|').split('|').map(str::trim).collect())
        .collect()
}

#[test]
fn site_file_page_has_every_key_with_its_type_and_default() {
    let text = page();
    let lines: Vec<&str> = text.lines().collect();
    for (path, fields) in SECTIONS {
        let h = heading(path);
        let rows = table_under(&lines, &h);
        assert!(!rows.is_empty(), "no table under {h}");
        assert_eq!(rows[0], ["Key", "Type", "Default", "Meaning"], "{h}");
        let keys: Vec<String> = rows[1..].iter().map(|r| r[0].replace('=', "")).collect();
        let want: Vec<&str> = fields.iter().map(|f| f.key).collect();
        assert_eq!(keys, want, "the keys of {h}");
        for (row, f) in rows[1..].iter().zip(fields.iter()) {
            assert_eq!(row[1], f.ty, "the type of {} in {h}", f.key);
            let default = if f.required { "required" } else { f.default };
            assert_eq!(
                row[2].replace('=', ""),
                default,
                "the default of {} in {h}",
                f.key
            );
            assert!(!row[3].is_empty(), "{} in {h} has no meaning", f.key);
        }
    }
}

#[test]
fn run_tests_howto_matches_eb_test_map_cli() {
    let dir = tempfile::tempdir().unwrap();
    let map = dir.path().join("map.tsv");
    let list = dir.path().join("build.list");
    std::fs::write(&map, "GROMACS gromacs\n").unwrap();
    std::fs::write(&list, "Unmapped-1.0.eb\n").unwrap();

    let reframe_unmapped = Command::new(env!("CARGO_BIN_EXE_eb-test-map"))
        .arg("--map")
        .arg(&map)
        .arg("--list")
        .arg(&list)
        .args(["--format", "reframe", "--allow-unmapped"])
        .output()
        .unwrap();
    assert_eq!(reframe_unmapped.status.code(), Some(0));

    let missing_list = Command::new(env!("CARGO_BIN_EXE_eb-test-map"))
        .env_remove("EB_PIPELINE_LIST")
        .env_remove("EB_PIPELINE_RESULT")
        .output()
        .unwrap();
    assert_eq!(missing_list.status.code(), Some(2));
    assert!(
        String::from_utf8_lossy(&missing_list.stderr).contains("--list or --result is required")
    );

    let bad_format = Command::new(env!("CARGO_BIN_EXE_eb-test-map"))
        .arg("--map")
        .arg(&map)
        .arg("--list")
        .arg(&list)
        .args(["--format", "bogus"])
        .output()
        .unwrap();
    assert_eq!(bad_format.status.code(), Some(2));
    assert!(String::from_utf8_lossy(&bad_format.stderr)
        .contains("expected `text`, `reframe`, or `eessi-modules`"));

    let howto_path = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("docs/orgmode/howto/run-tests-for-what-you-built.org");
    let howto = std::fs::read_to_string(&howto_path).unwrap();
    assert!(!howto.contains("applies to =--format text= only"));
    assert!(!howto.contains("The format is not =text= or =eessi-modules="));
    assert!(howto.contains("=--list or --result is required="));
    assert!(howto.contains("`text`, `reframe`, or `eessi-modules`"));
}

#[test]
fn publish_junit_howto_matches_eb_junit_cli() {
    let missing = Command::new(env!("CARGO_BIN_EXE_eb-junit"))
        .env_remove("EB_PIPELINE_RESULT")
        .output()
        .unwrap();
    assert_eq!(missing.status.code(), Some(2));
    let err = String::from_utf8_lossy(&missing.stderr);
    assert!(
        err.contains("eb-junit: --result or --reframe-report is required"),
        "{err}"
    );

    let howto_path = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("docs/orgmode/howto/publish-a-junit-report.org");
    let howto = std::fs::read_to_string(&howto_path).unwrap();
    assert!(!howto.contains("=eb-junit: --result is required="));
    assert!(howto.contains("=eb-junit: --result or --reframe-report is required="));
}

#[test]
fn retry_list_reference_matches_help_on_resumed_skips() {
    let help = Command::new(env!("CARGO_BIN_EXE_eb-retry-list"))
        .arg("--help")
        .output()
        .unwrap();
    assert_eq!(help.status.code(), Some(0));
    let help_text = String::from_utf8_lossy(&help.stdout);
    assert!(help_text.contains("resumed:"));
    assert!(help_text.contains("--status skipped"));

    let cli_path = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("docs/orgmode/reference/cli.org");
    let cli = std::fs::read_to_string(&cli_path).unwrap();
    let section = cli
        .split("*** eb-retry-list")
        .nth(1)
        .and_then(|rest| rest.split("*** ").next())
        .expect("cli.org has an eb-retry-list section");
    assert!(section.contains("resumed:"));
    assert!(section.contains("=--status skipped="));
}
