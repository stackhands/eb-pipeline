//! Opt-in resume from a prior result.
//!
//! The input identity is the file `eb-stack stack order --hashes-out` writes.
//! This module starts that command and reads its report; it does not order
//! recipes or hash easyconfigs itself. Restart boundaries come from
//! `eb-stack stack shard` the same way.

use crate::exit::{self, Fail};
use crate::schema::Site;
use crate::util::{package_of, read_text_file, ListLine};
use sha2::{Digest, Sha256};
use std::collections::{BTreeSet, HashSet};
use std::path::{Path, PathBuf};

/// One line of a prior result, as far as a resume needs it.
#[derive(Debug, Clone)]
pub struct PriorLine {
    /// The line's easyconfig.
    pub easyconfig: String,
    /// `success`, `failed`, `refused`, `pending` or `skipped`.
    pub status: String,
    /// Module files the prior run declared for a successful line.
    pub outputs: Vec<String>,
}

/// A prior result a resume compares and may skip from.
#[derive(Debug, Clone)]
pub struct Prior {
    /// Hex SHA-256 of the `stack order --hashes-out` file.
    pub digest: String,
    /// Every line the prior run recorded, in order.
    pub lines: Vec<PriorLine>,
}

/// Easyconfig trees for `stack order` and a resume boundary `stack shard`.
///
/// `resume.easyconfigs` wins when it is not empty. Otherwise the trees are
/// `shards.easyconfigs`.
pub fn easyconfig_trees(site: &Site) -> Vec<String> {
    if let Some(r) = &site.resume {
        if r.enabled && !r.easyconfigs.is_empty() {
            return r.easyconfigs.clone();
        }
    }
    site.shards
        .as_ref()
        .map(|s| s.easyconfigs.clone())
        .unwrap_or_default()
}

/// `--root` values for `stack order`, one per distinct package in list order.
pub fn roots(lines: &[ListLine]) -> Vec<String> {
    let mut seen = BTreeSet::new();
    let mut out = Vec::new();
    for line in lines {
        let (name, version) = package_of(&line.easyconfig);
        if name.is_empty() {
            continue;
        }
        let root = match version {
            Some(v) => format!("{name}=={v}"),
            None => name,
        };
        if seen.insert(root.clone()) {
            out.push(root);
        }
    }
    out
}

/// Where `run` asks `stack order` to write the hash file.
pub fn hashes_path(log_dir: &str) -> PathBuf {
    Path::new(log_dir).join("input-hashes.txt")
}

/// Where `run` asks `stack order` to write the build order.
pub fn order_path(log_dir: &str) -> PathBuf {
    Path::new(log_dir).join("input-order.txt")
}

/// A single path component for a repository path, so it can name a directory.
pub fn workspace_key(repo: &str) -> String {
    if !repo.contains(['_', '%']) {
        let key: String = repo
            .chars()
            .map(|c| {
                if c.is_ascii_alphanumeric() || c == '.' || c == '-' {
                    c
                } else {
                    '_'
                }
            })
            .collect();
        return if key.is_empty() || key.chars().all(|c| c == '_' || c == '.') {
            "repository".into()
        } else {
            key
        };
    }

    const HEX: &[u8; 16] = b"0123456789ABCDEF";
    let mut key = String::with_capacity(repo.len());
    for byte in repo.bytes() {
        if byte.is_ascii_alphanumeric() || byte == b'-' || byte == b'.' {
            key.push(char::from(byte));
        } else {
            key.push('%');
            key.push(char::from(HEX[usize::from(byte >> 4)]));
            key.push(char::from(HEX[usize::from(byte & 0x0f)]));
        }
    }
    key
}

/// Read a result file written by a run that recorded an input digest.
pub fn load(path: &Path) -> Result<Prior, Fail> {
    let text = read_text_file(path)
        .map_err(|e| Fail::usage(format!("cannot read {}: {e}", path.display())))?;
    let v: serde_json::Value =
        serde_json::from_str(&text).map_err(|e| Fail::usage(format!("{}: {e}", path.display())))?;
    if v.get("kind").and_then(|k| k.as_str()) != Some("result") {
        return Err(Fail::usage(format!(
            "{} is not an eb-pipeline result",
            path.display()
        )));
    }
    let Some(digest) = v
        .get("input_digest")
        .and_then(|d| d.as_str())
        .filter(|d| !d.is_empty())
    else {
        return Err(Fail::new(
            exit::REFUSED,
            format!(
                "{} has no input_digest; the prior run did not record one",
                path.display()
            ),
        ));
    };
    let Some(lines) = v.get("lines").and_then(|l| l.as_array()) else {
        return Err(Fail::usage(format!(
            "{} has no lines array",
            path.display()
        )));
    };
    let mut out = Vec::new();
    for (i, line) in lines.iter().enumerate() {
        let Some(easyconfig) = line.get("easyconfig").and_then(|s| s.as_str()) else {
            return Err(Fail::usage(format!(
                "{}: lines[{i}].easyconfig must be a string",
                path.display()
            )));
        };
        let status = line
            .get("status")
            .and_then(|s| s.as_str())
            .unwrap_or("")
            .to_string();
        let outputs = line
            .get("outputs")
            .and_then(|o| o.as_array())
            .map(|a| {
                a.iter()
                    .filter_map(|v| v.as_str().map(str::to_string))
                    .collect()
            })
            .unwrap_or_default();
        out.push(PriorLine {
            easyconfig: easyconfig.to_string(),
            status,
            outputs,
        });
    }
    Ok(Prior {
        digest: digest.to_string(),
        lines: out,
    })
}

