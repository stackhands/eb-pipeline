//! What eb-pipeline reads of EESSI for `[watch]`: the version a target's
//! module environment loads, the CVMFS revision of its repository, and an
//! identity for each module in its module trees.

use base64::Engine;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

/// EESSI as a target's module environment loads it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Probe {
    /// `EESSI_VERSION`, such as `2023.06`.
    pub version: String,
    /// `EESSI_SOFTWARE_SUBDIR`, the CPU subdirectory, such as
    /// `x86_64/amd/zen4`.
    pub software_subdir: String,
    /// Where the revision is read; none for a tree that is not CVMFS.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub repository: Option<String>,
    /// The EESSI module trees, in order.
    pub modules_dirs: Vec<String>,
    /// The loaded module names, when Lmod exported them.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub loaded_modules: Option<String>,
    /// SHA-256 of Lmod's decoded module table, when it exported one.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub module_table_sha256: Option<String>,
}

impl Probe {
    /// From the environment the module wrapper ends with; `repository` and
    /// `modules_dirs` from the site win over the variables.
    pub fn from_env(
        env: &BTreeMap<String, String>,
        repository: Option<&str>,
        modules_dirs: &[String],
    ) -> Result<Probe, String> {
        let get = |k: &str| env.get(k).map(|v| v.trim()).filter(|v| !v.is_empty());
        let version = get("EESSI_VERSION").ok_or("EESSI_VERSION is unset")?;
        let subdir = get("EESSI_SOFTWARE_SUBDIR").ok_or("EESSI_SOFTWARE_SUBDIR is unset")?;
        let modules_dirs: Vec<String> = if modules_dirs.is_empty() {
            ["EESSI_MODULEPATH", "EESSI_MODULEPATH_ACCEL"]
                .into_iter()
                .filter_map(get)
                .map(str::to_string)
                .collect()
        } else {
            modules_dirs.to_vec()
        };
        if modules_dirs.is_empty() {
            return Err("EESSI_MODULEPATH is unset".into());
        }
        Ok(Probe {
            version: version.into(),
            software_subdir: subdir.into(),
            repository: repository
                .or_else(|| get("EESSI_CVMFS_REPO"))
                .map(str::to_string),
            modules_dirs,
            loaded_modules: get("LOADEDMODULES").map(str::to_string),
            module_table_sha256: module_table_sha256(env),
        })
    }
}

/// Lmod splits the base64 text of its module table into `_ModuleTable%03d_`
/// pieces counted by `_ModuleTable_Sz_`, using the URL-safe alphabet.
fn module_table_sha256(env: &BTreeMap<String, String>) -> Option<String> {
    let encoded = match env.get("_ModuleTable_Sz_") {
        Some(size) => {
            let size: usize = size.parse().ok()?;
            let mut joined = String::new();
            for index in 1..=size {
                joined.push_str(env.get(&format!("_ModuleTable{index:03}_"))?);
            }
            joined
        }
        None => env.get("_ModuleTable_")?.to_string(),
    };
    let bytes = base64::engine::general_purpose::URL_SAFE
        .decode(encoded)
        .ok()?;
    Some(format!("{:x}", Sha256::digest(bytes)))
}

/// The variables `env -0` printed.
pub fn parse_env0(bytes: &[u8]) -> BTreeMap<String, String> {
    bytes
        .split(|b| *b == 0)
        .filter_map(|kv| {
            let kv = String::from_utf8_lossy(kv);
            let (k, v) = kv.split_once('=')?;
            Some((k.to_string(), v.to_string()))
        })
        .collect()
}

/// `<repository>/versions`, where each generation is a `YYYY.MM` directory.
pub fn versions_dir(repository: &str) -> PathBuf {
    Path::new(repository).join("versions")
}

