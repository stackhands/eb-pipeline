//! Opt-in attestation of the exact inputs of a finalized plan.
//!
//! With `[attest]` enabled, `plan` and a `run` that is not a dry run hash the
//! build list, every easyconfig `eb-stack stack order` resolves, the easyblock
//! and the patches those files name, the site file, the container image digest
//! and the route receipt. The canonical record is appended to an RFC 9162
//! sidecar log by running `rfc9162-sidecar`. `verify` recomputes that record
//! and checks the inclusion proof. Absent or disabled, a plan and a result
//! omit the attestation and are unchanged.

use crate::exit::{self, Fail};
use crate::resume;
use crate::schema::{Attest, Site};
use crate::tools::Tool;
use crate::util::{self, package_of, ListLine};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use std::collections::BTreeMap;
use std::fs;
use std::os::unix::process::ExitStatusExt;
use std::path::{Path, PathBuf};
use std::process::{Command, ExitStatus, Stdio};

/// The `kind` of a canonical input record.
pub const KIND: &str = "eb-pipeline-input-attestation-v1";

/// The checkpoint and inclusion proof stored on a plan and a result.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct InputAttestation {
    /// Lower-case SHA-256 of the canonical input record, including its
    /// trailing newline.
    pub record_sha256: String,
    /// The signed checkpoint and inclusion proof `rfc9162-sidecar prove` wrote.
    pub proof: Value,
}

/// What `plan`, `run` and `verify` hash.
pub struct Request<'a> {
    /// The loaded site.
    pub site: &'a Site,
    /// The enabled `[attest]` table.
    pub attest: &'a Attest,
    /// The site file, as given on the command line.
    pub site_path: &'a Path,
    /// The build list, as given on the command line.
    pub list_path: &'a Path,
    /// The lines of that list.
    pub lines: &'a [ListLine],
    /// The target the plan is for.
    pub target: &'a str,
}

/// Easyconfig trees for `stack order`.
///
/// `attest.easyconfigs` wins when it is not empty. Otherwise the trees are
/// those `[resume]` uses, which themselves fall back to `shards.easyconfigs`.
pub fn easyconfig_trees(site: &Site) -> Vec<String> {
    if let Some(a) = &site.attest {
        if a.enabled && !a.easyconfigs.is_empty() {
            return a.easyconfigs.clone();
        }
    }
    resume::easyconfig_trees(site)
}

/// Append the canonical record to the sidecar log and return its proof.
pub fn append(req: &Request<'_>) -> Result<InputAttestation, Fail> {
    let (sha, bytes) = canonical(req)?;
    let rel = format!("inputs/{sha}.json");
    let log_dir = PathBuf::from(req.attest.log_dir.as_deref().unwrap_or(""));
    let dest = log_dir.join(&rel);
    if let Some(parent) = dest.parent() {
        fs::create_dir_all(parent).map_err(|e| {
            Fail::new(
                exit::FAILED,
                format!("cannot create {}: {e}", parent.display()),
            )
        })?;
    }
    if dest.is_file() {
        let existing = fs::read(&dest)
            .map_err(|e| Fail::new(exit::FAILED, format!("cannot read {}: {e}", dest.display())))?;
        if existing != bytes {
            return Err(Fail::new(
                exit::REFUSED,
                format!(
                    "{} exists and is not the record just hashed",
                    dest.display()
                ),
            ));
        }
    } else {
        util::write_atomic(&dest, &bytes).map_err(|e| {
            Fail::new(
                exit::FAILED,
                format!("cannot write {}: {e}", dest.display()),
            )
        })?;
    }
    let dir = log_dir.display().to_string();
    let bin = req.attest.bin.clone();
    let key = req.attest.key.clone().unwrap_or_default();
    if !Path::new(&key).is_file() {
        return Err(Fail::new(
            exit::REFUSED,
            format!("[attest] key `{key}` is not a file"),
        ));
    }
    let origin = req.attest.origin.clone().unwrap_or_default();
    run(&argv(&bin, &["init", "--dir", &dir]), exit::FAILED)?;
    run(&argv(&bin, &["sync", "--dir", &dir]), exit::FAILED)?;
    run(
        &argv(
            &bin,
            &[
                "snapshot", "--dir", &dir, "--key", &key, "--origin", &origin,
            ],
        ),
        exit::FAILED,
    )?;
    let proof_text = run(
        &argv(&bin, &["prove", "--dir", &dir, "--path", &rel]),
        exit::FAILED,
    )?;
    let proof: Value = serde_json::from_str(&proof_text).map_err(|e| {
        Fail::new(
            exit::FAILED,
            format!("rfc9162-sidecar prove wrote no JSON object: {e}"),
        )
    })?;
    if proof.get("blob").and_then(Value::as_str) != Some(sha.as_str()) {
        return Err(Fail::new(
            exit::FAILED,
            format!("rfc9162-sidecar prove blob is not the record {sha}"),
        ));
    }
    if proof.get("path").and_then(Value::as_str) != Some(rel.as_str()) {
        return Err(Fail::new(
            exit::FAILED,
            format!("rfc9162-sidecar prove path is not {rel}"),
        ));
    }
    verify_inclusion_proof(&proof)?;
    Ok(InputAttestation {
        record_sha256: sha,
        proof,
    })
}

