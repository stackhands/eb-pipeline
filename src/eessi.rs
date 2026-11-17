//! What eb-pipeline reads of EESSI for `[watch]`: the version a target's
//! module environment loads, the CVMFS revision of its repository, and an
//! identity for each module in its module trees.

use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::path::Path;

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
        })
    }
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
    text.lines().find_map(|l| {
        let l = l.trim();
        if let Some(rest) = l.strip_prefix("local root") {
            let v = rest.trim_start().strip_prefix('=')?.trim();
            return Some(v.trim_matches('"').to_string());
        }
        let rest = l.strip_prefix("set")?;
        let mut w = rest.split_whitespace();
        (w.next()? == "root").then(|| w.next().map(str::to_string))?
    })
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
        let env = parse_env0(b"EESSI_VERSION=2023.06\0EESSI_SOFTWARE_SUBDIR=x86_64/generic\0EESSI_MODULEPATH=/e/modules/all\0EESSI_CVMFS_REPO=/e\0A=b=c\0");
        assert_eq!(env["A"], "b=c");
        let p = Probe::from_env(&env, None, &[]).unwrap();
        assert_eq!(p.version, "2023.06");
        assert_eq!(p.modules_dirs, ["/e/modules/all"]);
        assert_eq!(p.repository.as_deref(), Some("/e"));
        let fixture = Probe::from_env(&env, Some("/f"), &["/f/all".into()]).unwrap();
        assert_eq!(fixture.modules_dirs, ["/f/all"]);
        assert_eq!(fixture.repository.as_deref(), Some("/f"));
        assert_eq!(
            Probe::from_env(&BTreeMap::new(), None, &[]).unwrap_err(),
            "EESSI_VERSION is unset"
        );
    }

    #[test]
    fn tcl_and_lua_module_files_name_their_root() {
        assert_eq!(root_of("local root = \"/a/b\"\n").as_deref(), Some("/a/b"));
        assert_eq!(
            root_of("#%Module\nset root    /a/c\n").as_deref(),
            Some("/a/c")
        );
        assert_eq!(root_of("setenv FOO 1\n"), None);
    }

    #[test]
    fn a_directory_without_cvmfs_has_no_revision() {
        let d = tempfile::tempdir().unwrap();
        assert_eq!(revision(d.path()).unwrap(), None);
        assert!(revision(&d.path().join("absent")).is_err());
    }
}