/// `YYYY.MM` with a month from 01 to 12, as an EESSI generation directory is named.
pub fn generation_name(name: &str) -> Option<(u32, u32)> {
    let (year, month) = name.split_once('.')?;
    if year.len() != 4 || month.len() != 2 || name.split('.').count() != 2 {
        return None;
    }
    let year: u32 = year.parse().ok()?;
    let month: u32 = month.parse().ok()?;
    if (1..=12).contains(&month) {
        Some((year, month))
    } else {
        None
    }
}

/// `EESSI/YYYY.MM`, the way `[modules] init` names a generation.
/// A name [`generation_name`] rejects is not a generation.
pub fn module_generation(module: &str) -> Option<&str> {
    let rest = module.strip_prefix("EESSI/")?;
    generation_name(rest)?;
    Some(rest)
}

/// The last generation in a module list. A later `EESSI/YYYY.MM` wins, and
/// a name that is not a generation is skipped.
pub fn last_init_generation(modules: &[String]) -> Option<&str> {
    modules
        .iter()
        .rev()
        .find_map(|module| module_generation(module))
}

/// The newest `YYYY.MM` directory under `versions` that is newer than `loaded`.
///
/// A name that is not `YYYY.MM`, and a path that is not a directory, are
/// ignored. A missing `versions` directory is no newer generation. `loaded`
/// that is not `YYYY.MM` is not compared.
pub fn newer_generation(versions: &Path, loaded: &str) -> std::io::Result<Option<String>> {
    let Some(loaded_key) = generation_name(loaded) else {
        return Ok(None);
    };
    let entries = match std::fs::read_dir(versions) {
        Ok(entries) => entries,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(e) => return Err(e),
    };
    let mut best: Option<(u32, u32, String)> = None;
    for entry in entries {
        let entry = entry?;
        let name = entry.file_name().to_string_lossy().into_owned();
        let Some(key) = generation_name(&name) else {
            continue;
        };
        if !entry.path().is_dir() || key <= loaded_key {
            continue;
        }
        if best.as_ref().is_none_or(|b| key > (b.0, b.1)) {
            best = Some((key.0, key.1, name));
        }
    }
    Ok(best.map(|(_, _, name)| name))
}

/// The CVMFS revision of the repository mounted at `repo`: its
/// `user.revision` attribute, which a file system other than CVMFS lacks.
pub fn revision(repo: &Path) -> std::io::Result<Option<u64>> {
    use std::os::unix::ffi::OsStrExt;
    std::fs::metadata(repo)?;
    let path = std::ffi::CString::new(repo.as_os_str().as_bytes())
        .map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidInput, e))?;
    let mut buf = [0u8; 32];
    // SAFETY: both names are NUL-terminated and the buffer outlives the call.
    let n = unsafe {
        libc::getxattr(
            path.as_ptr(),
            c"user.revision".as_ptr(),
            buf.as_mut_ptr().cast(),
            buf.len(),
        )
    };
    if n < 0 {
        let e = std::io::Error::last_os_error();
        return match e.raw_os_error() {
            Some(libc::ENODATA) | Some(libc::ENOTSUP) => Ok(None),
            _ => Err(e),
        };
    }
    let text = String::from_utf8_lossy(&buf[..n as usize]);
    Ok(text.trim_end_matches('\0').trim().parse().ok())
}

/// One module of a module tree.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Module {
    /// `Name/version`.
    pub name: String,
    /// The install directory the module file sets as `root`.
    pub root: Option<String>,
    /// The easyconfig EasyBuild copied into `<root>/easybuild`.
    pub easyconfig: Option<String>,
    /// Changes with the module file and with the listing of its install's
    /// `easybuild` directory, where a rebuild in place leaves a new log.
    pub identity: String,
}

