//! Small helpers.

use crate::exit::Fail;
use std::ffi::{OsStr, OsString};
use std::fs::OpenOptions;
use std::io::{Read, Write};
use std::os::unix::ffi::{OsStrExt, OsStringExt};
use std::os::unix::fs::{MetadataExt, OpenOptionsExt, PermissionsExt};
use std::path::{Path, PathBuf};

/// Open a regular input file without waiting on a special file.
pub fn open_regular_file(path: &Path) -> std::io::Result<std::fs::File> {
    let file = OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_NONBLOCK)
        .open(path)?;
    let meta = file.metadata()?;
    if meta.is_dir() {
        return Err(std::io::Error::from_raw_os_error(libc::EISDIR));
    }
    if !meta.is_file() {
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidInput,
            "not a regular file",
        ));
    }
    Ok(file)
}

/// Read a regular input file as UTF-8 without waiting on a special file.
pub fn read_text_file(path: &Path) -> std::io::Result<String> {
    let mut file = open_regular_file(path)?;
    let mut text = String::new();
    file.read_to_string(&mut text)?;
    Ok(text)
}

/// Write all bytes to standard output, treating a closed pipeline consumer as
/// successful completion.
pub fn write_stdout(bytes: &[u8]) -> std::io::Result<()> {
    match std::io::stdout().lock().write_all(bytes) {
        Err(e) if e.kind() == std::io::ErrorKind::BrokenPipe => Ok(()),
        result => result,
    }
}

/// Write one line to standard error on a best-effort basis. A closed or full
/// stderr is ignored, so a diagnostic never turns into a panic or changes the
/// exit status the caller already chose.
pub fn write_stderr_line(message: &str) {
    let mut stderr = std::io::stderr().lock();
    let _ = writeln!(stderr, "{message}");
}

/// Write bytes to standard output or format a user-facing error message.
pub fn write_stdout_checked(bytes: &[u8]) -> Result<(), String> {
    write_stdout(bytes).map_err(|e| format!("cannot write stdout: {e}"))
}

/// A non-empty Unicode value from the process environment.
pub fn env_value(name: &str) -> Option<String> {
    std::env::var(name).ok().filter(|value| !value.is_empty())
}

/// Why a duration in seconds is invalid.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DurationError {
    /// Not finite or negative.
    Invalid,
    /// Larger than a consumer's explicit upper bound.
    TooLarge,
}

/// Validate a duration value in seconds, with an optional exclusive upper
/// bound.
pub fn checked_duration_seconds(
    seconds: f64,
    max_seconds: Option<f64>,
) -> Result<f64, DurationError> {
    if !seconds.is_finite() || seconds < 0.0 {
        return Err(DurationError::Invalid);
    }
    if max_seconds.is_some_and(|max| seconds >= max) {
        return Err(DurationError::TooLarge);
    }
    Ok(seconds)
}

/// Process arguments after the executable path, preserving byte paths.
pub fn os_args() -> Vec<OsString> {
    std::env::args_os().skip(1).collect()
}

/// Convert one argument that must be text to UTF-8.
pub fn os_to_string(arg: OsString) -> Result<String, String> {
    arg.into_string()
        .map_err(|arg| format!("argument is not valid UTF-8: `{}`", arg.to_string_lossy()))
}

/// Split `--flag=value` even when `value` is not UTF-8.
pub fn split_os_flag_value(arg: &OsStr) -> Option<(String, OsString)> {
    let bytes = arg.as_bytes();
    let at = bytes.iter().position(|byte| *byte == b'=')?;
    if at < 2 || !bytes.starts_with(b"--") {
        return None;
    }
    let flag = std::str::from_utf8(&bytes[..at]).ok()?.to_string();
    Some((flag, OsString::from_vec(bytes[at + 1..].to_vec())))
}

/// The next non-empty command-line value as bytes, without consuming another
/// long option as that value.
pub fn separate_os_value(
    words: &mut impl Iterator<Item = OsString>,
    missing: &str,
) -> Result<OsString, String> {
    words
        .next()
        .filter(|value| {
            !value.as_os_str().is_empty() && !value.as_os_str().as_bytes().starts_with(b"--")
        })
        .ok_or_else(|| missing.to_string())
}

