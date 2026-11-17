//! What each local install of a target was built against in EESSI, for
//! `[watch]`. `eb-stack rebuild-set` says which EESSI modules an install
//! depends on and how; eb-pipeline records each one's identity beside it.

use crate::eessi::{self, Module, Probe};
use crate::exit::{self, Fail};
use crate::plan::{module_rel, Planner};
use crate::run::{exec_logged, log, RunResult};
use crate::schema::Site;
use crate::tools::{self, Tool};
use crate::util::{cmp_version, parse_version, ListLine};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

/// The `schema_version` of the state file.
pub const SCHEMA_VERSION: u32 = 1;
const KIND: &str = "watch-state";
/// The `rebuild-set.json` schema this reads.
const REBUILD_SET_SCHEMA: u64 = 5;

/// `<state_dir>/<target>.json`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct State {
    /// [`SCHEMA_VERSION`].
    pub schema_version: u32,
    /// `watch-state`.
    pub kind: String,
    /// The name of the target.
    pub target: String,
    /// By module name, `Name/version`.
    pub installs: BTreeMap<String, Install>,
}

/// One local install and the EESSI it was built against.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Install {
    /// The easyconfig as the build list named it.
    pub easyconfig: String,
    /// `built` by a run of eb-pipeline.
    pub how: String,
    /// Log stamp of the run that recorded it.
    pub run: String,
    /// When it was recorded, in RFC 3339.
    pub recorded_at: String,
    /// The `EESSI_VERSION` it was built against.
    pub eessi_version: String,
    /// The `EESSI_SOFTWARE_SUBDIR` it was built against.
    pub software_subdir: String,
    /// The EESSI module trees its dependencies were found in.
    pub modules_dirs: Vec<String>,
    /// The CVMFS revision its dependencies were last found at; none for
    /// module trees outside CVMFS.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub revision: Option<u64>,
    /// EESSI modules it depends on, by module name.
    pub deps: BTreeMap<String, Dep>,
}

/// An EESSI module an install depends on, as `eb-stack rebuild-set` found.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Dep {
    /// The file name of its easyconfig.
    pub easyconfig: String,
    /// [`Module::identity`] when it was recorded.
    pub identity: String,
    /// `must-rebuild`, `review` or `no-rebuild`: what a change of it means
    /// for the install.
    pub class: String,
    /// The `edge_kind` of the row that gave the class, such as `runtime`.
    pub edge_kind: String,
}

impl State {
    fn new(target: &str) -> State {
        State {
            schema_version: SCHEMA_VERSION,
            kind: KIND.into(),
            target: target.into(),
            installs: BTreeMap::new(),
        }
    }
}

/// `<dir>/<target>.json`.
pub fn state_path(dir: &str, target: &str) -> PathBuf {
    Path::new(dir).join(format!("{target}.json"))
}

/// The recorded state of `target`; empty before its first record.
pub fn load(dir: &str, target: &str) -> Result<State, String> {
    let p = state_path(dir, target);
    let text = match std::fs::read_to_string(&p) {
        Ok(t) => t,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(State::new(target)),
        Err(e) => return Err(format!("cannot read {}: {e}", p.display())),
    };
    let s: State = serde_json::from_str(&text).map_err(|e| format!("{}: {e}", p.display()))?;
    if s.kind != KIND || s.schema_version != SCHEMA_VERSION {
        return Err(format!(
            "{}: kind `{}`, schema_version {}; this eb-pipeline reads `{KIND}` {SCHEMA_VERSION}",
            p.display(),
            s.kind,
            s.schema_version
        ));
    }
    if s.target != target {
        return Err(format!(
            "{}: holds target `{}`, not `{target}`",
            p.display(),
            s.target
        ));
    }
    Ok(s)
}