/// Every module under `dirs` by name; the first directory with a name
/// wins, as on a MODULEPATH.
pub fn scan(dirs: &[String]) -> BTreeMap<String, Module> {
    let mut out = BTreeMap::new();
    for d in dirs {
        let Ok(names) = std::fs::read_dir(d) else {
            continue;
        };
        for n in names.flatten() {
            let name = n.file_name().to_string_lossy().into_owned();
            if name.starts_with('.') {
                continue;
            }
            let Ok(files) = std::fs::read_dir(n.path()) else {
                continue;
            };
            for f in files.flatten() {
                let file = f.file_name().to_string_lossy().into_owned();
                if file.starts_with('.') {
                    continue;
                }
                let full = format!("{name}/{}", file.strip_suffix(".lua").unwrap_or(&file));
                if let std::collections::btree_map::Entry::Vacant(slot) = out.entry(full) {
                    if let Some(m) = read(&f.path(), slot.key()) {
                        slot.insert(m);
                    }
                }
            }
        }
    }
    out
}

/// The module `name` as `scan` of `dirs` finds it.
pub fn find(dirs: &[String], name: &str) -> Option<Module> {
    let (n, v) = name.split_once('/')?;
    dirs.iter().find_map(|d| {
        [format!("{v}.lua"), v.to_string()]
            .iter()
            .find_map(|f| read(&Path::new(d).join(n).join(f), name))
    })
}

/// A Lua or Tcl module file; any other file is none.
fn read(file: &Path, name: &str) -> Option<Module> {
    let bytes = std::fs::read(file).ok()?;
    let lua = file.extension().is_some_and(|e| e == "lua");
    if !lua && !bytes.starts_with(b"#%Module") {
        return None;
    }
    let root = root_of(&String::from_utf8_lossy(&bytes));
    let mut listing: Vec<String> = root
        .as_ref()
        .and_then(|r| std::fs::read_dir(Path::new(r).join("easybuild")).ok())
        .map(|rd| {
            rd.flatten()
                .map(|e| e.file_name().to_string_lossy().into_owned())
                .collect()
        })
        .unwrap_or_default();
    listing.sort();
    let own = format!("{}.eb", name.replacen('/', "-", 1));
    let file_name = if listing.contains(&own) {
        Some(own)
    } else {
        let mut ebs = listing.iter().filter(|f| f.ends_with(".eb"));
        match (ebs.next(), ebs.next()) {
            (Some(only), None) => Some(only.clone()),
            _ => None,
        }
    };
    let easyconfig = root
        .as_ref()
        .zip(file_name)
        .map(|(r, f)| format!("{r}/easybuild/{f}"));
    let mut h = Fnv::default();
    h.write(&bytes);
    for f in &listing {
        h.write(b"\0");
        h.write(f.as_bytes());
    }
    Some(Module {
        name: name.into(),
        root,
        easyconfig,
        identity: format!("fnv1a64:{:016x}", h.0),
    })
}

/// The install root an EasyBuild module file sets: `local root = "..."` in
/// Lua, `set root ...` in Tcl.
fn root_of(text: &str) -> Option<String> {
    text.lines().rev().find_map(|l| {
        let l = l.trim();
        if let Some(rest) = l.strip_prefix("local root") {
            let v = rest.trim_start().strip_prefix('=')?.trim();
            return path_value(v).map(str::to_string);
        }
        let rest = l.strip_prefix("set")?.trim_start();
        let value = rest.strip_prefix("root")?;
        value
            .chars()
            .next()
            .is_some_and(char::is_whitespace)
            .then(|| path_value(value).map(str::to_string))?
    })
}

fn path_value(text: &str) -> Option<&str> {
    let text = text.trim();
    for quote in ['"', '\''] {
        if let Some(rest) = text.strip_prefix(quote) {
            return rest.split_once(quote).map(|(value, _)| value);
        }
    }
    text.split_whitespace().next()
}

/// 64-bit FNV-1a, enough to tell a changed module from an unchanged one.
struct Fnv(u64);

impl Default for Fnv {
    fn default() -> Self {
        Fnv(0xcbf2_9ce4_8422_2325)
    }
}

impl Fnv {
    fn write(&mut self, bytes: &[u8]) {
        for b in bytes {
            self.0 ^= u64::from(*b);
            self.0 = self.0.wrapping_mul(0x0000_0100_0000_01b3);
        }
    }
}

#[cfg(test)]
#[allow(clippy::disallowed_methods)]
mod tests {
    use super::*;