/// The next non-empty command-line value, without consuming another long
/// option as that value.
pub fn separate_value(
    words: &mut impl Iterator<Item = String>,
    missing: &str,
) -> Result<String, String> {
    words
        .next()
        .filter(|value| !value.is_empty() && !value.starts_with("--"))
        .ok_or_else(|| missing.to_string())
}

/// Whether two paths name the same file, including through a symbolic or
/// hard link. A path that does not exist is resolved through its parent.
pub fn paths_alias(path_a: &Path, path_b: &Path) -> bool {
    if resolved_path(path_a) == resolved_path(path_b) {
        return true;
    }
    match (std::fs::metadata(path_a), std::fs::metadata(path_b)) {
        (Ok(a), Ok(b)) => a.dev() == b.dev() && a.ino() == b.ino(),
        _ => false,
    }
}

/// Resolve a path as far as the filesystem permits.
pub fn resolved_path(path: &Path) -> PathBuf {
    if let Ok(path) = std::fs::canonicalize(path) {
        return path;
    }
    let parent = path.parent().unwrap_or_else(|| Path::new("."));
    let parent = if parent.as_os_str().is_empty() {
        Path::new(".")
    } else {
        parent
    };
    match (
        std::fs::canonicalize(parent),
        path.file_name().map(ToOwned::to_owned),
    ) {
        (Ok(parent), Some(name)) => parent.join(name),
        _ => std::path::absolute(path).unwrap_or_else(|_| path.to_path_buf()),
    }
}

/// Refuse an output path that aliases any input path.
///
/// `-` means standard output and is not a file destination.
pub fn refuse_output_alias(
    output_option: &str,
    output: Option<&Path>,
    inputs: &[(&str, Option<&Path>)],
) -> Result<(), String> {
    let Some(output) = output.filter(|path| *path != Path::new("-")) else {
        return Ok(());
    };
    for (input_option, input) in inputs {
        let Some(input) = input else {
            continue;
        };
        if paths_alias(input, output) {
            return Err(format!(
                "{input_option} and {output_option} must name different files, both resolve to {}",
                resolved_path(input).display()
            ));
        }
    }
    Ok(())
}

/// Atomically replace a non-symlink `path` through a fully written temporary
/// sibling.
///
/// An existing regular file's permission bits are applied to the temporary
/// file before the rename. Ownership follows the account creating the
/// temporary file.
pub fn write_atomic(path: &Path, bytes: &[u8]) -> std::io::Result<()> {
    if path.is_symlink() {
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidInput,
            "output path is a symbolic link",
        ));
    }
    let destination = path.to_path_buf();
    let parent = destination.parent().unwrap_or_else(|| Path::new("."));
    let existing_mode = match std::fs::metadata(&destination) {
        Ok(metadata) if metadata.is_file() => Some(metadata.permissions().mode() & 0o7777),
        Ok(_) => None,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => None,
        Err(error) => return Err(error),
    };
    let name = destination
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or("output");
    for attempt in 0..1000u32 {
        let temporary = parent.join(format!(".{name}.{}.{}.tmp", std::process::id(), attempt));
        let mut file = match OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&temporary)
        {
            Ok(file) => file,
            Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => continue,
            Err(error) => return Err(error),
        };
        if let Some(mode) = existing_mode {
            if let Err(error) =
                std::fs::set_permissions(&temporary, std::fs::Permissions::from_mode(mode))
            {
                drop(file);
                let _ = std::fs::remove_file(&temporary);
                return Err(error);
            }
        }
        let written = file.write_all(bytes).and_then(|()| file.sync_all());
        drop(file);
        if let Err(error) = written {
            let _ = std::fs::remove_file(&temporary);
            return Err(error);
        }
        if let Err(error) = std::fs::rename(&temporary, &destination) {
            let _ = std::fs::remove_file(&temporary);
            return Err(error);
        }
        return Ok(());
    }
    Err(std::io::Error::new(
        std::io::ErrorKind::AlreadyExists,
        "cannot allocate a temporary output file",
    ))
}