/// Change the state of `target` under `<target>.lock`, so that a run's
/// record and a watch's do not drop each other's installs.
pub fn update(dir: &str, target: &str, f: impl FnOnce(&mut State)) -> Result<State, String> {
    std::fs::create_dir_all(dir).map_err(|e| format!("cannot create {dir}: {e}"))?;
    let lock_path = Path::new(dir).join(format!("{target}.lock"));
    let lock = std::fs::OpenOptions::new()
        .create(true)
        .truncate(false)
        .write(true)
        .open(&lock_path)
        .map_err(|e| format!("cannot open {}: {e}", lock_path.display()))?;
    lock.lock()
        .map_err(|e| format!("cannot lock {}: {e}", lock_path.display()))?;
    let mut s = load(dir, target)?;
    f(&mut s);
    let text = serde_json::to_string_pretty(&serde_json::to_value(&s).expect("serializable"))
        .expect("serializable")
        + "\n";
    let p = state_path(dir, target);
    let tmp = Path::new(dir).join(format!(".{target}.json.tmp"));
    std::fs::write(&tmp, text)
        .and_then(|_| std::fs::rename(&tmp, &p))
        .map_err(|e| format!("cannot write {}: {e}", p.display()))?;
    Ok(s)
}

/// `<modules_root>/all`, the target's module tree.
pub fn local_modules_dir(p: &Planner) -> String {
    format!("{}/all", p.scope.modules_root().trim_end_matches('/'))
}

/// `<scratch>/eb-pipeline-watch/<target>/<log stamp>/<what>`, emptied.
pub fn work_dir(p: &Planner, what: &str) -> Result<PathBuf, Fail> {
    let d = Path::new(&p.scope.site.filesystems.scratch)
        .join("eb-pipeline-watch")
        .join(&p.scope.target.name)
        .join(&p.scope.log_stamp)
        .join(what);
    let _ = std::fs::remove_dir_all(&d);
    std::fs::create_dir_all(&d)
        .map_err(|e| Fail::new(exit::FAILED, format!("cannot create {}: {e}", d.display())))?;
    Ok(d)
}

/// Refuse without an eb-stack to ask; one below `min_version` is a usage
/// error, as for the phases that call it.
pub fn need_eb_stack(site: &Site) -> Result<(), Fail> {
    let bin = Tool::EbStack.bin(site);
    let found = tools::probe(&bin).map_err(|e| {
        Fail::new(
            exit::REFUSED,
            format!("eb-stack tells what an install depends on, and {e}"),
        )
    })?;
    if let Some(min) = Tool::EbStack
        .min_version(site)
        .as_deref()
        .and_then(parse_version)
    {
        if cmp_version(&found, &min) == std::cmp::Ordering::Less {
            let v = |v: &[u64]| v.iter().map(u64::to_string).collect::<Vec<_>>().join(".");
            return Err(Fail::usage(format!(
                "eb_stack: version {} is below min_version {}",
                v(&found),
                v(&min)
            )));
        }
    }
    Ok(())
}

/// EESSI as the target's module environment loads it, from `env -0` run
/// inside the module wrapper, as `stack shard` runs; and that environment.
pub fn probe(p: &Planner, work: &Path) -> Result<(Probe, BTreeMap<String, String>), Fail> {
    let w = p.scope.site.watch_on().expect("[watch] is on");
    let target = &p.scope.target.name;
    let out = work.join("eessi.env");
    let inner = ["sh", "-c", "env -0 > \"$1\"", "sh"]
        .into_iter()
        .map(String::from)
        .chain([out.display().to_string()])
        .collect();
    let cmd = p.plan_role_cmd("watch", inner)?;
    let log_path = work.join("eessi-probe.log");
    let code = exec_logged(&cmd, &p.scope.env(), &log_path)?;
    let bytes = std::fs::read(&out).unwrap_or_default();
    // The dump holds the whole environment, a webhook's URL among it, and
    // the work directory can outlive the run.
    let _ = std::fs::remove_file(&out);
    if code != 0 || bytes.is_empty() {
        return Err(Fail::new(
            exit::TEMPFAIL,
            format!(
                "the module environment of target {target} did not load (exit {code}); its output is in {}",
                log_path.display()
            ),
        ));
    }
    let env = eessi::parse_env0(&bytes);
    let probe = Probe::from_env(&env, w.repository.as_deref(), &w.modules_dirs).map_err(|why| {
        Fail::new(
            exit::REFUSED,
            format!("no EESSI in the module environment of target {target}: {why}"),
        )
    })?;
    Ok((probe, env))
}