/// Recompute the record and check `stored` against the sidecar proof.
///
/// A record that no longer matches the inputs is [`exit::REFUSED`]. A proof
/// the sidecar rejects is [`exit::FAILED`].
pub fn verify(req: &Request<'_>, stored: &InputAttestation) -> Result<(), Fail> {
    let (sha, bytes) = canonical(req)?;
    if sha != stored.record_sha256 {
        return Err(Fail::new(
            exit::REFUSED,
            format!(
                "input record {sha} does not match attestation {}",
                stored.record_sha256
            ),
        ));
    }
    let blob = stored
        .proof
        .get("blob")
        .and_then(Value::as_str)
        .unwrap_or("");
    if blob != sha {
        return Err(Fail::new(
            exit::REFUSED,
            format!("proof blob {blob} does not match input record {sha}"),
        ));
    }
    verify_inclusion_proof(&stored.proof)?;
    let scratch = std::env::temp_dir().join(format!(
        "eb-pipeline-proof-{}-{}",
        std::process::id(),
        nanos()
    ));
    fs::create_dir_all(&scratch).map_err(|e| {
        Fail::new(
            exit::FAILED,
            format!("cannot create {}: {e}", scratch.display()),
        )
    })?;
    let _remove = RemoveDir(scratch.clone());
    let proof_path = scratch.join("proof.json");
    let blob_path = scratch.join("record.json");
    let proof_text = serde_json::to_string_pretty(&stored.proof)
        .map_err(|e| Fail::new(exit::FAILED, format!("cannot write the stored proof: {e}")))?
        + "\n";
    crate::util::write_atomic(&proof_path, proof_text.as_bytes()).map_err(|e| {
        Fail::new(
            exit::FAILED,
            format!("cannot write {}: {e}", proof_path.display()),
        )
    })?;
    crate::util::write_atomic(&blob_path, &bytes).map_err(|e| {
        Fail::new(
            exit::FAILED,
            format!("cannot write {}: {e}", blob_path.display()),
        )
    })?;
    let mut args = vec![
        req.attest.bin.clone(),
        "verify-proof".into(),
        "--proof".into(),
        proof_path.display().to_string(),
        "--blob-file".into(),
        blob_path.display().to_string(),
    ];
    for line in &req.attest.trusted {
        args.push("--trusted".into());
        args.push(line.clone());
    }
    if let Err(f) = run(&args, exit::FAILED) {
        return Err(Fail::new(exit::FAILED, f.msg));
    }
    Ok(())
}