    fn module(dir: &Path, name: &str, version: &str, logs: &[&str]) -> String {
        let root = dir.join("software").join(name).join(version);
        std::fs::create_dir_all(root.join("easybuild")).unwrap();
        std::fs::write(
            root.join("easybuild").join(format!("{name}-{version}.eb")),
            "",
        )
        .unwrap();
        for l in logs {
            std::fs::write(root.join("easybuild").join(l), "").unwrap();
        }
        let mods = dir.join("modules/all").join(name);
        std::fs::create_dir_all(&mods).unwrap();
        std::fs::write(
            mods.join(format!("{version}.lua")),
            format!("help([==[x]==])\nlocal root = \"{}\"\n", root.display()),
        )
        .unwrap();
        dir.join("modules/all").display().to_string()
    }

    #[test]
    fn a_module_rebuilt_in_place_gets_another_identity() {
        let d = tempfile::tempdir().unwrap();
        let tree = [module(d.path(), "Foo", "1.0", &["easybuild-Foo-1.0-1.log"])];
        let before = find(&tree, "Foo/1.0").unwrap();
        assert_eq!(
            before.easyconfig.as_deref(),
            Some(
                format!(
                    "{}/software/Foo/1.0/easybuild/Foo-1.0.eb",
                    d.path().display()
                )
                .as_str()
            )
        );
        assert_eq!(scan(&tree).get("Foo/1.0"), Some(&before));

        module(d.path(), "Foo", "1.0", &["easybuild-Foo-1.0-2.log"]);
        assert_ne!(find(&tree, "Foo/1.0").unwrap().identity, before.identity);
        assert!(find(&tree, "Bar/1.0").is_none());
    }

    #[test]
    fn the_environment_names_the_version_and_the_module_trees() {
        let env = parse_env0(b"EESSI_VERSION=2023.06\0EESSI_SOFTWARE_SUBDIR=x86_64/generic\0EESSI_MODULEPATH=/e/modules/all\0EESSI_CVMFS_REPO=/e\0LOADEDMODULES=EESSI/2023.06:EasyBuild\0_ModuleTable_Sz_=2\0_ModuleTable001_=YWJj\0_ModuleTable002_=-_-_\0A=b=c\0");
        assert_eq!(env["A"], "b=c");
        let p = Probe::from_env(&env, None, &[]).unwrap();
        assert_eq!(p.version, "2023.06");
        assert_eq!(p.modules_dirs, ["/e/modules/all"]);
        assert_eq!(p.repository.as_deref(), Some("/e"));
        let recorded = serde_json::to_value(&p).unwrap();
        assert_eq!(recorded["loaded_modules"], "EESSI/2023.06:EasyBuild");
        assert_eq!(
            recorded["module_table_sha256"],
            format!("{:x}", Sha256::digest(b"abc\xfb\xff\xbf"))
        );
        let fixture = Probe::from_env(&env, Some("/f"), &["/f/all".into()]).unwrap();
        assert_eq!(fixture.modules_dirs, ["/f/all"]);
        assert_eq!(fixture.repository.as_deref(), Some("/f"));
        assert_eq!(
            Probe::from_env(&BTreeMap::new(), None, &[]).unwrap_err(),
            "EESSI_VERSION is unset"
        );
    }

    #[test]
    fn a_missing_module_table_chunk_is_not_recorded() {
        let env = parse_env0(b"EESSI_VERSION=2023.06\0EESSI_SOFTWARE_SUBDIR=x86_64/generic\0EESSI_MODULEPATH=/e/modules/all\0_ModuleTable_Sz_=2\0_ModuleTable001_=YWJj\0");
        assert_eq!(
            Probe::from_env(&env, None, &[])
                .unwrap()
                .module_table_sha256,
            None
        );
    }

