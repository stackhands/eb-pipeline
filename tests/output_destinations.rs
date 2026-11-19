use std::io::Write;
use std::process::Command;

const CHILD: &str = "EB_PIPELINE_FULL_OUTPUT_CHILD";

#[test]
fn init_removes_a_partial_file_when_the_destination_fills() {
    if std::env::var_os(CHILD).is_none() {
        let available = Command::new("unshare")
            .args(["-Ur", "-m", "true"])
            .status()
            .is_ok_and(|status| status.success());
        if !available {
            eprintln!("skipping: user and mount namespaces are unavailable");
            return;
        }
        let status = Command::new("unshare")
            .args(["-Ur", "-m"])
            .arg(std::env::current_exe().unwrap())
            .args([
                "--exact",
                "init_removes_a_partial_file_when_the_destination_fills",
                "--nocapture",
            ])
            .env(CHILD, "1")
            .status()
            .unwrap();
        assert!(status.success(), "hostile-filesystem child failed");
        return;
    }

    let mountpoint = tempfile::tempdir().unwrap();
    let mounted = Command::new("mount")
        .args(["-t", "tmpfs", "-o", "size=1m", "tmpfs"])
        .arg(mountpoint.path())
        .status()
        .is_ok_and(|status| status.success());
    if !mounted {
        eprintln!("skipping: a private tmpfs cannot be mounted");
        return;
    }

    let filler_path = mountpoint.path().join("filler");
    let mut filler = std::fs::File::create(&filler_path).unwrap();
    let block = [0_u8; 4096];
    while filler.write_all(&block).is_ok() {}
    let used = filler.metadata().unwrap().len();
    assert!(used >= 8192, "tmpfs filled before the test was ready");
    filler.set_len(used - 4096).unwrap();
    filler.sync_all().unwrap();
    drop(filler);

    let output_path = mountpoint.path().join("site.toml");
    let output = Command::new(env!("CARGO_BIN_EXE_eb-pipeline"))
        .args(["init", "--template", "eessi-site", "--out"])
        .arg(&output_path)
        .output()
        .unwrap();
    assert_eq!(
        output.status.code(),
        Some(1),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(
        !output_path.exists(),
        "a partial output remained at {}",
        output_path.display()
    );
}