/// Check the RFC 9162 path in a sidecar proof with the published primitive.
fn verify_inclusion_proof(proof: &Value) -> Result<(), Fail> {
    let field = |name: &str| {
        proof
            .get(name)
            .and_then(Value::as_str)
            .ok_or_else(|| Fail::new(exit::FAILED, format!("proof {name} is not a string")))
    };
    let number = |name: &str| {
        let value = proof
            .get(name)
            .and_then(Value::as_u64)
            .ok_or_else(|| Fail::new(exit::FAILED, format!("proof {name} is not a number")))?;
        usize::try_from(value)
            .map_err(|_| Fail::new(exit::FAILED, format!("proof {name} does not fit usize")))
    };
    let entry = field("entry")?;
    let root = field("root")?;
    let index = number("index")?;
    let size = number("size")?;
    let inclusion = proof
        .get("inclusion")
        .and_then(Value::as_array)
        .ok_or_else(|| Fail::new(exit::FAILED, "proof inclusion is not an array"))?;
    let path = inclusion
        .iter()
        .map(|hash| {
            let text = hash
                .as_str()
                .ok_or_else(|| Fail::new(exit::FAILED, "proof inclusion hash is not a string"))?;
            rfc9162::from_hex(text).ok_or_else(|| {
                Fail::new(
                    exit::FAILED,
                    format!("proof inclusion hash {text:?} is not 32 bytes"),
                )
            })
        })
        .collect::<Result<Vec<_>, _>>()?;
    if !rfc9162::verify_inclusion(
        &rfc9162::leaf_hash(entry.as_bytes()),
        index,
        size,
        &path,
        root,
    ) {
        return Err(Fail::new(
            exit::FAILED,
            "RFC 9162 inclusion proof does not meet its checkpoint root",
        ));
    }
    Ok(())
}

/// The canonical record and the SHA-256 of the bytes written into the log.
fn canonical(req: &Request<'_>) -> Result<(String, Vec<u8>), Fail> {
    let site_path = absolute(req.site_path)?;
    let list_path = absolute(req.list_path)?;
    let list_bytes = read_bytes(&list_path)?;
    let list_kind = if crate::easystack::is_easystack(&list_path) {
        "easystack"
    } else {
        "list"
    };
    let easyconfigs = resolved_easyconfigs(req)?;
    let mut easyconfig_rows = Vec::new();
    let mut classes: BTreeMap<String, Option<ResolvedFile>> = BTreeMap::new();
    let mut patch_names: BTreeMap<String, ()> = BTreeMap::new();
    let mut easyconfig_dirs = Vec::new();
    for path in &easyconfigs {
        let bytes = read_bytes(path)?;
        let text = std::str::from_utf8(&bytes)
            .map_err(|_| Fail::new(exit::REFUSED, format!("{} is not UTF-8", path.display())))?;
        easyconfig_rows.push(json!({
            "path": path.display().to_string(),
            "sha256": sha256_hex(&bytes),
        }));
        if let Some(dir) = path.parent() {
            easyconfig_dirs.push(dir.to_path_buf());
        }
        let class = match easyblock_literal(text) {
            Some(class) => class,
            None => recipe_easyblock(req, path)?,
        };
        classes.entry(class).or_insert(None);
        for name in patch_literals(text)
            .map_err(|error| Fail::new(exit::REFUSED, format!("{}: {error}", path.display())))?
        {
            patch_names.insert(name, ());
        }
    }
    easyconfig_rows.sort_by(|a, b| a["path"].as_str().cmp(&b["path"].as_str()));
    easyconfig_rows.dedup_by(|a, b| a["path"] == b["path"]);

    let mut search_roots = Vec::new();
    for dir in &easyconfig_dirs {
        search_roots.push(dir.clone());
    }
    for tree in easyconfig_trees(req.site) {
        search_roots.push(PathBuf::from(tree));
    }
    for extra in &req.attest.patches {
        search_roots.push(PathBuf::from(extra));
    }
    let patches_index = index_files(&search_roots, ".patch")?;
    let mut patch_rows = Vec::new();
    for name in patch_names.keys() {
        let resolved = resolve_patch(name, &easyconfig_dirs, &patches_index)?;
        patch_rows.push(json!({
            "name": name,
            "path": resolved.path.display().to_string(),
            "sha256": resolved.sha256,
        }));
    }

    let block_index = index_files(
        &req.attest
            .easyblocks
            .iter()
            .map(PathBuf::from)
            .collect::<Vec<_>>(),
        ".py",
    )?;
    let mut easyblock_rows = Vec::new();
    for (class, slot) in &mut classes {
        *slot = find_easyblock(class, &block_index)?;
        let found = slot.as_ref();
        easyblock_rows.push(json!({
            "class": class,
            "path": found.map(|f| f.path.display().to_string()),
            "sha256": found.map(|f| f.sha256.clone()),
        }));
    }

    let image = match req.attest.image_digest_env.as_deref() {
        Some(name) => match std::env::var(name) {
            Ok(v) if !v.is_empty() => Value::String(v),
            _ => {
                return Err(Fail::new(
                    exit::REFUSED,
                    format!("[attest] image_digest_env {name} is unset or empty"),
                ))
            }
        },
        None => Value::Null,
    };
    let receipt = route_receipt(req);
    let record = json!({
        "easyblocks": easyblock_rows,
        "easyconfigs": easyconfig_rows,
        "image_digest": image,
        "kind": KIND,
        "list": {
            "kind": list_kind,
            "path": list_path.display().to_string(),
            "sha256": sha256_hex(&list_bytes),
        },
        "patches": patch_rows,
        "route_receipt": receipt,
        "site": {
            "path": site_path.display().to_string(),
            "sha256": sha256_hex(&read_bytes(&site_path)?),
        },
    });
    let mut bytes = serde_json::to_vec_pretty(&record)
        .map_err(|e| Fail::new(exit::FAILED, format!("cannot encode the input record: {e}")))?;
    bytes.push(b'\n');
    let sha = sha256_hex(&bytes);
    Ok((sha, bytes))
}