/// The revision of the probed repository; one that cannot be read is
/// temporary, as when CVMFS is not mounted.
pub fn revision_of(probe: &Probe) -> Result<Option<u64>, Fail> {
    let Some(repo) = &probe.repository else {
        return Ok(None);
    };
    eessi::revision(Path::new(repo)).map_err(|e| {
        Fail::new(
            exit::TEMPFAIL,
            format!("cannot read the revision of {repo}: {e}"),
        )
    })
}

/// The easyconfig trees and installed list eb-stack reads: a link to the
/// easyconfig in each module's install, EESSI's in one tree and the
/// target's in another.
pub struct Universe {
    /// The tree of EESSI's easyconfigs.
    pub eessi_dir: PathBuf,
    /// The tree of the target's easyconfigs.
    pub local_dir: PathBuf,
    /// The installed modules, one `Name/version` per line.
    pub installed: PathBuf,
    /// Easyconfig file name to module name.
    pub eessi_files: BTreeMap<String, String>,
    /// The same for the target's tree.
    pub local_files: BTreeMap<String, String>,
}

/// Link the easyconfig of each module of `eessi` and `local` under `work`,
/// the first module of a file name only, and list them all as installed.
pub fn stage(
    work: &Path,
    eessi: &BTreeMap<String, Module>,
    local: &BTreeMap<String, Module>,
) -> Result<Universe, Fail> {
    let link = |dir: &Path, mods: &BTreeMap<String, Module>| {
        std::fs::create_dir_all(dir).map_err(|e| {
            Fail::new(
                exit::FAILED,
                format!("cannot create {}: {e}", dir.display()),
            )
        })?;
        let mut files = BTreeMap::new();
        for m in mods.values() {
            let Some(ec) = &m.easyconfig else {
                continue;
            };
            let file = ec.rsplit('/').next().unwrap_or(ec).to_string();
            if files.contains_key(&file) {
                continue;
            }
            std::os::unix::fs::symlink(ec, dir.join(&file)).map_err(|e| {
                Fail::new(
                    exit::FAILED,
                    format!("cannot link {file} in {}: {e}", dir.display()),
                )
            })?;
            files.insert(file, m.name.clone());
        }
        Ok::<_, Fail>(files)
    };
    let eessi_dir = work.join("easyconfigs/eessi");
    let local_dir = work.join("easyconfigs/local");
    let eessi_files = link(&eessi_dir, eessi)?;
    let local_files = link(&local_dir, local)?;
    let installed = work.join("installed.txt");
    let names: std::collections::BTreeSet<&String> = eessi.keys().chain(local.keys()).collect();
    let text: String = names.iter().map(|n| format!("{n}\n")).collect();
    std::fs::write(&installed, text).map_err(|e| {
        Fail::new(
            exit::FAILED,
            format!("cannot write {}: {e}", installed.display()),
        )
    })?;
    Ok(Universe {
        eessi_dir,
        local_dir,
        installed,
        eessi_files,
        local_files,
    })
}

/// One row of `rebuild-set.json`.
#[derive(Debug, Clone, Deserialize)]
pub struct Row {
    /// The changed easyconfigs this row is about; a `review` row lists
    /// several, joined by `, `.
    pub changed: String,
    /// The installed module that depends on them, `Name/version`.
    pub dependent: String,
    /// `must-rebuild`, `review` or `no-rebuild`.
    pub class: String,
    /// How the dependent reaches them, such as `builddependency`.
    pub edge_kind: String,
}

