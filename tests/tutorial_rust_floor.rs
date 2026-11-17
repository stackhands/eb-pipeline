//! `docs/examples/tutorial-rust-floor.sh` fails until the install pages name
//! `rustup toolchain install 1.90` and `cargo +1.90 install`.

use std::process::Command;

#[test]
fn tutorial_and_install_pages_name_rust_190_toolchain() {
    let script = concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/docs/examples/tutorial-rust-floor.sh"
    );
    let o = Command::new("bash").arg(script).output().expect("bash");
    assert!(o.status.success(), "{}", String::from_utf8_lossy(&o.stderr));
}