struct ResolvedFile {
    path: PathBuf,
    sha256: String,
}

fn resolved_easyconfigs(req: &Request<'_>) -> Result<Vec<PathBuf>, Fail> {
    let trees = easyconfig_trees(req.site);
    if trees.is_empty() {
        return Err(Fail::new(
            exit::REFUSED,
            "[attest] needs easyconfigs, on attest.easyconfigs, resume.easyconfigs or shards.easyconfigs",
        ));
    }
    let roots = resume::roots(req.lines);
    if roots.is_empty() {
        return Err(Fail::new(
            exit::REFUSED,
            "[attest] the build list names no package for stack order --root",
        ));
    }
    let order = std::env::temp_dir().join(format!(
        "eb-pipeline-order-{}-{}.txt",
        std::process::id(),
        nanos()
    ));
    let _remove = RemoveFile(order.clone());
    let bin = Tool::EbStack.bin(req.site);
    let mut args = vec![
        "stack".to_string(),
        "order".into(),
        "--out".into(),
        order.display().to_string(),
    ];
    for tree in &trees {
        args.push("--easyconfigs".into());
        args.push(tree.clone());
    }
    for root in &roots {
        args.push("--root".into());
        args.push(root.clone());
    }
    let shown: Vec<&str> = std::iter::once(bin.as_str())
        .chain(args.iter().map(String::as_str))
        .collect();
    let out = Command::new(&bin)
        .args(&args)
        .stdin(Stdio::null())
        .output()
        .map_err(|e| Fail::usage(format!("cannot run `{bin}`: {e}")))?;
    if !out.status.success() {
        let code = subprocess_code(&out.status);
        let err = stderr_text(&out.stderr);
        let exit_code = if code == 2 {
            exit::USAGE
        } else if out.status.code().is_none() {
            code
        } else {
            exit::REFUSED
        };
        return Err(Fail::new(
            exit_code,
            format!("`{}` exited {code}: {err}", shown.join(" ")),
        ));
    }
    let text = fs::read_to_string(&order).map_err(|e| {
        Fail::new(
            exit::REFUSED,
            format!("cannot read stack order output {}: {e}", order.display()),
        )
    })?;
    let paths: Vec<PathBuf> = text
        .lines()
        .map(str::trim)
        .filter(|l| !l.is_empty())
        .map(PathBuf::from)
        .collect();
    if paths.is_empty() {
        return Err(Fail::new(
            exit::REFUSED,
            "eb-stack stack order wrote no easyconfig paths",
        ));
    }
    Ok(paths)
}