/// Split a string on whitespace, honouring single and double quotes.
pub fn split_args(s: &str) -> Result<Vec<String>, Fail> {
    let mut out = Vec::new();
    let mut cur = String::new();
    let mut quote: Option<char> = None;
    let mut have = false;
    for c in s.chars() {
        match quote {
            Some(q) if c == q => quote = None,
            Some(_) => cur.push(c),
            None if c == '\'' || c == '"' => {
                quote = Some(c);
                have = true;
            }
            None if c.is_whitespace() => {
                if have || !cur.is_empty() {
                    out.push(std::mem::take(&mut cur));
                    have = false;
                }
            }
            None => cur.push(c),
        }
    }
    if quote.is_some() {
        return Err(Fail::usage(format!("unterminated quote in `{s}`")));
    }
    if have || !cur.is_empty() {
        out.push(cur);
    }
    Ok(out)
}

/// Quote `a` for a shell: as it is when it holds only characters a shell
/// leaves alone, else in single quotes. A single quote in `a` is written
/// `'"'"'`, which a shell and [`split_args`] both read back.
///
/// A build list line is not a shell command. [`list_word`] writes one word
/// of a build list, and leaves a parenthesis or a star bare. `stack shard`
/// splits on white space and keeps the quotes a shell would add.
pub fn quote(a: &str) -> String {
    if !a.is_empty()
        && a.chars()
            .all(|c| c.is_ascii_alphanumeric() || "-_./=:,@%+".contains(c))
    {
        a.to_string()
    } else {
        format!("'{}'", a.replace('\'', "'\"'\"'"))
    }
}

/// One word of a build list line, which [`parse_list`] reads back as `word`.
///
/// The word stays bare when the reader already keeps it as one word, including
/// a parenthesis or a star that a shell would quote. A word that holds a space
/// or a quote is quoted so the reader still returns it: double quotes when it
/// holds an apostrophe, single quotes otherwise, and the two side by side when
/// it holds both. A line break cannot be one word of one line.
pub fn list_word(word: &str) -> Result<String, Fail> {
    if word.contains(['\n', '\r']) {
        return Err(Fail::usage(
            "a build list word holds a line break, which one line cannot hold",
        ));
    }
    if word.is_empty() {
        return Ok("''".into());
    }
    if is_bare_list_word(word) {
        return Ok(word.to_string());
    }
    if !word.contains('\'') {
        return Ok(format!("'{word}'"));
    }
    if !word.contains('"') {
        return Ok(format!("\"{word}\""));
    }
    let mut out = String::new();
    let mut chars = word.chars().peekable();
    while chars.peek().is_some() {
        let quote = if chars.peek() == Some(&'\'') {
            '"'
        } else {
            '\''
        };
        let mut run = String::new();
        while let Some(&c) = chars.peek() {
            if c == quote {
                break;
            }
            run.push(c);
            chars.next();
        }
        out.push(quote);
        out.push_str(&run);
        out.push(quote);
    }
    Ok(out)
}

/// Whether `word` is already one word to [`parse_list`], and not a comment
/// when it is the first word of a line.
fn is_bare_list_word(word: &str) -> bool {
    !word.starts_with('#')
        && !word
            .chars()
            .any(|c| c.is_whitespace() || c == '\'' || c == '"')
}

/// Whether `s` is a log stamp: a UTC time as `YYYYMMDD_HHMMSS`, the name
/// `eb-keep-logs rotate` ages as a run directory.
pub fn is_log_stamp(s: &str) -> bool {
    let b = s.as_bytes();
    b.len() == 15
        && b[8] == b'_'
        && b[..8].iter().chain(&b[9..]).all(u8::is_ascii_digit)
        && chrono::NaiveDateTime::parse_from_str(s, "%Y%m%d_%H%M%S").is_ok()
}

/// Parse dotted numbers (`1.2.3`); a leading `v` is accepted.
pub fn parse_version(s: &str) -> Option<Vec<u64>> {
    let s = s.trim().trim_start_matches('v');
    if s.is_empty() {
        return None;
    }
    s.split('.').map(|p| p.parse::<u64>().ok()).collect()
}

