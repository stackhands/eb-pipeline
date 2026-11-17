//! After a clean build, `eb-stack stack sbom --lock` writes CycloneDX beside the summary.

mod common;
use common::{code, run, setup, stderr, STAMP};

const LIST: &str = "Foo-1.0-foss-2025a.eb\n";

#[test]
fn a_clean_build_stores_the_cyclonedx_file_beside_the_summary() {
    let t = setup("[sbom]\nlock = \"@ROOT@/stack.lock.json\"\n");
    std::fs::write(t.root().join("stack.lock.json"), "{}\n").unwrap();
    let o = run(&t, "cpu", STAMP, LIST, &[]);
    assert_eq!(code(&o), 0, "{}", stderr(&o));
    let doc = t.run_dir(STAMP).join("sbom.cdx.json");
    let text = std::fs::read_to_string(&doc).unwrap_or_else(|e| panic!("{e}: {}", stderr(&o)));
    assert!(text.contains("CycloneDX"), "{text}");
    let calls = t.calls("ebstack");
    assert!(calls.contains("stack sbom"), "{calls}");
    assert!(calls.contains("--lock"), "{calls}");
    assert!(
        calls.contains(doc.to_str().unwrap()),
        "{calls} does not name {}",
        doc.display()
    );
    let summary = t.summary(STAMP);
    assert_eq!(summary["exit_code"], 0);
}

#[test]
fn a_relative_lock_is_refused_before_any_build() {
    let t = setup("[sbom]\nlock = \"stack.lock.json\"\n");
    let o = run(&t, "cpu", STAMP, LIST, &[]);
    assert_eq!(code(&o), 2, "{}", stderr(&o));
    assert!(
        stderr(&o).contains("not an absolute path"),
        "{}",
        stderr(&o)
    );
    assert!(!t.calls("ebstack").contains("stack sbom"));
}

#[test]
fn a_failed_build_does_not_ask_for_an_sbom() {
    let t = setup("[sbom]\nlock = \"@ROOT@/stack.lock.json\"\n");
    let before = t.calls("ebstack");
    let o = run(
        &t,
        "cpu",
        STAMP,
        LIST,
        &[("STUB_EB_FAIL", "Foo-1.0-foss-2025a")],
    );
    assert_eq!(code(&o), 1, "{}", stderr(&o));
    assert_eq!(t.calls("ebstack"), before);
    assert!(!t.run_dir(STAMP).join("sbom.cdx.json").exists());
}

#[test]
fn an_sbom_command_that_fails_fails_the_run_and_leaves_no_document() {
    let t = setup("[sbom]\nlock = \"@ROOT@/stack.lock.json\"\n");
    let o = run(&t, "cpu", STAMP, LIST, &[("STUB_SBOM_FAIL", "1")]);
    assert_eq!(code(&o), 1, "{}", stderr(&o));
    assert!(stderr(&o).contains("stack sbom"), "{}", stderr(&o));
    assert!(!t.run_dir(STAMP).join("sbom.cdx.json").exists());
    assert_eq!(t.summary(STAMP)["exit_code"], 1);
}
