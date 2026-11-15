//! Exit-code contract.

/// All work succeeded.
pub const OK: i32 = 0;
/// Work was attempted and failed.
pub const FAILED: i32 = 1;
/// Usage error or invalid site file.
pub const USAGE: i32 = 2;
/// The planner declined to shard; the plan was built unsharded.
pub const UNSHARDED: i32 = 3;
/// A resource is temporarily unavailable; rerun.
pub const TEMPFAIL: i32 = 75;
/// Refused by a precondition; no build step started.
pub const REFUSED: i32 = 78;
/// Interrupted by SIGINT.
pub const SIGINT: i32 = 130;
/// Interrupted by SIGTERM.
pub const SIGTERM: i32 = 143;

/// Table printed in `--help` and the README.
pub const TABLE: &str = "\
Exit codes:
   0  all requested work completed
   1  work attempted and failed (a build line or a phase); see the result file
   2  usage error, invalid site file, or a tool below its minimum version
   3  the planner declined to shard; the plan was built unsharded (informational)
  75  a resource is temporarily unavailable or a step is pending; rerun
  78  refused by a precondition (admission, configuration); no build step started
 130  interrupted by SIGINT, after the running child ended
 143  interrupted by SIGTERM, after the running child ended

Sibling tool codes are mapped explicitly:
  eb-stack admit    0 admitted, 78 refused, 1 failed, 2 usage
  eb-stack shard    the code in shards.planner_exit_unsharded (3) means unsharded
  eb-staged-rebuild 2 is a refusal there; the driver records it as refused
                    (exit 78), 75 as pending (exit 75), any other non-zero as failed
Precedence when several apply: 1, then 78, then 75, then 0. Codes are never summed.";

/// Error carrying the process exit code.
#[derive(Debug)]
pub struct Fail {
    pub code: i32,
    pub msg: String,
}

impl Fail {
    pub fn new(code: i32, msg: impl Into<String>) -> Self {
        Fail {
            code,
            msg: msg.into(),
        }
    }
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