fn recipe_easyblock(req: &Request<'_>, easyconfig: &Path) -> Result<String, Fail> {
    let (name, _) = package_of(&easyconfig.display().to_string());
    if name.is_empty() {
        return Err(Fail::new(
            exit::REFUSED,
            format!(
                "{} names no package for recipe easyblock",
                easyconfig.display()
            ),
        ));
    }
    let bin = Tool::EbStack.bin(req.site);
    let text = run(
        &[
            bin.clone(),
            "recipe".into(),
            "easyblock".into(),
            name.clone(),
        ],
        exit::REFUSED,
    )?;
    let class = text.lines().next().unwrap_or("").trim();
    if class.is_empty() || !is_ident(class) {
        return Err(Fail::new(
            exit::REFUSED,
            format!("eb-stack recipe easyblock {name} printed no class"),
        ));
    }
    Ok(class.to_string())
}

fn route_receipt(req: &Request<'_>) -> Value {
    let Some(target) = req.site.targets.iter().find(|t| t.name == req.target) else {
        return Value::Null;
    };
    let Some(name) = target
        .route
        .as_ref()
        .and_then(|r| r.receipt_digest_env.as_deref())
    else {
        return Value::Null;
    };
    match std::env::var(name) {
        Ok(v) if !v.is_empty() => Value::String(v),
        _ => Value::Null,
    }
}

/// A quoted literal that ends in `.patch`.
fn patch_literals(text: &str) -> Result<Vec<String>, &'static str> {
    let mut out = Vec::new();
    for statement in python_statements(text) {
        let Some(rest) = statement.trim().strip_prefix("patches") else {
            continue;
        };
        let rest = rest.trim_start();
        let (append, rhs) = if let Some(rhs) = rest.strip_prefix("+=") {
            (true, rhs)
        } else if let Some(rhs) = rest.strip_prefix('=') {
            (false, rhs)
        } else {
            continue;
        };
        if !append {
            out.clear();
        }
        for literal in python_string_literals(rhs)? {
            if literal.ends_with(".patch") && literal != ".patch" {
                out.push(literal);
            }
        }
    }
    Ok(out)
}

fn python_string_literals(text: &str) -> Result<Vec<String>, &'static str> {
    let characters: Vec<char> = text.chars().collect();
    let mut literals = Vec::new();
    let mut index = 0;
    while index < characters.len() {
        while index < characters.len() && characters[index] != '\'' && characters[index] != '"' {
            index += 1;
        }
        if index == characters.len() {
            break;
        }
        let mut literal = String::new();
        loop {
            let quote = characters[index];
            index += 1;
            let mut closed = false;
            while index < characters.len() {
                let character = characters[index];
                index += 1;
                if character == quote {
                    closed = true;
                    break;
                }
                if character == '\\' && index < characters.len() {
                    let escaped = characters[index];
                    index += 1;
                    match escaped {
                        'n' => literal.push('\n'),
                        'r' => literal.push('\r'),
                        't' => literal.push('\t'),
                        '\\' | '\'' | '"' => literal.push(escaped),
                        _ => {
                            literal.push('\\');
                            literal.push(escaped);
                        }
                    }
                } else {
                    literal.push(character);
                }
            }
            if !closed {
                return Err("unclosed quoted string in `patches`");
            }
            while index < characters.len() && characters[index].is_whitespace() {
                index += 1;
            }
            if index == characters.len() || (characters[index] != '\'' && characters[index] != '"')
            {
                break;
            }
        }
        literals.push(literal);
    }
    Ok(literals)
}

fn python_statements(text: &str) -> Vec<String> {
    fn finish(statements: &mut Vec<String>, statement: &mut String) {
        if !statement.trim().is_empty() {
            statements.push(std::mem::take(statement));
        } else {
            statement.clear();
        }
    }

    let mut statements = Vec::new();
    let mut statement = String::new();
    let mut quote = None;
    let mut escaped = false;
    let mut comment = false;
    let mut depth = 0_u32;
    for character in text.chars() {
        if comment {
            if character == '\n' {
                comment = false;
                if depth == 0 {
                    finish(&mut statements, &mut statement);
                } else {
                    statement.push(' ');
                }
            }
            continue;
        }
        if let Some(delimiter) = quote {
            statement.push(character);
            if escaped {
                escaped = false;
            } else if character == '\\' {
                escaped = true;
            } else if character == delimiter {
                quote = None;
            }
            continue;
        }
        match character {
            '\'' | '"' => {
                quote = Some(character);
                statement.push(character);
            }
            '#' => comment = true,
            '[' | '(' | '{' => {
                depth += 1;
                statement.push(character);
            }
            ']' | ')' | '}' => {
                depth = depth.saturating_sub(1);
                statement.push(character);
            }
            '\n' if depth == 0 => finish(&mut statements, &mut statement),
            ';' if depth == 0 => finish(&mut statements, &mut statement),
            '\n' => statement.push(' '),
            _ => statement.push(character),
        }
    }
    finish(&mut statements, &mut statement);
    statements
}