/// Compare two dotted versions; missing components count as zero.
pub fn cmp_version(a: &[u64], b: &[u64]) -> std::cmp::Ordering {
    let n = a.len().max(b.len());
    for i in 0..n {
        let x = a.get(i).copied().unwrap_or(0);
        let y = b.get(i).copied().unwrap_or(0);
        if x != y {
            return x.cmp(&y);
        }
    }
    std::cmp::Ordering::Equal
}

/// First dotted-number token of a `--version` output.
pub fn find_version(output: &str) -> Option<Vec<u64>> {
    for tok in output.split(|c: char| c.is_whitespace() || c == ',' || c == '(' || c == ')') {
        let t = tok.trim_start_matches('v');
        if t.chars().next().is_some_and(|c| c.is_ascii_digit()) {
            let core: String = t
                .chars()
                .take_while(|c| c.is_ascii_digit() || *c == '.')
                .collect();
            if let Some(v) = parse_version(core.trim_end_matches('.')) {
                return Some(v);
            }
        }
    }
    None
}

/// One line of a build list.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ListLine {
    /// The line as written, trimmed.
    pub raw: String,
    /// Easyconfig file name or path.
    pub easyconfig: String,
    /// Remaining easybuild arguments.
    pub args: Vec<String>,
}

/// Read build list lines: blank lines and `#` comments are skipped.
pub fn parse_list(text: &str) -> Result<Vec<ListLine>, Fail> {
    let mut out = Vec::new();
    let text = text.strip_prefix('\u{feff}').unwrap_or(text);
    for l in text.lines() {
        let t = l.trim();
        if t.is_empty() || t.starts_with('#') {
            continue;
        }
        let mut parts = split_args(t)?;
        let easyconfig = parts.remove(0);
        out.push(ListLine {
            raw: t.to_string(),
            easyconfig,
            args: parts,
        });
    }
    Ok(out)
}