/// The digest of a `stack order --hashes-out` run, from its log and the file.
///
/// `incomplete` other than 0 is a refusal: the file is not a full identity.
pub fn digest_from_log(log: &Path, hashes: &Path) -> Result<String, Fail> {
    let text = std::fs::read_to_string(log)
        .map_err(|e| Fail::new(exit::FAILED, format!("cannot read {}: {e}", log.display())))?;
    let incomplete = incomplete_of(&text)?;
    if incomplete != 0 {
        return Err(Fail::new(
            exit::REFUSED,
            format!(
                "eb-stack stack order input identity is incomplete ({incomplete} modules); see {}",
                log.display()
            ),
        ));
    }
    let bytes = std::fs::read(hashes).map_err(|e| {
        Fail::new(
            exit::FAILED,
            format!("cannot read {}: {e}", hashes.display()),
        )
    })?;
    if bytes.is_empty() {
        return Err(Fail::new(
            exit::REFUSED,
            format!(
                "eb-stack stack order wrote an empty hash file {}",
                hashes.display()
            ),
        ));
    }
    Ok(sha256_hex(&bytes))
}

/// Declared outputs of a successful prior line with this easyconfig.
pub fn outputs_for(prior: &Prior, easyconfig: &str) -> Vec<String> {
    prior
        .lines
        .iter()
        .find(|l| l.easyconfig == easyconfig && l.status == "success")
        .map(|l| l.outputs.clone())
        .unwrap_or_default()
}

/// Units whose every line succeeded in `prior` and whose declared outputs
/// still exist.
///
/// `cpu-prerequisites`, `prelude` and `all` are prefixes: a prefix that cannot
/// be skipped blocks every unit after it. Shards do not block one another.
pub fn units_to_skip<'a>(
    units: impl IntoIterator<Item = (&'a str, &'a [ListLine])>,
    prior: &Prior,
) -> BTreeSet<String> {
    let mut used = vec![false; prior.lines.len()];
    let mut blocked = false;
    let mut skip = BTreeSet::new();
    for (name, lines) in units {
        let claimed = if blocked || lines.is_empty() {
            None
        } else {
            claim(lines, prior, &used)
        };
        match claimed {
            Some(idx) => {
                for i in idx {
                    used[i] = true;
                }
                skip.insert(name.to_string());
            }
            None if is_prefix(name) => blocked = true,
            None => {}
        }
    }
    skip
}

fn is_prefix(name: &str) -> bool {
    matches!(name, "all" | "cpu-prerequisites" | "prelude")
}

fn outputs_exist(paths: &[String]) -> bool {
    !paths.is_empty() && paths.iter().all(|p| Path::new(p).is_file())
}

/// Indexes of prior lines that cover `lines`, or none. Does not update `used`.
fn claim(lines: &[ListLine], prior: &Prior, used: &[bool]) -> Option<Vec<usize>> {
    let mut local: HashSet<usize> = used
        .iter()
        .enumerate()
        .filter_map(|(i, u)| u.then_some(i))
        .collect();
    let mut idx = Vec::new();
    for line in lines {
        let found = prior.lines.iter().enumerate().position(|(i, p)| {
            !local.contains(&i)
                && p.easyconfig == line.easyconfig
                && p.status == "success"
                && outputs_exist(&p.outputs)
        })?;
        local.insert(found);
        idx.push(found);
    }
    Some(idx)
}

fn incomplete_of(log: &str) -> Result<usize, Fail> {
    let mut found = None;
    for line in log.lines() {
        let Some(rest) = line.trim().strip_prefix("hashes=") else {
            continue;
        };
        let Some(raw) = rest
            .split_whitespace()
            .find_map(|w| w.strip_prefix("incomplete="))
        else {
            continue;
        };
        let n: usize = raw.parse().map_err(|_| {
            Fail::new(
                exit::FAILED,
                format!("eb-stack stack order printed an invalid incomplete count `{raw}`"),
            )
        })?;
        if found.is_some() {
            return Err(Fail::new(
                exit::FAILED,
                "eb-stack stack order printed more than one `incomplete=` count",
            ));
        }
        found = Some(n);
    }
    found.ok_or_else(|| {
        Fail::new(
            exit::FAILED,
            "eb-stack stack order printed no `incomplete=` count",
        )
    })
}