/// `easyblock = 'Class'` on its own, ignoring a trailing `#` comment.
fn easyblock_literal(text: &str) -> Option<String> {
    let mut literal = None;
    for statement in python_statements(text) {
        let Some(rest) = statement.trim().strip_prefix("easyblock") else {
            continue;
        };
        let Some(rest) = rest.trim().strip_prefix('=') else {
            continue;
        };
        literal = None;
        let Some(quote) = rest.trim().chars().next() else {
            continue;
        };
        if quote != '\'' && quote != '"' {
            continue;
        }
        let body = &rest.trim()[quote.len_utf8()..];
        let Some(end) = body.find(quote) else {
            continue;
        };
        let name = &body[..end];
        if body[end + quote.len_utf8()..].trim().is_empty() && is_ident(name) {
            literal = Some(name.to_string());
        }
    }
    literal
}

fn is_ident(name: &str) -> bool {
    !name.is_empty() && name.chars().all(|c| c.is_ascii_alphanumeric() || c == '_')
}

struct Index {
    by_name: BTreeMap<String, Vec<PathBuf>>,
}

fn index_files(roots: &[PathBuf], suffix: &str) -> Result<Index, Fail> {
    let mut by_name: BTreeMap<String, Vec<PathBuf>> = BTreeMap::new();
    for root in roots {
        if root.as_os_str().is_empty() {
            continue;
        }
        if !root.is_dir() {
            return Err(Fail::new(
                exit::REFUSED,
                format!("{} is not a directory", root.display()),
            ));
        }
        walk(root, suffix, &mut by_name)?;
    }
    Ok(Index { by_name })
}

fn walk(dir: &Path, suffix: &str, out: &mut BTreeMap<String, Vec<PathBuf>>) -> Result<(), Fail> {
    let entries = fs::read_dir(dir)
        .map_err(|e| Fail::new(exit::REFUSED, format!("cannot read {}: {e}", dir.display())))?;
    for entry in entries {
        let entry = entry
            .map_err(|e| Fail::new(exit::REFUSED, format!("cannot read {}: {e}", dir.display())))?;
        let path = entry.path();
        let ft = entry.file_type().map_err(|e| {
            Fail::new(
                exit::REFUSED,
                format!("cannot read {}: {e}", path.display()),
            )
        })?;
        if ft.is_symlink() {
            match fs::metadata(&path) {
                Ok(meta) if meta.is_dir() => continue,
                Ok(meta) if meta.is_file() => {}
                _ => continue,
            }
        } else if ft.is_dir() {
            walk(&path, suffix, out)?;
            continue;
        } else if !ft.is_file() {
            continue;
        }
        let Some(name) = path.file_name().and_then(|n| n.to_str()) else {
            continue;
        };
        if name.ends_with(suffix) {
            out.entry(name.to_string()).or_default().push(path);
        }
    }
    Ok(())
}

fn find_easyblock(class: &str, index: &Index) -> Result<Option<ResolvedFile>, Fail> {
    let mut found = Vec::new();
    for paths in index.by_name.values() {
        for path in paths {
            let text = fs::read_to_string(path).map_err(|e| {
                Fail::new(
                    exit::REFUSED,
                    format!("cannot read {}: {e}", path.display()),
                )
            })?;
            if defines_class(&text, class) {
                found.push(path.clone());
            }
        }
    }
    pick_one(class, &found)
}

