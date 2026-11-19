//! Exit-code contract.

use std::path::Path;

/// All work succeeded.
pub const OK: i32 = 0;
/// Work was attempted and failed.
pub const FAILED: i32 = 1;
/// Usage error or invalid site file.
pub const USAGE: i32 = 2;
/// A resource is temporarily unavailable; rerun.
pub const TEMPFAIL: i32 = 75;
/// Refused by a precondition, by admission or by eb-staged-rebuild. Lines
/// built before the refusal stay built.
pub const REFUSED: i32 = 78;
/// Interrupted by SIGINT.
pub const SIGINT: i32 = 130;
/// Interrupted by SIGTERM.
pub const SIGTERM: i32 = 143;

/// Table printed in `--help`.
pub const TABLE: &str = "\
Exit codes:
   0  all requested work completed
   1  work attempted and failed (a build line or a phase); see the result file
   2  usage error, invalid site file, or a tool below its minimum version
  75  a resource is temporarily unavailable or a step is pending; rerun
  78  refused by a precondition, by admission or by eb-staged-rebuild; lines
      built before the refusal stay built, see the result file
 130  interrupted by SIGINT, after the running child ended
 143  interrupted by SIGTERM, after the running child ended

Sibling tool codes are mapped explicitly:
  eb-stack stack admit
                    0 admitted, 78 refused, 1 failed, 2 usage
  eb-stack stack shard
                    the code in shards.planner_exit_unsharded (default 3) means
                    unsharded; the run continues
  eb-staged-rebuild 2 is a refusal there; the driver records it as refused
                    (exit 78), 75 as pending (exit 75), any other non-zero as failed
  eb-keep-logs      rotate 75 means another rotate holds the log root's lock;
                    the logs phase is recorded as skipped, any other non-zero
                    as failed
A signal (130, 143) or a usage error (2) ends the run with that code. Otherwise,
when several apply: 1, then 78, then 75, then 0. Codes are never summed.";

/// Error carrying the process exit code.
#[derive(Debug)]
pub struct Fail {
    /// The exit code the program ends with, one of the constants above.
    pub code: i32,
    /// What went wrong; the program prints it as `ERROR [main] -: <msg>`.
    pub msg: String,
}

impl Fail {
    /// A failure that ends the program with `code`.
    pub fn new(code: i32, msg: impl Into<String>) -> Self {
        Fail {
            code,
            msg: msg.into(),
        }
    }
    /// A failure with [`USAGE`], 2: a wrong command line, an invalid site
    /// file or input, or a tool below its `min_version`.
    pub fn usage(msg: impl Into<String>) -> Self {
        Fail::new(USAGE, msg)
    }
}

impl std::fmt::Display for Fail {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.msg)
    }
}

impl std::error::Error for Fail {}

/// Parse JSON output written by a subprocess.
pub fn parse_json_output(
    bytes: &[u8],
    path: &Path,
    producer: &str,
    error_code: i32,
) -> Result<serde_json::Value, Fail> {
    serde_json::from_slice(bytes).map_err(|error| {
        Fail::new(
            error_code,
            format!(
                "{producer} wrote invalid JSON to {}: {error}",
                path.display()
            ),
        )
    })
}

/// Read and parse a JSON file written by a subprocess.
pub fn read_json_output(
    path: &Path,
    producer: &str,
    error_code: i32,
) -> Result<(Vec<u8>, serde_json::Value), Fail> {
    let bytes = std::fs::read(path).map_err(|error| {
        Fail::new(
            error_code,
            format!("cannot read {}: {error}", path.display()),
        )
    })?;
    let value = parse_json_output(&bytes, path, producer, error_code)?;
    Ok((bytes, value))
}