fn sha256_hex(bytes: &[u8]) -> String {
    let dig = Sha256::digest(bytes);
    let mut s = String::with_capacity(dig.len() * 2);
    for b in dig {
        s.push_str(&format!("{b:02x}"));
    }
    s
}

#[cfg(test)]
#[allow(clippy::disallowed_methods)]
mod tests {
    use super::*;
    use std::ffi::CString;
    use std::os::unix::ffi::OsStrExt;

    #[test]
    fn resume_result_reader_refuses_a_fifo_promptly() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("result.json");
        let raw = CString::new(path.as_os_str().as_bytes()).unwrap();
        assert_eq!(unsafe { libc::mkfifo(raw.as_ptr(), 0o600) }, 0);
        let (send, receive) = std::sync::mpsc::channel();
        let reader = std::thread::spawn(move || {
            send.send(load(&path)).unwrap();
        });
        let error = receive
            .recv_timeout(std::time::Duration::from_secs(5))
            .expect("resume reader blocked")
            .unwrap_err();
        assert!(error.msg.contains("not a regular file"), "{error:?}");
        reader.join().unwrap();
    }

    fn line(easyconfig: &str) -> ListLine {
        ListLine {
            raw: format!("{easyconfig}\n"),
            easyconfig: easyconfig.into(),
            args: vec![],
        }
    }

    #[test]
    fn sha256_of_abc_matches_the_standard_vector() {
        assert_eq!(
            sha256_hex(b"abc"),
            "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad"
        );
    }

    #[test]
    fn a_repository_path_becomes_one_directory_name() {
        assert_eq!(
            workspace_key("/opt/site/easyconfigs"),
            "_opt_site_easyconfigs"
        );
        assert_eq!(workspace_key(".."), "repository");
        assert_eq!(workspace_key("/opt/a_b"), "%2Fopt%2Fa%5Fb");
        assert_eq!(workspace_key("/opt/a%b"), "%2Fopt%2Fa%25b");
        assert_eq!(workspace_key("/opt/a/b"), "_opt_a_b");
        assert_ne!(workspace_key("/opt/a_b"), workspace_key("/opt/a/b"));
    }

    #[test]
    fn roots_keep_list_order_and_drop_repeats() {
        let lines = [
            line("Foo-1.0-foss-2025a.eb"),
            line("Foo-1.0-foss-2025a.eb"),
            line("Bar-2.0-foss-2025a.eb"),
        ];
        assert_eq!(roots(&lines), ["Foo==1.0", "Bar==2.0"]);
    }

    #[test]
    fn a_failed_prefix_blocks_later_shards_whose_outputs_remain() {
        let dir = tempfile::tempdir().unwrap();
        let output = dir.path().join("mod.lua");
        std::fs::write(&output, "module\n").unwrap();
        let kept = output.display().to_string();
        let prior = Prior {
            digest: "aa".into(),
            lines: vec![
                PriorLine {
                    easyconfig: "Pre-1.0.eb".into(),
                    status: "failed".into(),
                    outputs: vec![],
                },
                PriorLine {
                    easyconfig: "Foo-1.0.eb".into(),
                    status: "success".into(),
                    outputs: vec![kept],
                },
            ],
        };
        let pre = [line("Pre-1.0.eb")];
        let shard = [line("Foo-1.0.eb")];
        let skip = units_to_skip(
            [("prelude", pre.as_slice()), ("shard-01", shard.as_slice())],
            &prior,
        );
        assert!(skip.is_empty(), "{skip:?}");

        let foo = dir.path().join("foo.lua");
        std::fs::write(&foo, "module\n").unwrap();
        let prior = Prior {
            digest: "aa".into(),
            lines: vec![
                PriorLine {
                    easyconfig: "Foo-1.0.eb".into(),
                    status: "success".into(),
                    outputs: vec![foo.display().to_string()],
                },
                PriorLine {
                    easyconfig: "Bar-2.0.eb".into(),
                    status: "failed".into(),
                    outputs: vec![],
                },
            ],
        };
        let a = [line("Foo-1.0.eb")];
        let b = [line("Bar-2.0.eb")];
        let skip = units_to_skip(
            [("shard-01", a.as_slice()), ("shard-02", b.as_slice())],
            &prior,
        );
        assert_eq!(skip.into_iter().collect::<Vec<_>>(), ["shard-01"]);

        let prior = Prior {
            digest: "aa".into(),
            lines: vec![PriorLine {
                easyconfig: "Foo-1.0.eb".into(),
                status: "success".into(),
                outputs: vec!["/no/such/module.lua".into()],
            }],
        };
        let lines = [line("Foo-1.0.eb")];
        let skip = units_to_skip([("all", lines.as_slice())], &prior);
        assert!(skip.is_empty());
    }
}