fn defines_class(text: &str, class: &str) -> bool {
    text.lines().any(|line| {
        let rest = line.trim().strip_prefix("class ").unwrap_or("");
        let name: String = rest
            .chars()
            .take_while(|c| c.is_ascii_alphanumeric() || *c == '_')
            .collect();
        name == class
    })
}

fn resolve_patch(
    literal: &str,
    easyconfig_dirs: &[PathBuf],
    index: &Index,
) -> Result<ResolvedFile, Fail> {
    let mut found = Vec::new();
    let as_path = Path::new(literal);
    if as_path.is_absolute() && as_path.is_file() {
        found.push(as_path.to_path_buf());
    }
    for dir in easyconfig_dirs {
        let beside = dir.join(literal);
        if beside.is_file() {
            found.push(beside);
        }
    }
    if let Some(name) = as_path.file_name().and_then(|s| s.to_str()) {
        if let Some(paths) = index.by_name.get(name) {
            found.extend(paths.iter().cloned());
        }
    }
    found.sort();
    found.dedup();
    pick_one(literal, &found)?.ok_or_else(|| {
        Fail::new(
            exit::REFUSED,
            format!("[attest] found no file for patch `{literal}`"),
        )
    })
}

fn pick_one(what: &str, paths: &[PathBuf]) -> Result<Option<ResolvedFile>, Fail> {
    if paths.is_empty() {
        return Ok(None);
    }
    let mut hashed = Vec::new();
    for path in paths {
        hashed.push(ResolvedFile {
            sha256: sha256_hex(&read_bytes(path)?),
            path: path.clone(),
        });
    }
    hashed.sort_by(|a, b| a.path.cmp(&b.path));
    let first = hashed[0].sha256.clone();
    if hashed.iter().any(|f| f.sha256 != first) {
        let listed = hashed
            .iter()
            .map(|f| f.path.display().to_string())
            .collect::<Vec<_>>()
            .join(", ");
        return Err(Fail::new(
            exit::REFUSED,
            format!("[attest] {what} resolves to more than one file: {listed}"),
        ));
    }
    Ok(Some(hashed.into_iter().next().expect("one path")))
}

fn absolute(path: &Path) -> Result<PathBuf, Fail> {
    std::path::absolute(path)
        .map_err(|e| Fail::usage(format!("cannot resolve {}: {e}", path.display())))
}

fn read_bytes(path: &Path) -> Result<Vec<u8>, Fail> {
    fs::read(path).map_err(|e| {
        Fail::new(
            exit::REFUSED,
            format!("cannot read {}: {e}", path.display()),
        )
    })
}

fn sha256_hex(bytes: &[u8]) -> String {
    let digest: [u8; 32] = Sha256::digest(bytes).into();
    digest.iter().map(|b| format!("{b:02x}")).collect()
}

fn nanos() -> u128 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or(0)
}

fn argv(bin: &str, args: &[&str]) -> Vec<String> {
    std::iter::once(bin.to_string())
        .chain(args.iter().map(|s| (*s).to_string()))
        .collect()
}

fn run(argv: &[String], on_error: i32) -> Result<String, Fail> {
    let bin = &argv[0];
    let out = Command::new(bin)
        .args(&argv[1..])
        .stdin(Stdio::null())
        .output()
        .map_err(|e| Fail::usage(format!("cannot run `{bin}`: {e}")))?;
    if !out.status.success() {
        let code = subprocess_code(&out.status);
        let err = stderr_text(&out.stderr);
        let exit_code = if code == 2 {
            exit::USAGE
        } else if out.status.code().is_none() {
            code
        } else {
            on_error
        };
        return Err(Fail::new(
            exit_code,
            format!("`{}` exited {code}: {err}", argv.join(" ")),
        ));
    }
    Ok(String::from_utf8_lossy(&out.stdout).into_owned())
}

fn subprocess_code(status: &ExitStatus) -> i32 {
    status
        .code()
        .or_else(|| status.signal().map(|signal| 128 + signal))
        .unwrap_or(exit::FAILED)
}

fn stderr_text(bytes: &[u8]) -> String {
    let text = String::from_utf8_lossy(bytes);
    let trimmed = text.trim();
    if trimmed.chars().count() > 500 {
        trimmed.chars().take(500).collect::<String>() + "..."
    } else if trimmed.is_empty() {
        "no message".into()
    } else {
        trimmed.to_string()
    }
}