    #[test]
    fn singleton_and_chunked_module_table_exports_hash_the_same() {
        let singleton = parse_env0(b"EESSI_VERSION=2023.06\0EESSI_SOFTWARE_SUBDIR=x86_64/generic\0EESSI_MODULEPATH=/e/modules/all\0_ModuleTable_=YWJj\0");
        let chunked = parse_env0(b"EESSI_VERSION=2023.06\0EESSI_SOFTWARE_SUBDIR=x86_64/generic\0EESSI_MODULEPATH=/e/modules/all\0_ModuleTable_Sz_=1\0_ModuleTable001_=YWJj\0");
        assert_eq!(
            Probe::from_env(&singleton, None, &[])
                .unwrap()
                .module_table_sha256,
            Probe::from_env(&chunked, None, &[])
                .unwrap()
                .module_table_sha256
        );
    }

    #[test]
    fn a_malformed_singleton_module_table_is_not_recorded() {
        let env = parse_env0(b"EESSI_VERSION=2023.06\0EESSI_SOFTWARE_SUBDIR=x86_64/generic\0EESSI_MODULEPATH=/e/modules/all\0_ModuleTable_=!\0");
        assert_eq!(
            Probe::from_env(&env, None, &[])
                .unwrap()
                .module_table_sha256,
            None
        );
    }

    #[test]
    fn tcl_and_lua_module_files_name_their_root() {
        assert_eq!(root_of("local root = \"/a/b\"\n").as_deref(), Some("/a/b"));
        assert_eq!(root_of("local root = '/a c'\n").as_deref(), Some("/a c"));
        assert_eq!(
            root_of("#%Module\nset root    \"/a d\"\n").as_deref(),
            Some("/a d")
        );
        assert_eq!(root_of("setenv FOO 1\n"), None);
    }

    #[test]
    fn the_last_module_root_assignment_is_effective() {
        assert_eq!(
            root_of("local root = \"/old\"\nlocal root = \"/new\"\n").as_deref(),
            Some("/new")
        );
        assert_eq!(
            root_of("#%Module\nset root /old\nset root /new\n").as_deref(),
            Some("/new")
        );
    }

    #[test]
    fn a_newer_yyyy_mm_directory_is_the_newest_one() {
        let d = tempfile::tempdir().unwrap();
        let versions = d.path().join("versions");
        std::fs::create_dir_all(versions.join("2023.06")).unwrap();
        std::fs::create_dir_all(versions.join("2025.06")).unwrap();
        std::fs::create_dir_all(versions.join("2026.06")).unwrap();
        std::fs::write(versions.join("latest"), "not a directory name we read\n").unwrap();
        std::fs::create_dir(versions.join("2024.13")).unwrap();
        assert_eq!(
            newer_generation(&versions, "2023.06").unwrap().as_deref(),
            Some("2026.06")
        );
        assert_eq!(newer_generation(&versions, "2026.06").unwrap(), None);
        assert_eq!(newer_generation(&versions, "latest").unwrap(), None);
        assert_eq!(
            newer_generation(&d.path().join("absent"), "2023.06").unwrap(),
            None
        );
    }

    #[test]
    fn a_module_generation_is_eessi_then_yyyy_mm() {
        assert_eq!(module_generation("EESSI/2025.06"), Some("2025.06"));
        assert_eq!(module_generation("EESSI/2025.13"), None);
        assert_eq!(module_generation("EESSI/2025.6"), None);
        assert_eq!(module_generation("EESSI/2025.06/extra"), None);
        assert_eq!(module_generation("2025.06"), None);
        assert_eq!(
            last_init_generation(&[
                "EESSI/2023.06".into(),
                "Other/1.0".into(),
                "EESSI/2025.06".into(),
            ]),
            Some("2025.06")
        );
        assert_eq!(
            last_init_generation(&["EESSI/2025.13".into(), "Other/1.0".into()]),
            None
        );
    }

    #[test]
    fn a_directory_without_cvmfs_has_no_revision() {
        let d = tempfile::tempdir().unwrap();
        assert_eq!(revision(d.path()).unwrap(), None);
        assert!(revision(&d.path().join("absent")).is_err());
    }
}