/// `eb-stack rebuild-set` of `changed` over the universe: which installed
/// modules depend on those easyconfigs, and how. The universe's local tree
/// comes last, so a local easyconfig wins over EESSI's of the same name.
pub fn rebuild_set(
    p: &Planner,
    work: &Path,
    u: &Universe,
    changed: &[String],
    name: &str,
    label: &str,
) -> Result<Vec<Row>, Fail> {
    if changed.is_empty() {
        return Ok(Vec::new());
    }
    let list = work.join(format!("{name}.changed"));
    let out = work.join(name);
    std::fs::write(&list, changed.join("\n") + "\n").map_err(|e| {
        Fail::new(
            exit::FAILED,
            format!("cannot write {}: {e}", list.display()),
        )
    })?;
    let path = |p: &Path| p.display().to_string();
    let argv = vec![
        Tool::EbStack.bin(p.scope.site),
        "rebuild-set".into(),
        "--changed".into(),
        path(&list),
        "--easyconfigs".into(),
        path(&u.eessi_dir),
        "--easyconfigs".into(),
        path(&u.local_dir),
        "--installed".into(),
        path(&u.installed),
        "--installed-label".into(),
        label.into(),
        "--out-dir".into(),
        path(&out),
    ];
    let cmd = p.plan_role_cmd("watch", argv)?;
    let log_path = work.join(format!("{name}.log"));
    let code = exec_logged(&cmd, &p.scope.env(), &log_path)?;
    if code != 0 {
        return Err(Fail::new(
            exit::FAILED,
            format!(
                "eb-stack rebuild-set exited {code}; its output is in {}",
                log_path.display()
            ),
        ));
    }
    read_rows(&out.join("rebuild-set.json"))
}

fn read_rows(path: &Path) -> Result<Vec<Row>, Fail> {
    let bad = |why: String| Fail::new(exit::FAILED, format!("{}: {why}", path.display()));
    let text = std::fs::read_to_string(path).map_err(|e| bad(e.to_string()))?;
    let mut v: serde_json::Value = serde_json::from_str(&text).map_err(|e| bad(e.to_string()))?;
    let schema = v["schema_version"].as_u64();
    if schema != Some(REBUILD_SET_SCHEMA) {
        return Err(bad(format!(
            "schema_version {schema:?}; this eb-pipeline reads {REBUILD_SET_SCHEMA}"
        )));
    }
    serde_json::from_value(v["rows"].take()).map_err(|e| bad(format!("rows: {e}")))
}

fn rank(class: &str) -> u8 {
    match class {
        "must-rebuild" => 2,
        "review" => 1,
        _ => 0,
    }
}

/// The EESSI modules `install` depends on in `rows`, each with the
/// strongest class a row gives it.
pub fn deps_of(
    rows: &[Row],
    install: &str,
    files: &BTreeMap<String, String>,
    modules: &BTreeMap<String, Module>,
) -> BTreeMap<String, Dep> {
    let mut out: BTreeMap<String, Dep> = BTreeMap::new();
    for r in rows.iter().filter(|r| r.dependent == install) {
        for file in r.changed.split(", ") {
            let Some(m) = files.get(file).and_then(|n| modules.get(n)) else {
                continue;
            };
            if out
                .get(&m.name)
                .is_some_and(|d| rank(&d.class) >= rank(&r.class))
            {
                continue;
            }
            let dep = Dep {
                easyconfig: file.into(),
                identity: m.identity.clone(),
                class: r.class.clone(),
                edge_kind: r.edge_kind.clone(),
            };
            out.insert(m.name.clone(), dep);
        }
    }
    out
}

/// An install to record: its module, and its easyconfig as the build list
/// named it.
pub struct ToRecord {
    /// `Name/version`.
    pub module: String,
    /// The easyconfig as the build list named it.
    pub easyconfig: String,
    /// [`Install::how`].
    pub how: &'static str,
}

