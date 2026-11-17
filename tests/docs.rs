//! The site file reference, `docs/orgmode/reference/site-file.org`, against
//! the schema: one table per section of `schema::SECTIONS`, with a row per
//! key in declaration order, and the type and default `eb-pipeline schema`
//! prints.

use eb_pipeline::schema::SECTIONS;
use std::path::PathBuf;

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