/// Package name and version of an easyconfig file name `Name-version[-toolchain].eb`.
pub fn package_of(easyconfig: &str) -> (String, Option<String>) {
    let base = easyconfig.rsplit('/').next().unwrap_or(easyconfig);
    let stem = base.strip_suffix(".eb").unwrap_or(base);
    let parts: Vec<&str> = stem.split('-').collect();
    let mut name = Vec::new();
    let mut i = 0;
    while i < parts.len() {
        if i > 0 && parts[i].chars().next().is_some_and(|c| c.is_ascii_digit()) {
            break;
        }
        name.push(parts[i]);
        i += 1;
    }
    (name.join("-"), parts.get(i).map(|s| s.to_string()))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::ffi::CString;
    use std::os::unix::ffi::OsStrExt;

    #[test]
    fn regular_text_reader_rejects_hostile_inputs_promptly() {
        let dir = tempfile::tempdir().unwrap();

        let fifo = dir.path().join("input.fifo");
        let raw = CString::new(fifo.as_os_str().as_bytes()).unwrap();
        assert_eq!(unsafe { libc::mkfifo(raw.as_ptr(), 0o600) }, 0);
        let (send, receive) = std::sync::mpsc::channel();
        let reader = std::thread::spawn(move || {
            send.send(read_text_file(&fifo)).unwrap();
        });
        let error = receive
            .recv_timeout(std::time::Duration::from_secs(5))
            .expect("FIFO reader blocked")
            .unwrap_err();
        reader.join().unwrap();
        assert_eq!(error.kind(), std::io::ErrorKind::InvalidInput);

        let error = read_text_file(dir.path()).unwrap_err();
        assert_eq!(error.kind(), std::io::ErrorKind::IsADirectory);

        let dangling = dir.path().join("dangling");
        std::os::unix::fs::symlink("gone", &dangling).unwrap();
        let error = read_text_file(&dangling).unwrap_err();
        assert_eq!(error.kind(), std::io::ErrorKind::NotFound);

        let invalid = dir.path().join("invalid");
        std::fs::write(&invalid, [0xff]).unwrap();
        let error = read_text_file(&invalid).unwrap_err();
        assert_eq!(error.kind(), std::io::ErrorKind::InvalidData);

        let bom = dir.path().join("bom");
        std::fs::write(&bom, "\u{feff}text").unwrap();
        assert_eq!(read_text_file(&bom).unwrap(), "\u{feff}text");
    }

    #[test]
    fn atomic_write_replaces_only_after_the_new_file_is_complete() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("result.json");
        std::fs::write(&path, b"old bytes\n").unwrap();
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o640)).unwrap();
        let old_inode = std::fs::metadata(&path).unwrap().ino();

        write_atomic(&path, b"complete new bytes\n").unwrap();

        let metadata = std::fs::metadata(&path).unwrap();
        assert_ne!(metadata.ino(), old_inode);
        assert_eq!(metadata.permissions().mode() & 0o777, 0o640);
        assert_eq!(std::fs::read(&path).unwrap(), b"complete new bytes\n");
        assert_eq!(std::fs::read_dir(dir.path()).unwrap().count(), 1);
    }

    #[test]
    fn atomic_write_failure_preserves_the_previous_file() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("result.json");
        std::fs::write(&path, b"complete old bytes\n").unwrap();
        for attempt in 0..1000u32 {
            std::fs::write(
                dir.path().join(format!(
                    ".result.json.{}.{}.tmp",
                    std::process::id(),
                    attempt
                )),
                b"occupied\n",
            )
            .unwrap();
        }

        let error = write_atomic(&path, b"new bytes\n").unwrap_err();

        assert_eq!(error.kind(), std::io::ErrorKind::AlreadyExists);
        assert_eq!(std::fs::read(&path).unwrap(), b"complete old bytes\n");
    }

    #[test]
    fn atomic_write_refuses_output_symlinks() {
        let dir = tempfile::tempdir().unwrap();
        let target = dir.path().join("result.json");
        let link = dir.path().join("latest.json");
        std::fs::write(&target, b"old bytes\n").unwrap();
        std::os::unix::fs::symlink("result.json", &link).unwrap();

        let error = write_atomic(&link, b"new bytes\n").unwrap_err();
        assert_eq!(error.kind(), std::io::ErrorKind::InvalidInput);
        assert_eq!(std::fs::read(&target).unwrap(), b"old bytes\n");
        assert!(link.is_symlink());

        let missing = dir.path().join("missing.json");
        let dangling = dir.path().join("dangling.json");
        std::os::unix::fs::symlink("missing.json", &dangling).unwrap();
        let error = write_atomic(&dangling, b"new bytes\n").unwrap_err();
        assert_eq!(error.kind(), std::io::ErrorKind::InvalidInput);
        assert!(!missing.exists());
        assert!(dangling.is_symlink());
    }

    #[test]
    fn split_quotes() {
        assert_eq!(
            split_args("-p batch --comment 'a b' \"\"").unwrap(),
            vec!["-p", "batch", "--comment", "a b", ""]
        );
        assert!(split_args("'open").is_err());
    }

    #[test]
    fn checked_duration_seconds_rejects_invalid_and_too_large_values() {
        assert_eq!(
            checked_duration_seconds(f64::NAN, None),
            Err(DurationError::Invalid)
        );
        assert_eq!(
            checked_duration_seconds(-0.1, Some(10.0)),
            Err(DurationError::Invalid)
        );
        assert_eq!(
            checked_duration_seconds(11.0, Some(10.0)),
            Err(DurationError::TooLarge)
        );
        assert_eq!(
            checked_duration_seconds(10.0, Some(10.0)),
            Err(DurationError::TooLarge)
        );
        assert_eq!(checked_duration_seconds(1.25, None), Ok(1.25));
        assert_eq!(
            checked_duration_seconds(18446744073709551616.0, Some(18446744073709551616.0)),
            Err(DurationError::TooLarge)
        );
    }

    #[test]
    fn a_quoted_word_reads_back_as_itself() {
        for a in [
            "--try-amend=configopts=-DA=OFF -DB=OFF",
            "it's",
            "a\"b'c",
            "",
            "--rebuild",
        ] {
            assert_eq!(split_args(&quote(a)).unwrap(), [a], "{}", quote(a));
            let o = std::process::Command::new("sh")
                .arg("-c")
                .arg(format!("printf %s {}", quote(a)))
                .output()
                .unwrap();
            assert_eq!(String::from_utf8_lossy(&o.stdout), a, "{}", quote(a));
        }
        assert_eq!(quote("--rebuild"), "--rebuild");
    }

    #[test]
    fn a_shell_quote_is_not_a_build_list_word() {
        // quote() is the shell quoter. An apostrophe is '"'"', which a shell
        // and split_args both read back, not the backslash form '\''.
        let shell = quote("it's");
        assert!(shell.contains("'\"'\"'"), "{shell}");
        assert!(!shell.contains("'\\''"), "{shell}");
        assert_eq!(split_args(&shell).unwrap(), ["it's"]);

        // A parenthesis is one bare word of a build list. The shell form
        // quotes it, so a reader that splits on white space and keeps the
        // quotes does not see the option.
        let hooks = "--hooks=/tmp/a(b).py";
        let quoted = quote(hooks);
        assert_ne!(quoted, hooks, "{quoted}");
        assert_ne!(quoted.split_whitespace().collect::<Vec<_>>(), [hooks]);
        assert_eq!(list_word(hooks).unwrap(), hooks);
        assert_eq!(
            list_word("--filter-deps=foo(bar)").unwrap(),
            "--filter-deps=foo(bar)"
        );
        assert_eq!(list_word("--opt=a*b").unwrap(), "--opt=a*b");

        for word in [
            "it's",
            "a b",
            "say \"hi\"",
            "it's \"a\"",
            "",
            "#hash",
            "--hooks=/tmp/a(b).py",
            "--opt=a*b",
            "--filter-deps=foo(bar)",
            "  ",
        ] {
            let listed = list_word(word).unwrap();
            let lines = parse_list(&format!("x.eb {listed}")).unwrap();
            assert_eq!(
                lines[0].args,
                vec![word.to_string()],
                "{word:?} encoded as {listed}"
            );
        }
        assert_eq!(list_word("it's").unwrap(), "\"it's\"");
        assert_eq!(list_word("a b").unwrap(), "'a b'");
        assert!(list_word("a\nb").is_err());
        assert!(list_word("a\rb").is_err());
    }

    #[test]
    fn versions() {
        assert_eq!(find_version("eb-stack 0.7.4 (abc)"), Some(vec![0, 7, 4]));
        assert_eq!(find_version("tool v1.2"), Some(vec![1, 2]));
        assert_eq!(cmp_version(&[1, 2], &[1, 2, 0]), std::cmp::Ordering::Equal);
        assert_eq!(cmp_version(&[0, 7, 4], &[0, 10]), std::cmp::Ordering::Less);
    }

    #[test]
    fn package_names() {
        assert_eq!(
            package_of("GROMACS-2025.2-foss-2025a.eb"),
            ("GROMACS".into(), Some("2025.2".into()))
        );
        assert_eq!(
            package_of("/x/y/R-bundle-Bioconductor-3.20-foss-2025a.eb"),
            ("R-bundle-Bioconductor".into(), Some("3.20".into()))
        );
    }

    #[test]
    fn list_lines() {
        let l = parse_list("# c\n\nFoo-1.0-foss-2025a.eb --rebuild\n").unwrap();
        assert_eq!(l.len(), 1);
        assert_eq!(l[0].args, vec!["--rebuild"]);
    }

    #[test]
    fn parse_list_strips_one_initial_utf8_bom() {
        let lines = parse_list("\u{feff}Foo-1.0.eb --rebuild\nBar-2.0.eb\n").unwrap();
        assert_eq!(lines[0].easyconfig, "Foo-1.0.eb");
        assert_eq!(lines[0].raw, "Foo-1.0.eb --rebuild");
        assert_eq!(lines[1].easyconfig, "Bar-2.0.eb");
    }

    #[test]
    fn log_stamps() {
        for ok in ["20261116_024713", "20240229_235959", "19700101_000000"] {
            assert!(is_log_stamp(ok), "{ok}");
        }
        for bad in [
            "",
            "2026-11-16",
            "20261116",
            "20261116_02471",
            "20261116-024713",
            "20261116_0247130",
            "20261301_000000",
            "20250229_000000",
            "20261116_240000",
            "20261116_006000",
            "+2026116_024713",
            " 2026116_024713",
            "../20261116_0247",
        ] {
            assert!(!is_log_stamp(bad), "{bad:?}");
        }
    }
}