/// Record `installs` against the EESSI `probe` describes, each with the
/// dependencies `eb-stack rebuild-set` finds for it when every EESSI
/// easyconfig counts as changed. Returns the modules recorded; an install
/// without a module file is left out.
pub fn record(
    p: &Planner,
    work: &Path,
    probe: &Probe,
    revision: Option<u64>,
    installs: &[ToRecord],
) -> Result<Vec<String>, Fail> {
    let eessi = eessi::scan(&probe.modules_dirs);
    let local = eessi::scan(&[local_modules_dir(p)]);
    let u = stage(work, &eessi, &local)?;
    let changed: Vec<String> = u
        .eessi_files
        .keys()
        .filter(|f| !u.local_files.contains_key(*f))
        .cloned()
        .collect();
    let rows = rebuild_set(p, work, &u, &changed, "rebuild-set", &probe.software_subdir)?;
    let now = chrono::Utc::now().to_rfc3339();
    let entries: Vec<(String, Install)> = installs
        .iter()
        .filter(|i| local.contains_key(&i.module))
        .map(|i| {
            let install = Install {
                easyconfig: i.easyconfig.clone(),
                how: i.how.into(),
                run: p.scope.log_stamp.clone(),
                recorded_at: now.clone(),
                eessi_version: probe.version.clone(),
                software_subdir: probe.software_subdir.clone(),
                modules_dirs: probe.modules_dirs.clone(),
                revision,
                deps: deps_of(&rows, &i.module, &u.eessi_files, &eessi),
            };
            (i.module.clone(), install)
        })
        .collect();
    let names: Vec<String> = entries.iter().map(|(m, _)| m.clone()).collect();
    let dir = p.scope.watch_state_dir().expect("[watch] is on");
    update(&dir, &p.scope.target.name, |s| s.installs.extend(entries))
        .map_err(|e| Fail::new(exit::FAILED, e))?;
    Ok(names)
}

/// The `easybuild` directory listing of each line's install, taken before a
/// run so that `record_run` can tell the lines it built from those that
/// left an install as it was.
pub type Listings = BTreeMap<String, Vec<String>>;

/// The listings of the installs of `lines`, by module.
pub fn listings(p: &Planner, lines: &[ListLine]) -> Listings {
    lines
        .iter()
        .filter_map(|l| module_rel(&l.easyconfig))
        .map(|rel| {
            let l = listing(p, &rel);
            (rel, l)
        })
        .collect()
}

fn listing(p: &Planner, rel: &str) -> Vec<String> {
    let dir = Path::new(&p.scope.software_root())
        .join(rel)
        .join("easybuild");
    let mut v: Vec<String> = std::fs::read_dir(dir)
        .map(|rd| {
            rd.flatten()
                .map(|e| e.file_name().to_string_lossy().into_owned())
                .collect()
        })
        .unwrap_or_default();
    v.sort();
    v
}

/// After a run, record the installs it built. A problem is a warning; the
/// run's exit code stays as it was.
pub fn record_run(p: &Planner, res: &RunResult, before: &Listings) {
    let target = &p.scope.target.name;
    let mut built = Vec::new();
    let mut unchanged = 0;
    for l in res.lines.iter().filter(|l| l.status == "success") {
        let Some(rel) = module_rel(&l.easyconfig) else {
            continue;
        };
        if before.get(&rel) == Some(&listing(p, &rel)) {
            unchanged += 1;
            continue;
        }
        built.push(ToRecord {
            module: rel,
            easyconfig: l.easyconfig.clone(),
            how: "built",
        });
    }
    if unchanged > 0 {
        log(
            "INFO",
            "watch",
            target,
            &format!("{unchanged} line(s) left their install as it was and are not recorded again"),
        );
    }
    if built.is_empty() {
        return;
    }
    let what = built
        .iter()
        .map(|b| b.module.as_str())
        .collect::<Vec<_>>()
        .join(", ");
    let outcome = if let Some(sig) = &res.interrupted_by {
        Err(Fail::new(
            res.exit_code,
            format!("the run was stopped by {sig}"),
        ))
    } else {
        record_now(p, &built)
    };
    match outcome {
        Ok((probe, revision, names)) => log(
            "INFO",
            "watch",
            target,
            &format!(
                "recorded {} against EESSI {} {}{}: {}",
                names.len(),
                probe.version,
                probe.software_subdir,
                revision
                    .map(|r| format!(" at revision {r}"))
                    .unwrap_or_default(),
                names.join(", ")
            ),
        ),
        Err(f) => log(
            "WARN",
            "watch",
            target,
            &format!(
                "{what} not recorded: {}; no EESSI change under them is seen until a record of them is written",
                f.msg
            ),
        ),
    }
}

