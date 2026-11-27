//! Optional check of one ingest archive before a CVMFS transaction.

use eb_pipeline::util;
use flate2::bufread::GzDecoder;
use serde::Deserialize;
use sha2::{Digest, Sha256};
use std::collections::BTreeSet;
use std::fs::{self, File};
use std::io::{self, BufRead, BufReader, Read, Seek, SeekFrom};
use std::path::{Path, PathBuf};

const MAX_MEMBER_PATH: usize = 4096;
const MAX_META_BYTES: u64 = 1024 * 1024;

pub(crate) fn check(manifest_path: &Path) -> Result<(), String> {
    let manifest = load_manifest(manifest_path)?;
    let directory = resolve_directory(manifest_path, &manifest.directory)?;
    let archive_path = one_archive(&directory, &manifest.name)?;
    let mut file = util::open_regular_file(&archive_path).map_err(|error| {
        fail(&format!(
            "cannot read archive {}: {error}",
            archive_path.display()
        ))
    })?;
    let actual = sha256_file(&mut file)?;
    if actual != manifest.sha256 {
        return Err(fail("archive sha256 does not match the manifest"));
    }
    file.seek(SeekFrom::Start(0))
        .map_err(|error| fail(&format!("cannot read archive: {error}")))?;
    parse_archive(
        &mut file,
        manifest.compression,
        manifest.uid_type,
        manifest.gid_type,
    )
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct ManifestFile {
    directory: String,
    name: String,
    sha256: String,
    uid_type: IdType,
    gid_type: IdType,
}

struct Manifest {
    directory: String,
    name: String,
    sha256: String,
    uid_type: IdType,
    gid_type: IdType,
    compression: Compression,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
enum IdType {
    #[serde(rename = "u16")]
    U16,
    #[serde(rename = "i32")]
    I32,
    #[serde(rename = "u32")]
    U32,
}

impl IdType {
    fn as_str(self) -> &'static str {
        match self {
            Self::U16 => "u16",
            Self::I32 => "i32",
            Self::U32 => "u32",
        }
    }

    fn max(self) -> u128 {
        match self {
            Self::U16 => u128::from(u16::MAX),
            Self::I32 => u128::from(i32::MAX as u32),
            Self::U32 => u128::from(u32::MAX),
        }
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Compression {
    Plain,
    Gzip,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Kind {
    File,
    Directory,
    Link,
}

#[derive(Default)]
struct Overrides {
    path: Option<String>,
    link: Option<String>,
    size: Option<u128>,
    uid: Option<u128>,
    gid: Option<u128>,
}

struct Tree {
    dirs: BTreeSet<String>,
    seen: BTreeSet<String>,
    members: u64,
}

fn load_manifest(path: &Path) -> Result<Manifest, String> {
    let file = util::open_regular_file(path)
        .map_err(|error| fail(&format!("cannot read manifest {}: {error}", path.display())))?;
    let parsed: ManifestFile = serde_json::from_reader(file)
        .map_err(|error| fail(&format!("manifest is invalid: {error}")))?;
    if parsed.directory.is_empty() {
        return Err(fail("manifest directory is empty"));
    }
    let Some(compression) = compression_of(&parsed.name) else {
        return Err(fail(
            "manifest name must be one archive ending in .tar, .tar.gz, or .tgz",
        ));
    };
    if !valid_sha256(&parsed.sha256) {
        return Err(fail(
            "manifest sha256 must be 64 lowercase hexadecimal digits",
        ));
    }
    Ok(Manifest {
        directory: parsed.directory,
        name: parsed.name,
        sha256: parsed.sha256,
        uid_type: parsed.uid_type,
        gid_type: parsed.gid_type,
        compression,
    })
}

fn compression_of(name: &str) -> Option<Compression> {
    if name.is_empty()
        || name == "."
        || name == ".."
        || name.contains('/')
        || name.contains('\\')
        || name.contains('\0')
    {
        return None;
    }
    if name.ends_with(".tar.gz") || name.ends_with(".tgz") {
        Some(Compression::Gzip)
    } else if name.ends_with(".tar") {
        Some(Compression::Plain)
    } else {
        None
    }
}

fn valid_sha256(text: &str) -> bool {
    text.len() == 64
        && text
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
}

fn resolve_directory(manifest: &Path, directory: &str) -> Result<PathBuf, String> {
    let relative = Path::new(directory);
    if relative.is_absolute() {
        return Ok(relative.to_path_buf());
    }
    match manifest.parent() {
        Some(parent) if !parent.as_os_str().is_empty() => Ok(parent.join(relative)),
        _ => Ok(relative.to_path_buf()),
    }
}

fn one_archive(directory: &Path, name: &str) -> Result<PathBuf, String> {
    match fs::metadata(directory) {
        Ok(meta) if meta.is_dir() => {}
        Ok(_) => return Err(fail("directory is not a directory")),
        Err(error) if error.kind() == io::ErrorKind::NotFound => {
            return Err(fail("directory does not exist"));
        }
        Err(error) => {
            return Err(fail(&format!(
                "cannot read directory {}: {error}",
                directory.display()
            )));
        }
    }
    let mut entries = Vec::new();
    for entry in fs::read_dir(directory).map_err(|error| {
        fail(&format!(
            "cannot read directory {}: {error}",
            directory.display()
        ))
    })? {
        entries.push(entry.map_err(|error| {
            fail(&format!(
                "cannot read directory {}: {error}",
                directory.display()
            ))
        })?);
    }
    let entry = match entries.len() {
        0 => return Err(fail("directory contains no archive")),
        1 => &entries[0],
        count => {
            return Err(fail(&format!(
                "directory contains {count} entries, expected exactly one archive"
            )));
        }
    };
    if entry.file_name() != std::ffi::OsStr::new(name)
        || !entry
            .file_type()
            .map_err(|error| {
                fail(&format!(
                    "cannot read directory {}: {error}",
                    directory.display()
                ))
            })?
            .is_file()
    {
        return Err(fail("unrecognized archive entry"));
    }
    let len = entry
        .metadata()
        .map_err(|error| {
            fail(&format!(
                "cannot read directory {}: {error}",
                directory.display()
            ))
        })?
        .len();
    if len == 0 {
        return Err(fail("archive is empty"));
    }
    Ok(entry.path())
}

fn sha256_file(file: &mut File) -> Result<String, String> {
    file.seek(SeekFrom::Start(0))
        .map_err(|error| fail(&format!("cannot read archive: {error}")))?;
    let mut hasher = Sha256::new();
    let mut buffer = [0u8; 64 * 1024];
    loop {
        let count = file
            .read(&mut buffer)
            .map_err(|error| fail(&format!("cannot read archive: {error}")))?;
        if count == 0 {
            break;
        }
        hasher.update(&buffer[..count]);
    }
    Ok(hex(&hasher.finalize()))
}

fn hex(bytes: &[u8]) -> String {
    const DIGITS: &[u8; 16] = b"0123456789abcdef";
    let mut text = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        text.push(DIGITS[usize::from(byte >> 4)] as char);
        text.push(DIGITS[usize::from(byte & 0x0f)] as char);
    }
    text
}

fn parse_archive(
    file: &mut File,
    compression: Compression,
    uid_type: IdType,
    gid_type: IdType,
) -> Result<(), String> {
    let mut magic = [0u8; 2];
    let count = file
        .read(&mut magic)
        .map_err(|error| fail(&format!("cannot read archive: {error}")))?;
    file.seek(SeekFrom::Start(0))
        .map_err(|error| fail(&format!("cannot read archive: {error}")))?;
    let gzip_magic = count >= 2 && magic == [0x1f, 0x8b];
    match compression {
        Compression::Gzip if !gzip_magic => Err(fail("unrecognized archive")),
        Compression::Plain if gzip_magic => Err(fail("unrecognized archive")),
        Compression::Plain => parse_tar(file, uid_type, gid_type),
        Compression::Gzip => {
            let mut decoder = GzDecoder::new(BufReader::new(file));
            parse_tar(&mut decoder, uid_type, gid_type)?;
            let mut inner = decoder.into_inner();
            let leftover = inner.fill_buf().map_err(|_| fail("unrecognized archive"))?;
            if leftover.is_empty() {
                Ok(())
            } else {
                Err(fail("unrecognized archive"))
            }
        }
    }
}

fn parse_tar<R: Read>(reader: &mut R, uid_type: IdType, gid_type: IdType) -> Result<(), String> {
    let mut pending = Overrides::default();
    let mut global = Overrides::default();
    let mut tree = Tree {
        dirs: BTreeSet::new(),
        seen: BTreeSet::new(),
        members: 0,
    };
    loop {
        let Some(block) = read_block(reader)? else {
            return Err(fail("unrecognized archive"));
        };
        if block.iter().all(|byte| *byte == 0) {
            let Some(second) = read_block(reader)? else {
                return Err(fail("unrecognized archive"));
            };
            if second.iter().any(|byte| *byte != 0) || consume_trailer(reader).is_err() {
                return Err(fail("unrecognized archive"));
            }
            break;
        }
        if header_checksum(&block).is_err() || &block[257..262] != b"ustar" {
            return Err(fail("unrecognized archive"));
        }
        match block[156] {
            b'L' | b'K' | b'x' | b'g' => {
                apply_meta(&block, reader, &mut pending, &mut global)?;
            }
            b'0' | 0 | b'5' | b'1' | b'2' => {
                accept_member(
                    &block,
                    reader,
                    &mut pending,
                    &global,
                    &mut tree,
                    uid_type,
                    gid_type,
                )?;
            }
            _ => return Err(fail("unrecognized archive")),
        }
    }
    if tree.members == 0 {
        Err(fail("archive has no members"))
    } else {
        Ok(())
    }
}

fn apply_meta<R: Read>(
    block: &[u8; 512],
    reader: &mut R,
    pending: &mut Overrides,
    global: &mut Overrides,
) -> Result<(), String> {
    let size = header_number(&block[124..136])?;
    let payload = read_meta(reader, size)?;
    match block[156] {
        b'L' => pending.path = Some(meta_text(&payload)?),
        b'K' => pending.link = Some(meta_text(&payload)?),
        b'x' => apply_pax(&payload, pending, false)?,
        b'g' => apply_pax(&payload, global, true)?,
        _ => return Err(fail("unrecognized archive")),
    }
    Ok(())
}

fn accept_member<R: Read>(
    block: &[u8; 512],
    reader: &mut R,
    pending: &mut Overrides,
    global: &Overrides,
    tree: &mut Tree,
    uid_type: IdType,
    gid_type: IdType,
) -> Result<(), String> {
    let header_size = header_number(&block[124..136])?;
    let size = pending.size.unwrap_or(header_size);
    let size = u64::try_from(size).map_err(|_| fail("unrecognized archive"))?;
    let kind = match block[156] {
        b'5' => Kind::Directory,
        b'1' | b'2' => Kind::Link,
        _ => Kind::File,
    };
    let raw_path = member_path(block, pending.path.as_deref())?;
    let path = normalize(&raw_path, kind == Kind::Directory).ok_or_else(|| {
        fail(&format!(
            "path `{}` is not normalized",
            display_path(&raw_path)
        ))
    })?;
    if kind == Kind::Link {
        let raw_link = pending
            .link
            .clone()
            .map_or_else(|| field_text(&block[157..257]), Ok)?;
        if normalize(&raw_link, false).is_none() {
            return Err(fail(&format!("path `{path}` target is not normalized")));
        }
    }
    require_ancestors(&path, tree)?;
    if !tree.seen.insert(path.clone()) {
        return Err(fail(&format!("duplicate path `{path}`")));
    }
    let uid = pending
        .uid
        .or(global.uid)
        .map_or_else(|| header_number(&block[108..116]), Ok)?;
    let gid = pending
        .gid
        .or(global.gid)
        .map_or_else(|| header_number(&block[116..124]), Ok)?;
    if uid > uid_type.max() {
        return Err(fail(&format!(
            "path `{path}` uid {uid} does not fit {}",
            uid_type.as_str()
        )));
    }
    if gid > gid_type.max() {
        return Err(fail(&format!(
            "path `{path}` gid {gid} does not fit {}",
            gid_type.as_str()
        )));
    }
    if kind == Kind::Directory {
        tree.dirs.insert(path);
    }
    tree.members += 1;
    *pending = Overrides::default();
    discard(reader, size)
}

fn member_path(block: &[u8; 512], pending: Option<&str>) -> Result<String, String> {
    if let Some(path) = pending {
        return Ok(path.to_string());
    }
    let name = field_text(&block[0..100])?;
    let prefix = field_text(&block[345..500])?;
    if prefix.is_empty() {
        Ok(name)
    } else if name.is_empty() {
        Ok(prefix)
    } else {
        Ok(format!("{prefix}/{name}"))
    }
}

fn require_ancestors(path: &str, tree: &Tree) -> Result<(), String> {
    let mut prefix = String::new();
    let parts: Vec<&str> = path.split('/').collect();
    for part in &parts[..parts.len() - 1] {
        if !prefix.is_empty() {
            prefix.push('/');
        }
        prefix.push_str(part);
        if !tree.dirs.contains(&prefix) {
            return Err(fail(&format!(
                "path `{path}` is missing explicit directory `{prefix}`"
            )));
        }
    }
    Ok(())
}

fn normalize(raw: &str, directory: bool) -> Option<String> {
    if raw.contains('\0') || raw.contains('\\') {
        return None;
    }
    let path = if directory {
        match raw.strip_suffix('/') {
            Some(stripped) if !stripped.is_empty() && !stripped.ends_with('/') => stripped,
            Some(_) => return None,
            None => raw,
        }
    } else if raw.ends_with('/') {
        return None;
    } else {
        raw
    };
    if path.len() > MAX_MEMBER_PATH || path.is_empty() || path.starts_with('/') {
        return None;
    }
    if path
        .split('/')
        .any(|part| part.is_empty() || part == "." || part == "..")
    {
        return None;
    }
    Some(path.to_string())
}

fn display_path(raw: &str) -> String {
    raw.chars().take(256).collect()
}

fn apply_pax(payload: &[u8], target: &mut Overrides, global: bool) -> Result<(), String> {
    for (key, value) in pax_records(payload)? {
        match key.as_str() {
            "path" | "linkpath" | "size" if global => return Err(fail("unrecognized archive")),
            "path" => target.path = Some(value),
            "linkpath" => target.link = Some(value),
            "size" => target.size = Some(parse_u128(&value)?),
            "uid" => target.uid = Some(parse_u128(&value)?),
            "gid" => target.gid = Some(parse_u128(&value)?),
            _ => {}
        }
    }
    Ok(())
}

fn pax_records(payload: &[u8]) -> Result<Vec<(String, String)>, String> {
    let mut records = Vec::new();
    let mut index = 0;
    while index < payload.len() {
        if payload[index] == 0 {
            if payload[index..].iter().all(|byte| *byte == 0) {
                break;
            }
            return Err(fail("unrecognized archive"));
        }
        let rest = &payload[index..];
        let Some(space) = rest.iter().position(|byte| *byte == b' ') else {
            return Err(fail("unrecognized archive"));
        };
        let length: usize = std::str::from_utf8(&rest[..space])
            .ok()
            .and_then(|text| text.parse().ok())
            .ok_or_else(|| fail("unrecognized archive"))?;
        let Some(end) = index.checked_add(length) else {
            return Err(fail("unrecognized archive"));
        };
        if length < space + 2 || end > payload.len() {
            return Err(fail("unrecognized archive"));
        }
        let record = &payload[index..end];
        if record.last() != Some(&b'\n') {
            return Err(fail("unrecognized archive"));
        }
        let body = &record[space + 1..record.len() - 1];
        let Some(equals) = body.iter().position(|byte| *byte == b'=') else {
            return Err(fail("unrecognized archive"));
        };
        let key = std::str::from_utf8(&body[..equals])
            .map_err(|_| fail("unrecognized archive"))?
            .to_string();
        let value = std::str::from_utf8(&body[equals + 1..])
            .map_err(|_| fail("unrecognized archive"))?
            .to_string();
        records.push((key, value));
        index = end;
    }
    Ok(records)
}

fn parse_u128(text: &str) -> Result<u128, String> {
    if text.is_empty() || !text.bytes().all(|byte| byte.is_ascii_digit()) {
        return Err(fail("unrecognized archive"));
    }
    text.parse().map_err(|_| fail("unrecognized archive"))
}

fn meta_text(payload: &[u8]) -> Result<String, String> {
    let end = payload
        .iter()
        .rposition(|byte| *byte != 0)
        .map(|index| index + 1)
        .unwrap_or(0);
    if payload[..end].contains(&0) {
        return Err(fail("unrecognized archive"));
    }
    String::from_utf8(payload[..end].to_vec()).map_err(|_| fail("unrecognized archive"))
}

fn field_text(field: &[u8]) -> Result<String, String> {
    let end = field
        .iter()
        .position(|byte| *byte == 0)
        .unwrap_or(field.len());
    String::from_utf8(field[..end].to_vec()).map_err(|_| fail("a member path is not normalized"))
}

fn header_number(field: &[u8]) -> Result<u128, String> {
    if field.is_empty() {
        return Err(fail("unrecognized archive"));
    }
    if field[0] == 0xff || (field[0] & 0x80 != 0 && field[0] != 0x80) {
        return Err(fail("unrecognized archive"));
    }
    if field[0] == 0x80 {
        let mut value: u128 = 0;
        for byte in &field[1..] {
            value = value
                .checked_shl(8)
                .ok_or_else(|| fail("unrecognized archive"))?
                | u128::from(*byte);
        }
        return Ok(value);
    }
    parse_octal(field)
}

fn parse_octal(field: &[u8]) -> Result<u128, String> {
    let is_pad = |byte: u8| byte == 0 || byte == b' ';
    let start = field
        .iter()
        .position(|byte| !is_pad(*byte))
        .unwrap_or(field.len());
    let length = field[start..]
        .iter()
        .position(|byte| is_pad(*byte))
        .unwrap_or(field.len() - start);
    let digits = &field[start..start + length];
    if field[start + length..].iter().any(|byte| !is_pad(*byte)) {
        return Err(fail("unrecognized archive"));
    }
    if digits.is_empty() {
        return Ok(0);
    }
    let text = std::str::from_utf8(digits).map_err(|_| fail("unrecognized archive"))?;
    u128::from_str_radix(text, 8).map_err(|_| fail("unrecognized archive"))
}

fn header_checksum(block: &[u8; 512]) -> Result<(), String> {
    let stored = parse_octal(&block[148..156])?;
    let mut sum = 0u32;
    for (index, byte) in block.iter().enumerate() {
        let add = if (148..156).contains(&index) {
            b' '
        } else {
            *byte
        };
        sum += u32::from(add);
    }
    if u128::from(sum) == stored {
        Ok(())
    } else {
        Err(fail("unrecognized archive"))
    }
}

fn read_block<R: Read>(reader: &mut R) -> Result<Option<[u8; 512]>, String> {
    let mut block = [0u8; 512];
    let mut filled = 0;
    while filled < block.len() {
        match reader.read(&mut block[filled..]) {
            Ok(0) if filled == 0 => return Ok(None),
            Ok(0) => return Err(fail("unrecognized archive")),
            Ok(count) => filled += count,
            Err(error) if error.kind() == io::ErrorKind::Interrupted => continue,
            Err(_) => return Err(fail("unrecognized archive")),
        }
    }
    Ok(Some(block))
}

fn read_meta<R: Read>(reader: &mut R, size: u128) -> Result<Vec<u8>, String> {
    if size > u128::from(MAX_META_BYTES) {
        return Err(fail("unrecognized archive"));
    }
    let size = u64::try_from(size).map_err(|_| fail("unrecognized archive"))?;
    let length = usize::try_from(size).map_err(|_| fail("unrecognized archive"))?;
    let mut payload = vec![0u8; length];
    reader
        .read_exact(&mut payload)
        .map_err(|_| fail("unrecognized archive"))?;
    let padded = padded_size(size)?;
    discard_n(reader, padded - size)?;
    Ok(payload)
}

fn discard<R: Read>(reader: &mut R, size: u64) -> Result<(), String> {
    discard_n(reader, padded_size(size)?)
}

fn discard_n<R: Read>(reader: &mut R, mut left: u64) -> Result<(), String> {
    let mut buffer = [0u8; 8192];
    while left > 0 {
        let chunk = usize::try_from(left.min(buffer.len() as u64)).unwrap_or(buffer.len());
        reader
            .read_exact(&mut buffer[..chunk])
            .map_err(|_| fail("unrecognized archive"))?;
        left -= u64::try_from(chunk).unwrap_or(0);
    }
    Ok(())
}

fn padded_size(size: u64) -> Result<u64, String> {
    let remainder = size % 512;
    if remainder == 0 {
        Ok(size)
    } else {
        size.checked_add(512 - remainder)
            .ok_or_else(|| fail("unrecognized archive"))
    }
}

fn consume_trailer<R: Read>(reader: &mut R) -> Result<(), ()> {
    let mut buffer = [0u8; 512];
    loop {
        let mut filled = 0;
        while filled < buffer.len() {
            match reader.read(&mut buffer[filled..]) {
                Ok(0) if filled == 0 => return Ok(()),
                Ok(0) => {
                    return if buffer[..filled].iter().any(|byte| *byte != 0) {
                        Err(())
                    } else {
                        Ok(())
                    };
                }
                Ok(count) => filled += count,
                Err(error) if error.kind() == io::ErrorKind::Interrupted => continue,
                Err(_) => return Err(()),
            }
        }
        if buffer.iter().any(|byte| *byte != 0) {
            return Err(());
        }
    }
}

fn fail(detail: &str) -> String {
    format!("archive manifest: {detail}")
}

#[cfg(test)]
#[allow(clippy::disallowed_methods)]
mod tests {
    use super::*;
    use std::ffi::CString;
    use std::os::unix::ffi::OsStrExt;
    use std::process::Command;

    #[test]
    fn archive_checker_refuses_a_fifo_manifest_promptly() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("manifest.json");
        let raw = CString::new(path.as_os_str().as_bytes()).unwrap();
        assert_eq!(unsafe { libc::mkfifo(raw.as_ptr(), 0o600) }, 0);
        let (send, receive) = std::sync::mpsc::channel();
        let reader = std::thread::spawn(move || {
            send.send(check(&path)).unwrap();
        });
        let error = receive
            .recv_timeout(std::time::Duration::from_secs(5))
            .expect("archive manifest reader blocked")
            .unwrap_err();
        assert!(error.contains("not a regular file"), "{error}");
        reader.join().unwrap();
    }

    fn sha256_hex(bytes: &[u8]) -> String {
        hex(&Sha256::digest(bytes))
    }

    fn write_manifest(
        root: &Path,
        directory: &str,
        name: &str,
        bytes: &[u8],
        uid_type: &str,
        gid_type: &str,
    ) -> PathBuf {
        let manifest = root.join("manifest.json");
        let json = format!(
            "{{\n  \"directory\": \"{directory}\",\n  \"name\": \"{name}\",\n  \"sha256\": \"{}\",\n  \"uid_type\": \"{uid_type}\",\n  \"gid_type\": \"{gid_type}\"\n}}\n",
            sha256_hex(bytes)
        );
        std::fs::write(&manifest, json).unwrap();
        manifest
    }

    fn put_octal(field: &mut [u8], value: u64) {
        let rendered = format!("{:0width$o}", value, width = field.len() - 1);
        assert!(rendered.len() < field.len());
        field[..rendered.len()].copy_from_slice(rendered.as_bytes());
    }

    fn finish(block: &mut [u8; 512]) {
        block[257..262].copy_from_slice(b"ustar");
        block[263..265].copy_from_slice(b"00");
        let mut sum = 0u32;
        for (index, byte) in block.iter().enumerate() {
            sum += u32::from(if (148..156).contains(&index) {
                b' '
            } else {
                *byte
            });
        }
        let text = format!("{sum:06o}\0 ");
        block[148..156].copy_from_slice(text.as_bytes());
    }

    fn header(name: &str, typeflag: u8, uid: u64, gid: u64, size: u64) -> [u8; 512] {
        let mut block = [0u8; 512];
        assert!(name.len() < 100);
        block[..name.len()].copy_from_slice(name.as_bytes());
        put_octal(&mut block[100..108], 0o644);
        put_octal(&mut block[108..116], uid);
        put_octal(&mut block[116..124], gid);
        put_octal(&mut block[124..136], size);
        block[156] = typeflag;
        finish(&mut block);
        block
    }

    fn archive(parts: &[Vec<u8>]) -> Vec<u8> {
        let mut bytes = Vec::new();
        for part in parts {
            bytes.extend(part);
        }
        bytes.extend(std::iter::repeat_n(0u8, 1024));
        bytes
    }

    fn file_bytes(name: &str, uid: u64, gid: u64, payload: &[u8]) -> Vec<u8> {
        let mut bytes = header(name, b'0', uid, gid, payload.len() as u64).to_vec();
        bytes.extend(payload);
        let pad = (512 - (payload.len() % 512)) % 512;
        bytes.extend(std::iter::repeat_n(0u8, pad));
        bytes
    }

    fn dir_bytes(name: &str, uid: u64, gid: u64) -> Vec<u8> {
        header(&format!("{name}/"), b'5', uid, gid, 0).to_vec()
    }

    fn place(root: &Path, name: &str, bytes: &[u8]) -> PathBuf {
        let directory = root.join("ingest");
        std::fs::create_dir_all(&directory).unwrap();
        std::fs::write(directory.join(name), bytes).unwrap();
        write_manifest(root, "ingest", name, bytes, "i32", "i32")
    }

    #[test]
    fn explicit_directory_archive_is_accepted() {
        let root = tempfile::tempdir().unwrap();
        let bytes = archive(&[
            dir_bytes("pkg", 0, 0),
            file_bytes("pkg/file", 1000, 1000, b"hi"),
        ]);
        let manifest = place(root.path(), "cache.tar", &bytes);
        check(&manifest).unwrap();
    }

    #[test]
    fn a_symlinked_archive_directory_is_accepted() {
        let root = tempfile::tempdir().unwrap();
        let bytes = archive(&[file_bytes("pkg", 0, 0, b"x")]);
        std::fs::create_dir(root.path().join("real-ingest")).unwrap();
        std::fs::write(root.path().join("real-ingest/cache.tar"), &bytes).unwrap();
        std::os::unix::fs::symlink("real-ingest", root.path().join("ingest")).unwrap();
        let manifest = write_manifest(root.path(), "ingest", "cache.tar", &bytes, "i32", "i32");

        check(&manifest).unwrap();
    }

    #[test]
    fn a_missing_parent_directory_is_rejected_before_any_other_use() {
        let root = tempfile::tempdir().unwrap();
        let bytes = archive(&[file_bytes("pkg/file", 1000, 1000, b"hi")]);
        let manifest = place(root.path(), "cache.tar", &bytes);
        let error = check(&manifest).unwrap_err();
        assert!(
            error.contains("path `pkg/file` is missing explicit directory `pkg`"),
            "{error}"
        );
    }

    #[test]
    fn an_unrepresentable_uid_fits_only_the_configured_u32_type() {
        let root = tempfile::tempdir().unwrap();
        let mut directory = header("pkg/", b'5', 0, 0, 0);
        let mut file = header("pkg/file", b'0', 0, 0, 1);
        file[108] = 0x80;
        file[109..116].copy_from_slice(&[0x00, 0x00, 0x00, 0xff, 0xff, 0xff, 0xff]);
        finish(&mut file);
        finish(&mut directory);
        let bytes = archive(&[directory.to_vec(), {
            let mut member = file.to_vec();
            member.push(b'x');
            member.extend(std::iter::repeat_n(0u8, 511));
            member
        }]);
        std::fs::create_dir(root.path().join("ingest")).unwrap();
        std::fs::write(root.path().join("ingest/cache.tar"), &bytes).unwrap();
        let narrow = write_manifest(root.path(), "ingest", "cache.tar", &bytes, "i32", "i32");
        let error = check(&narrow).unwrap_err();
        assert!(
            error.contains("path `pkg/file` uid 4294967295 does not fit i32"),
            "{error}"
        );
        let wide = write_manifest(root.path(), "ingest", "cache.tar", &bytes, "u32", "u32");
        check(&wide).unwrap();
    }

    #[test]
    fn ownership_boundaries_follow_the_manifest_type() {
        let root = tempfile::tempdir().unwrap();
        let bytes = archive(&[file_bytes("pkg", 65535, 65536, b"x")]);
        let manifest = place(root.path(), "cache.tar", &bytes);
        std::fs::write(
            &manifest,
            std::fs::read_to_string(&manifest)
                .unwrap()
                .replace("\"i32\"", "\"u16\""),
        )
        .unwrap();
        let error = check(&manifest).unwrap_err();
        assert!(error.contains("gid 65536 does not fit u16"), "{error}");
    }

    #[test]
    fn numeric_header_fields_reject_bytes_after_the_terminator() {
        let root = tempfile::tempdir().unwrap();
        let mut file = header("pkg", b'0', 0, 0, 0);
        file[108..116].copy_from_slice(b"1\0junk!!");
        finish(&mut file);
        let bytes = archive(&[file.to_vec()]);
        let manifest = place(root.path(), "cache.tar", &bytes);

        let error = check(&manifest).unwrap_err();
        assert!(error.contains("unrecognized archive"), "{error}");
    }

    #[test]
    fn paths_must_already_be_normalized() {
        for name in ["../pkg", "/pkg", "pkg//file", "./pkg", "pkg/../file"] {
            let root = tempfile::tempdir().unwrap();
            let bytes = archive(&[file_bytes(name, 0, 0, b"x")]);
            let manifest = place(root.path(), "cache.tar", &bytes);
            let error = check(&manifest).unwrap_err();
            assert!(error.contains("is not normalized"), "{name}: {error}");
        }
    }

    #[test]
    fn a_directory_that_appears_after_its_child_is_not_explicit() {
        let root = tempfile::tempdir().unwrap();
        let bytes = archive(&[file_bytes("pkg/file", 0, 0, b"x"), dir_bytes("pkg", 0, 0)]);
        let manifest = place(root.path(), "cache.tar", &bytes);
        let error = check(&manifest).unwrap_err();
        assert!(
            error.contains("missing explicit directory `pkg`"),
            "{error}"
        );
    }

    #[test]
    fn duplicate_normalized_paths_are_rejected() {
        let root = tempfile::tempdir().unwrap();
        let bytes = archive(&[dir_bytes("pkg", 0, 0), dir_bytes("pkg", 0, 0)]);
        let manifest = place(root.path(), "cache.tar", &bytes);
        let error = check(&manifest).unwrap_err();
        assert!(error.contains("duplicate path `pkg`"), "{error}");
    }

    #[test]
    fn a_link_target_must_be_normalized() {
        let root = tempfile::tempdir().unwrap();
        let mut block = header("pkg", b'2', 0, 0, 0);
        block[157..160].copy_from_slice(b"../");
        finish(&mut block);
        let bytes = archive(&[block.to_vec()]);
        let manifest = place(root.path(), "cache.tar", &bytes);
        let error = check(&manifest).unwrap_err();
        assert!(error.contains("target is not normalized"), "{error}");
    }

    #[test]
    fn pax_uid_overrides_the_header_and_global_uid_persists() {
        let root = tempfile::tempdir().unwrap();
        let global = pax_header(b'g', &[("uid", "4294967295"), ("gid", "0")]);
        let file = file_bytes("pkg", 0, 0, b"x");
        let bytes = archive(&[global, file]);
        let manifest = place(root.path(), "cache.tar", &bytes);
        let error = check(&manifest).unwrap_err();
        assert!(error.contains("uid 4294967295 does not fit i32"), "{error}");

        let per_file = pax_header(b'x', &[("uid", "7"), ("gid", "8")]);
        let bytes = archive(&[global_uid_only(), per_file, file_bytes("pkg", 0, 0, b"x")]);
        let manifest = place(root.path(), "cache.tar", &bytes);
        check(&manifest).unwrap();
    }

    fn global_uid_only() -> Vec<u8> {
        pax_header(b'g', &[("uid", "4294967295"), ("gid", "0")])
    }

    fn pax_header(typeflag: u8, pairs: &[(&str, &str)]) -> Vec<u8> {
        let mut payload = Vec::new();
        for (key, value) in pairs {
            let suffix = format!(" {key}={value}\n");
            let mut length = suffix.len();
            loop {
                let text = format!("{length}{suffix}");
                if text.len() == length {
                    payload.extend(text.into_bytes());
                    break;
                }
                length = text.len();
            }
        }
        let mut block = header("./PaxHeaders/item", typeflag, 0, 0, payload.len() as u64);
        block[0..17].copy_from_slice(b"./PaxHeaders/item");
        finish(&mut block);
        let mut bytes = block.to_vec();
        bytes.extend(&payload);
        let pad = (512 - (payload.len() % 512)) % 512;
        bytes.extend(std::iter::repeat_n(0u8, pad));
        bytes
    }

    #[test]
    fn gnu_long_names_and_ustar_prefixes_are_member_paths() {
        let root = tempfile::tempdir().unwrap();
        let long = format!("pkg/{}", "a".repeat(120));
        let mut name_block = header("././@LongLink", b'L', 0, 0, long.len() as u64 + 1);
        finish(&mut name_block);
        let mut bytes = dir_bytes("pkg", 0, 0);
        bytes.extend(name_block);
        bytes.extend(long.as_bytes());
        bytes.push(0);
        let pad = (512 - ((long.len() + 1) % 512)) % 512;
        bytes.extend(std::iter::repeat_n(0u8, pad));
        bytes.extend(file_bytes("short", 0, 0, b"x"));
        bytes.extend(std::iter::repeat_n(0u8, 1024));
        let manifest = place(root.path(), "cache.tar", &bytes);
        check(&manifest).unwrap();

        let mut prefixed = header("file", b'0', 0, 0, 1);
        prefixed[345..348].copy_from_slice(b"pkg");
        finish(&mut prefixed);
        let mut body = dir_bytes("pkg", 0, 0);
        body.extend(prefixed);
        body.push(b'x');
        body.extend(std::iter::repeat_n(0u8, 511));
        body.extend(std::iter::repeat_n(0u8, 1024));
        let manifest = place(root.path(), "cache.tar", &body);
        check(&manifest).unwrap();
    }

    #[test]
    fn a_directory_slash_does_not_count_toward_the_path_limit() {
        fn long_directory(name: &str) -> Vec<u8> {
            let stored = format!("{name}/");
            let mut bytes = header("././@LongLink", b'L', 0, 0, stored.len() as u64 + 1).to_vec();
            bytes.extend(stored.as_bytes());
            bytes.push(0);
            let pad = (512 - ((stored.len() + 1) % 512)) % 512;
            bytes.extend(std::iter::repeat_n(0u8, pad));
            bytes.extend(header("entry/", b'5', 0, 0, 0));
            archive(&[bytes])
        }

        let root = tempfile::tempdir().unwrap();
        let at_limit = long_directory(&"a".repeat(MAX_MEMBER_PATH));
        let manifest = place(root.path(), "cache.tar", &at_limit);
        check(&manifest).unwrap();

        let over = long_directory(&"b".repeat(MAX_MEMBER_PATH + 1));
        let manifest = place(root.path(), "cache.tar", &over);
        let error = check(&manifest).unwrap_err();
        assert!(error.contains("is not normalized"), "{error}");
    }

    #[test]
    fn a_pax_record_length_that_overflows_is_refused() {
        let root = tempfile::tempdir().unwrap();
        let suffix = " k=v\n";
        let mut length = suffix.len();
        let first = loop {
            let text = format!("{length}{suffix}");
            if text.len() == length {
                break text;
            }
            length = text.len();
        };
        let mut payload = first.into_bytes();
        payload.extend(format!("{} x=\n", usize::MAX).into_bytes());
        let mut block = header("./PaxHeaders/item", b'x', 0, 0, payload.len() as u64);
        block[0..17].copy_from_slice(b"./PaxHeaders/item");
        finish(&mut block);
        let mut bytes = block.to_vec();
        bytes.extend(&payload);
        let pad = (512 - (payload.len() % 512)) % 512;
        bytes.extend(std::iter::repeat_n(0u8, pad));
        bytes.extend(std::iter::repeat_n(0u8, 1024));
        let manifest = place(root.path(), "cache.tar", &bytes);
        let error = check(&manifest).unwrap_err();
        assert!(error.contains("unrecognized archive"), "{error}");
    }

    #[test]
    fn empty_multiple_unrecognized_and_wrong_digests_are_refused() {
        let root = tempfile::tempdir().unwrap();
        std::fs::create_dir(root.path().join("ingest")).unwrap();
        let manifest = write_manifest(root.path(), "ingest", "cache.tar", b"", "i32", "i32");
        let error = check(&manifest).unwrap_err();
        assert!(error.contains("directory contains no archive"), "{error}");

        std::fs::write(root.path().join("ingest/cache.tar"), []).unwrap();
        let error = check(&manifest).unwrap_err();
        assert!(error.contains("archive is empty"), "{error}");

        let bytes = archive(&[file_bytes("pkg", 0, 0, b"x")]);
        std::fs::write(root.path().join("ingest/cache.tar"), &bytes).unwrap();
        std::fs::write(root.path().join("ingest/other.tar"), &bytes).unwrap();
        let error = check(&manifest).unwrap_err();
        assert!(error.contains("expected exactly one archive"), "{error}");
        std::fs::remove_file(root.path().join("ingest/other.tar")).unwrap();

        std::fs::rename(
            root.path().join("ingest/cache.tar"),
            root.path().join("ingest/notes.txt"),
        )
        .unwrap();
        let error = check(&manifest).unwrap_err();
        assert!(error.contains("unrecognized archive entry"), "{error}");

        std::fs::rename(
            root.path().join("ingest/notes.txt"),
            root.path().join("ingest/cache.tar"),
        )
        .unwrap();
        let wrong = write_manifest(root.path(), "ingest", "cache.tar", b"other", "i32", "i32");
        let error = check(&wrong).unwrap_err();
        assert!(error.contains("sha256 does not match"), "{error}");

        let empty_tar = vec![0u8; 1024];
        let manifest = place(root.path(), "cache.tar", &empty_tar);
        let error = check(&manifest).unwrap_err();
        assert!(error.contains("archive has no members"), "{error}");

        let manifest = place(root.path(), "cache.tar", b"not a tar");
        let error = check(&manifest).unwrap_err();
        assert!(error.contains("unrecognized archive"), "{error}");
    }

    #[test]
    fn gzip_is_one_member_and_plain_tar_rejects_gzip() {
        let root = tempfile::tempdir().unwrap();
        let bytes = archive(&[file_bytes("pkg", 0, 0, b"x")]);
        let encoder = flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::default());
        let mut encoder = encoder;
        std::io::Write::write_all(&mut encoder, &bytes).unwrap();
        let gzip = encoder.finish().unwrap();
        let manifest = place(root.path(), "cache.tar.gz", &gzip);
        check(&manifest).unwrap();

        let mut extra = gzip.clone();
        extra.push(b'Z');
        let manifest = place(root.path(), "cache.tar.gz", &extra);
        let error = check(&manifest).unwrap_err();
        assert!(error.contains("unrecognized archive"), "{error}");

        std::fs::remove_file(root.path().join("ingest/cache.tar.gz")).unwrap();
        let manifest = place(root.path(), "cache.tar", &gzip);
        let error = check(&manifest).unwrap_err();
        assert!(error.contains("unrecognized archive"), "{error}");
    }

    #[test]
    fn gnu_tar_ustar_pax_and_gzip_archives_follow_the_same_rules() {
        let root = tempfile::tempdir().unwrap();
        let tree = root.path().join("tree");
        std::fs::create_dir_all(tree.join("pkg")).unwrap();
        std::fs::write(tree.join("pkg/file"), b"hi\n").unwrap();

        let flat = root.path().join("flat.tar");
        std::fs::write(root.path().join("list"), "pkg/file\n").unwrap();
        let status = Command::new("tar")
            .args([
                "--format=ustar",
                "-cf",
                flat.to_str().unwrap(),
                "-C",
                tree.to_str().unwrap(),
                "--no-recursion",
                "--files-from",
                root.path().join("list").to_str().unwrap(),
            ])
            .status()
            .unwrap();
        assert!(status.success());
        let bytes = std::fs::read(&flat).unwrap();
        std::fs::create_dir(root.path().join("one")).unwrap();
        std::fs::write(root.path().join("one/flat.tar"), &bytes).unwrap();
        let manifest = write_manifest(root.path(), "one", "flat.tar", &bytes, "i32", "i32");
        let error = check(&manifest).unwrap_err();
        assert!(
            error.contains("missing explicit directory `pkg`"),
            "{error}"
        );

        let plain = root.path().join("plain.tar");
        let status = Command::new("tar")
            .args(["--format=ustar", "-cf"])
            .arg(&plain)
            .arg("-C")
            .arg(&tree)
            .arg("pkg")
            .status()
            .unwrap();
        assert!(status.success());
        let bytes = std::fs::read(&plain).unwrap();
        std::fs::create_dir(root.path().join("plain")).unwrap();
        std::fs::write(root.path().join("plain/cache.tar"), &bytes).unwrap();
        let manifest = write_manifest(root.path(), "plain", "cache.tar", &bytes, "i32", "i32");
        check(&manifest).unwrap();

        let gzip_path = root.path().join("cache.tar.gz");
        let status = Command::new("tar")
            .args(["--format=ustar", "-czf"])
            .arg(&gzip_path)
            .arg("-C")
            .arg(&tree)
            .arg("pkg")
            .status()
            .unwrap();
        assert!(status.success());
        let bytes = std::fs::read(&gzip_path).unwrap();
        std::fs::create_dir(root.path().join("gz")).unwrap();
        std::fs::write(root.path().join("gz/cache.tar.gz"), &bytes).unwrap();
        let manifest = write_manifest(root.path(), "gz", "cache.tar.gz", &bytes, "u32", "u32");
        check(&manifest).unwrap();

        let long = "b".repeat(120);
        std::fs::write(tree.join("pkg").join(&long), b"y").unwrap();
        for format in ["posix", "gnu"] {
            let archive_path = root.path().join(format!("{format}.tar"));
            let status = Command::new("tar")
                .args(["--format", format, "-cf"])
                .arg(&archive_path)
                .arg("-C")
                .arg(&tree)
                .arg("pkg")
                .status()
                .unwrap();
            assert!(status.success(), "{format}");
            let bytes = std::fs::read(&archive_path).unwrap();
            std::fs::create_dir_all(root.path().join(format)).unwrap();
            std::fs::write(root.path().join(format).join("cache.tar"), &bytes).unwrap();
            let manifest = write_manifest(root.path(), format, "cache.tar", &bytes, "i32", "i32");
            check(&manifest).unwrap_or_else(|error| panic!("{format}: {error}"));
        }
    }

    #[test]
    fn manifest_shape_errors_name_the_field() {
        let root = tempfile::tempdir().unwrap();
        let path = root.path().join("manifest.json");
        std::fs::write(&path, "{\"directory\":\"ingest\"}").unwrap();
        let error = check(&path).unwrap_err();
        assert!(error.contains("manifest is invalid"), "{error}");
        std::fs::write(
            &path,
            "{\"directory\":\"ingest\",\"name\":\"cache.tar\",\"sha256\":\"abcd\",\"uid_type\":\"i32\",\"gid_type\":\"i32\",\"extra\":1}",
        )
        .unwrap();
        let error = check(&path).unwrap_err();
        assert!(error.contains("unknown field"), "{error}");
        std::fs::write(
            &path,
            "{\"directory\":\"ingest\",\"name\":\"cache.zip\",\"sha256\":\"aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa\",\"uid_type\":\"i32\",\"gid_type\":\"i32\"}",
        )
        .unwrap();
        let error = check(&path).unwrap_err();
        assert!(error.contains(".tar"), "{error}");
    }
}
