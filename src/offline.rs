//! Content-addressed offline bundles made after source prefetch.

use crate::exit::{self, Fail};
use crate::resolve::Scope;
use crate::schema::OfflineAsset;
use serde::Serialize;
use sha2::{Digest, Sha256};
use std::collections::{BTreeMap, BTreeSet};
use std::fs::File;
use std::io::Read;
use std::path::{Path, PathBuf};

/// One file in an offline bundle.
#[derive(Debug, Clone, Serialize, PartialEq, Eq, PartialOrd, Ord)]
pub struct Entry {
    /// Upstream URL, or an `easybuild-source:` locator for a prefetched file.
    pub url: String,
    /// SHA-256 of the bytes.
    pub sha256: String,
    /// Name the consumer uses.
    pub local_name: String,
    /// Package, test or command that consumes the file.
    pub consumer: String,
    /// Path below the bundle directory.
    pub object: String,
}

/// Portable index written as `manifest.json`.
#[derive(Debug, Serialize)]
pub struct Manifest {
    /// Version of this format.
    pub schema_version: u32,
    /// `offline-bundle`.
    pub kind: &'static str,
    /// Files sorted by URL, consumer and local name.
    pub files: Vec<Entry>,
}

fn sha256(path: &Path) -> Result<String, Fail> {
    let mut file = File::open(path).map_err(|e| {
        Fail::new(
            exit::FAILED,
            format!("cannot read offline asset {}: {e}", path.display()),
        )
    })?;
    let mut hash = Sha256::new();
    let mut buf = [0u8; 64 * 1024];
    loop {
        let n = file.read(&mut buf).map_err(|e| {
            Fail::new(
                exit::FAILED,
                format!("cannot read offline asset {}: {e}", path.display()),
            )
        })?;
        if n == 0 {
            break;
        }
        hash.update(&buf[..n]);
    }
    Ok(hash
        .finalize()
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect())
}

fn files_under(dir: &Path, out: &mut Vec<PathBuf>) -> Result<(), Fail> {
    let entries = std::fs::read_dir(dir).map_err(|e| {
        Fail::new(
            exit::FAILED,
            format!("cannot list source path {}: {e}", dir.display()),
        )
    })?;
    for entry in entries {
        let entry = entry.map_err(|e| {
            Fail::new(
                exit::FAILED,
                format!("cannot list source path {}: {e}", dir.display()),
            )
        })?;
        let path = entry.path();
        let kind = entry.file_type().map_err(|e| {
            Fail::new(
                exit::FAILED,
                format!("cannot inspect source path {}: {e}", path.display()),
            )
        })?;
        if kind.is_dir() {
            files_under(&path, out)?;
        } else if kind.is_file() {
            out.push(path);
        } else if kind.is_symlink() {
            return Err(Fail::new(
                exit::FAILED,
                format!(
                    "source path {} is a symbolic link; an offline bundle only copies regular files",
                    path.display()
                ),
            ));
        }
    }
    Ok(())
}

fn encode_locator(text: &str) -> String {
    let mut out = String::new();
    for byte in text.bytes() {
        if byte.is_ascii_alphanumeric() || matches!(byte, b'/' | b'.' | b'_' | b'-') {
            out.push(byte as char);
        } else {
            out.push_str(&format!("%{byte:02X}"));
        }
    }
    out
}

fn store(root: &Path, path: &Path, digest: &str) -> Result<String, Fail> {
    let rel = format!("objects/sha256/{}/{}", &digest[..2], digest);
    let dest = root.join(&rel);
    if dest.is_file() {
        let found = sha256(&dest)?;
        if found != digest {
            return Err(Fail::new(
                exit::FAILED,
                format!(
                    "offline object {} has SHA-256 {found}, expected {digest}",
                    dest.display()
                ),
            ));
        }
        return Ok(rel);
    }
    let parent = dest.parent().expect("object has a parent");
    std::fs::create_dir_all(parent).map_err(|e| {
        Fail::new(
            exit::FAILED,
            format!("cannot create {}: {e}", parent.display()),
        )
    })?;
    let tmp = parent.join(format!(".{digest}.tmp-{}", std::process::id()));
    std::fs::copy(path, &tmp).map_err(|e| {
        Fail::new(
            exit::FAILED,
            format!(
                "cannot copy offline asset {} to {}: {e}",
                path.display(),
                tmp.display()
            ),
        )
    })?;
    let copied = match sha256(&tmp) {
        Ok(copied) => copied,
        Err(failure) => {
            let _ = std::fs::remove_file(&tmp);
            return Err(failure);
        }
    };
    if copied != digest {
        let _ = std::fs::remove_file(&tmp);
        return Err(Fail::new(
            exit::FAILED,
            format!(
                "offline asset {} changed while it was copied: expected SHA-256 {digest}, copied {copied}",
                path.display()
            ),
        ));
    }
    std::fs::rename(&tmp, &dest).map_err(|e| {
        let _ = std::fs::remove_file(&tmp);
        Fail::new(
            exit::FAILED,
            format!("cannot publish offline object {}: {e}", dest.display()),
        )
    })?;
    Ok(rel)
}

fn local_name(path: &Path) -> Result<String, Fail> {
    path.file_name()
        .and_then(|name| name.to_str())
        .map(str::to_string)
        .ok_or_else(|| {
            Fail::new(
                exit::FAILED,
                format!("offline asset {} has no UTF-8 file name", path.display()),
            )
        })
}