/// Probe EESSI and record `installs`. The work directory goes once the
/// record is written, and stays for a look when it is not.
fn record_now(
    p: &Planner,
    installs: &[ToRecord],
) -> Result<(Probe, Option<u64>, Vec<String>), Fail> {
    need_eb_stack(p.scope.site)?;
    let work = work_dir(p, "record")?;
    let (probe, _) = probe(p, &work)?;
    let revision = revision_of(&probe)?;
    let names = record(p, &work, &probe, revision, installs)?;
    let _ = std::fs::remove_dir_all(&work);
    Ok((probe, revision, names))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn module(name: &str) -> Module {
        Module {
            name: name.into(),
            root: None,
            easyconfig: None,
            identity: format!("id-{name}"),
        }
    }

    #[test]
    fn a_review_row_names_several_dependencies_and_the_strongest_class_wins() {
        let row = |changed: &str, class: &str, edge: &str| Row {
            changed: changed.into(),
            dependent: "Foo/1.0".into(),
            class: class.into(),
            edge_kind: edge.into(),
        };
        let rows = [
            row("A-1.eb, B-2.eb", "review", "builddependency"),
            row("B-2.eb", "no-rebuild", "runtime"),
            row("C-3.eb", "no-rebuild", "runtime"),
            row("Unknown-1.eb", "must-rebuild", "changed"),
            Row {
                dependent: "Bar/1.0".into(),
                ..row("C-3.eb", "must-rebuild", "changed")
            },
        ];
        let files: BTreeMap<String, String> =
            [("A-1.eb", "A/1"), ("B-2.eb", "B/2"), ("C-3.eb", "C/3")]
                .into_iter()
                .map(|(f, m)| (f.to_string(), m.to_string()))
                .collect();
        let modules: BTreeMap<String, Module> =
            files.values().map(|m| (m.clone(), module(m))).collect();
        let deps = deps_of(&rows, "Foo/1.0", &files, &modules);
        let class = |m: &str| deps[m].class.as_str();
        assert_eq!(deps.keys().collect::<Vec<_>>(), ["A/1", "B/2", "C/3"]);
        assert_eq!(class("A/1"), "review");
        assert_eq!(class("B/2"), "review");
        assert_eq!(class("C/3"), "no-rebuild");
        assert_eq!(deps["C/3"].identity, "id-C/3");
        assert_eq!(deps["A/1"].easyconfig, "A-1.eb");
    }

    #[test]
    fn a_state_file_of_another_target_or_schema_is_refused() {
        let d = tempfile::tempdir().unwrap();
        let dir = d.path().display().to_string();
        assert!(load(&dir, "cpu").unwrap().installs.is_empty());
        update(&dir, "cpu", |_| {}).unwrap();
        assert_eq!(load(&dir, "cpu").unwrap().target, "cpu");
        std::fs::copy(state_path(&dir, "cpu"), state_path(&dir, "gpu")).unwrap();
        assert!(load(&dir, "gpu")
            .unwrap_err()
            .contains("holds target `cpu`, not `gpu`"));
        std::fs::write(
            state_path(&dir, "cpu"),
            r#"{"schema_version":2,"kind":"watch-state","target":"cpu","installs":{}}"#,
        )
        .unwrap();
        assert!(load(&dir, "cpu").unwrap_err().contains("schema_version 2"));
    }
}