struct RemoveFile(PathBuf);
impl Drop for RemoveFile {
    fn drop(&mut self) {
        let _ = fs::remove_file(&self.0);
    }
}

struct RemoveDir(PathBuf);
impl Drop for RemoveDir {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

#[cfg(test)]
mod tests {
    use super::{easyblock_literal, find_easyblock, index_files, patch_literals, run};
    use crate::exit;

    #[test]
    fn a_quoted_patch_and_an_easyblock_literal_are_read() {
        let text = "easyblock = 'EB_Foo'  # local\npatches = ['a.patch', \"dir/b.patch\"]\n";
        assert_eq!(easyblock_literal(text).as_deref(), Some("EB_Foo"));
        assert_eq!(patch_literals(text).unwrap(), ["a.patch", "dir/b.patch"]);
    }

    #[test]
    fn the_last_easyblock_assignment_determines_the_literal() {
        assert_eq!(
            easyblock_literal("easyblock = 'EB_Old'\neasyblock = 'EB_New'\n").as_deref(),
            Some("EB_New")
        );
        assert_eq!(
            easyblock_literal("easyblock = 'EB_Old'\neasyblock = selected_easyblock\n"),
            None
        );
    }

    #[test]
    fn an_easyblock_expression_is_not_a_string_literal() {
        assert_eq!(easyblock_literal("easyblock = 'EB_Base' + '_Tool'\n"), None);
        assert_eq!(
            easyblock_literal("easyblock = 'ConfigureMake', 'autoconf'\n"),
            None
        );
    }

    #[test]
    fn only_patch_assignments_contribute_uncommented_literals() {
        let text = "\
patches_extra = ['prefix.patch']
description = 'unrelated.patch'
patches = [
    'real.patch',
    # 'comment.patch'
]
";
        assert_eq!(patch_literals(text).unwrap(), ["real.patch"]);
    }

    #[test]
    fn patch_assignment_replaces_and_append_assignment_extends() {
        let text = "\
patches = ['old.patch']
patches = ['new.patch']
patches += ['extra.patch']
";
        assert_eq!(patch_literals(text).unwrap(), ["new.patch", "extra.patch"]);
    }

    #[test]
    fn adjacent_patch_string_literals_are_concatenated() {
        let text = "patches = ['a' '.patch', 'dir/' \"b.patch\"]\n";
        assert_eq!(patch_literals(text).unwrap(), ["a.patch", "dir/b.patch"]);
    }

    #[test]
    fn an_unclosed_patch_string_does_not_return_a_partial_list() {
        let text = "patches = ['first.patch', 'second.patch\n";
        assert_eq!(
            patch_literals(text).unwrap_err(),
            "unclosed quoted string in `patches`"
        );
    }

    #[test]
    fn a_commented_easyblock_is_not_a_literal() {
        let text = "# easyblock = 'No'\nname = 'Foo'\n";
        assert_eq!(easyblock_literal(text), None);
    }

    #[test]
    fn an_unreadable_easyblock_file_is_refused() {
        let root = tempfile::tempdir().unwrap();
        let path = root.path().join("eb_foo.py");
        std::fs::write(&path, [0xff]).unwrap();
        let index = index_files(&[root.path().to_path_buf()], ".py").unwrap();
        let failure = match find_easyblock("EB_Foo", &index) {
            Err(failure) => failure,
            Ok(_) => panic!("an unreadable easyblock was ignored"),
        };
        assert_eq!(failure.code, exit::REFUSED);
        assert!(failure.msg.contains("cannot read"), "{}", failure.msg);
        assert!(
            failure.msg.contains(&path.display().to_string()),
            "{}",
            failure.msg
        );
    }

    #[test]
    fn a_missing_helper_is_a_usage_error() {
        let root = tempfile::tempdir().unwrap();
        let missing = root.path().join("missing-attestation-helper");
        let failure = run(&[missing.display().to_string()], exit::REFUSED).unwrap_err();
        assert_eq!(failure.code, exit::USAGE);
        assert!(failure.msg.contains("cannot run"), "{}", failure.msg);
    }
}