/// Create a new bundle directory. Existing paths are refused so a failed
/// rerun cannot mix new planner files with an earlier valid manifest.
pub fn prepare(root: &Path) -> Result<(), Fail> {
    std::fs::create_dir(root).map_err(|e| {
        Fail::new(
            exit::FAILED,
            format!(
                "cannot create new offline bundle {}: {e}; choose an absent directory",
                root.display()
            ),
        )
    })
}

fn validate_locators(entries: &BTreeSet<Entry>) -> Result<(), Fail> {
    let mut locators = BTreeMap::new();
    for entry in entries {
        let key = (
            entry.url.as_str(),
            entry.consumer.as_str(),
            entry.local_name.as_str(),
        );
        if let Some(earlier) = locators.insert(key, entry.sha256.as_str()) {
            if earlier != entry.sha256 {
                return Err(Fail::new(
                    exit::FAILED,
                    format!(
                        "conflicting offline locator {} for consumer {} and local name {}",
                        entry.url, entry.consumer, entry.local_name
                    ),
                ));
            }
        }
    }
    Ok(())
}

/// Copy prefetched EasyBuild files and configured assets into `root`, then
/// atomically write their checksum index.
pub fn write(root: &Path, scope: &Scope<'_>, assets: &[OfflineAsset]) -> Result<Manifest, Fail> {
    let mut entries = BTreeSet::new();
    let canonical_root = root.canonicalize().map_err(|e| {
        Fail::new(
            exit::FAILED,
            format!("cannot resolve offline bundle {}: {e}", root.display()),
        )
    })?;
    let planner = root.join("planner");
    if planner.is_dir() {
        let mut files = Vec::new();
        files_under(&planner, &mut files)?;
        files.sort();
        for path in files {
            let digest = sha256(&path)?;
            let rel = path.strip_prefix(&planner).unwrap_or(&path);
            entries.insert(Entry {
                url: format!(
                    "eb-stack-planner:{}",
                    encode_locator(&rel.to_string_lossy())
                ),
                sha256: digest.clone(),
                local_name: local_name(&path)?,
                consumer: "eb-stack".into(),
                object: store(root, &path, &digest)?,
            });
        }
    }
    for source in scope.source_paths() {
        let source = PathBuf::from(source);
        if !source.exists() {
            continue;
        }
        let canonical_source = source.canonicalize().map_err(|e| {
            Fail::new(
                exit::FAILED,
                format!("cannot resolve source path {}: {e}", source.display()),
            )
        })?;
        let mut files = Vec::new();
        files_under(&source, &mut files)?;
        files.sort();
        for path in files {
            let canonical_path = path.canonicalize().map_err(|e| {
                Fail::new(
                    exit::FAILED,
                    format!("cannot resolve offline input {}: {e}", path.display()),
                )
            })?;
            if canonical_path.starts_with(&canonical_root) {
                continue;
            }
            let digest = sha256(&path)?;
            let rel = canonical_path
                .strip_prefix(&canonical_source)
                .unwrap_or(&canonical_path);
            entries.insert(Entry {
                url: format!(
                    "easybuild-source:{}",
                    encode_locator(&rel.to_string_lossy())
                ),
                sha256: digest.clone(),
                local_name: local_name(&path)?,
                consumer: "easybuild".into(),
                object: store(root, &path, &digest)?,
            });
        }
    }
    for asset in assets {
        let path = Path::new(&asset.path);
        let kind = path.symlink_metadata().map_err(|e| {
            Fail::new(
                exit::FAILED,
                format!("cannot inspect offline asset {}: {e}", path.display()),
            )
        })?;
        if !kind.file_type().is_file() {
            return Err(Fail::new(
                exit::FAILED,
                format!("offline asset {} is not a regular file", path.display()),
            ));
        }
        let digest = sha256(path)?;
        if digest != asset.sha256 {
            return Err(Fail::new(
                exit::FAILED,
                format!(
                    "offline asset {} has SHA-256 {digest}, expected {}",
                    path.display(),
                    asset.sha256
                ),
            ));
        }
        entries.insert(Entry {
            url: asset.url.clone(),
            sha256: digest.clone(),
            local_name: match &asset.local_name {
                Some(name) => name.clone(),
                None => local_name(path)?,
            },
            consumer: asset.consumer.clone(),
            object: store(root, path, &digest)?,
        });
    }
    validate_locators(&entries)?;
    let manifest = Manifest {
        schema_version: 1,
        kind: "offline-bundle",
        files: entries.into_iter().collect(),
    };
    let text = serde_json::to_string_pretty(&manifest)
        .map_err(|e| Fail::new(exit::FAILED, format!("cannot render offline manifest: {e}")))?
        + "\n";
    crate::util::write_atomic(&root.join("manifest.json"), text.as_bytes()).map_err(|e| {
        Fail::new(
            exit::FAILED,
            format!("cannot publish offline manifest: {e}"),
        )
    })?;
    Ok(manifest)
}

#[cfg(test)]
mod tests {
    use super::{validate_locators, Entry};
    use std::collections::BTreeSet;

    fn entry(digest: &str) -> Entry {
        Entry {
            url: "easybuild-source:conflict.tar.gz".into(),
            sha256: digest.into(),
            local_name: "conflict.tar.gz".into(),
            consumer: "easybuild".into(),
            object: format!("objects/sha256/{digest}"),
        }
    }

    #[test]
    fn conflicting_consumer_locators_are_refused() {
        let entries = BTreeSet::from([entry("first"), entry("second")]);
        let failure = validate_locators(&entries).unwrap_err();
        assert!(failure.msg.contains("conflicting offline locator"));
    }
}
