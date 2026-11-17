//! What each local install of a target was built against in EESSI, and the
//! targeted rebuild of the installs an EESSI change reaches. `eb-stack
//! rebuild-set` says which EESSI modules an install depends on and how, and
//! `eb-stack stack shard` orders the rebuild; eb-pipeline records each
//! dependency's identity, compares it with EESSI now, and drives the run.

use crate::eessi::{self, Module, Probe};
use crate::exit::{self, Fail};
use crate::plan::{module_rel, read_units, Planner};
use crate::resolve::job_dir_refusal;
use crate::run::{exec_logged, log, RunResult};
use crate::schema::{Site, Watch};
use crate::tools::{self, Tool};
use crate::util::{cmp_version, list_word, parse_version, ListLine};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::fs::File;
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
    /// The options that its build list line gave `eb`, without a rebuild
    /// option; a rebuild passes them again.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub args: Vec<String>,
    /// `built` by a run of eb-pipeline or by `watch`, or `adopted` by
    /// `watch --adopt`.
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
    /// The changes a watch rebuilt it for, kept while no rebuild of it has
    /// succeeded, so that the next watch rebuilds it again.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub pending: Vec<Change>,
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

/// `: "LINE"`, where LINE is the line of `log` in which the job directory
/// wrapper says why it started no command; nothing when no line says so.
fn refusal_quote(log: &Path) -> String {
    let output = std::fs::read(log).unwrap_or_default();
    job_dir_refusal(&String::from_utf8_lossy(&output))
        .map(|l| format!(": \"{l}\""))
        .unwrap_or_default()
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
                "the module environment of target {target} did not load (exit {code}); its output is in {}{}",
                log_path.display(),
                refusal_quote(&log_path)
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
                "eb-stack rebuild-set exited {code}; its output is in {}{}",
                log_path.display(),
                refusal_quote(&log_path)
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

/// An install to record: its module, and its easyconfig and options as its
/// build list line gave them.
pub struct ToRecord {
    /// `Name/version`.
    pub module: String,
    /// The easyconfig as the build list named it.
    pub easyconfig: String,
    /// [`Install::args`].
    pub args: Vec<String>,
    /// [`Install::how`].
    pub how: &'static str,
}

/// Record `installs` against the EESSI `probe` describes, each with the
/// dependencies `eb-stack rebuild-set` finds for it when every EESSI
/// easyconfig counts as changed, beside the recorded ones or, with
/// `replace`, in their place. Returns the modules recorded; an install
/// without a module file is left out.
pub fn record(
    p: &Planner,
    work: &Path,
    probe: &Probe,
    revision: Option<u64>,
    installs: &[ToRecord],
    replace: bool,
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
                args: i.args.clone(),
                how: i.how.into(),
                run: p.scope.log_stamp.clone(),
                recorded_at: now.clone(),
                eessi_version: probe.version.clone(),
                software_subdir: probe.software_subdir.clone(),
                modules_dirs: probe.modules_dirs.clone(),
                revision,
                deps: deps_of(&rows, &i.module, &u.eessi_files, &eessi),
                pending: Vec::new(),
            };
            (i.module.clone(), install)
        })
        .collect();
    let names: Vec<String> = entries.iter().map(|(m, _)| m.clone()).collect();
    let dir = p.scope.watch_state_dir().expect("[watch] is on");
    update(&dir, &p.scope.target.name, |s| {
        if replace {
            s.installs.clear();
        }
        s.installs.extend(entries)
    })
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

/// The installs the successful lines of a run of `lines` built, whose
/// `easybuild` listing is not the one `before` holds, and the number of
/// successful lines that left their install as it was.
fn built_by(
    p: &Planner,
    lines: &[ListLine],
    res: &RunResult,
    before: &Listings,
) -> Result<(Vec<ToRecord>, usize), Fail> {
    let mut built = Vec::new();
    let mut unchanged = 0;
    let (kept, _) = p.select_lines(lines)?;
    for l in res.lines.iter().filter(|l| l.status == "success") {
        let Some(rel) = module_rel(&l.easyconfig) else {
            continue;
        };
        if before.get(&rel) == Some(&listing(p, &rel)) {
            unchanged += 1;
            continue;
        }
        let args = kept
            .iter()
            .find(|x| x.easyconfig == l.easyconfig)
            .map(|x| {
                x.args
                    .iter()
                    .filter(|a| !matches!(a.as_str(), "--rebuild" | "-f" | "--force"))
                    .cloned()
                    .collect()
            })
            .unwrap_or_default();
        built.push(ToRecord {
            module: rel,
            easyconfig: l.easyconfig.clone(),
            args,
            how: "built",
        });
    }
    Ok((built, unchanged))
}

/// After a run of `lines`, record the installs it built. A problem is a
/// warning; the run's exit code stays as it was.
pub fn record_run(p: &Planner, lines: &[ListLine], res: &RunResult, before: &Listings) {
    let target = &p.scope.target.name;
    let (built, unchanged) = match built_by(p, lines, res, before) {
        Ok(v) => v,
        Err(f) => {
            log(
                "WARN",
                "watch",
                target,
                &format!(
                    "not recorded: {}; no EESSI change under them is seen until watch --adopt records them",
                    f.msg
                ),
            );
            return;
        }
    };
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
                "{what} not recorded: {}; no EESSI change under them is seen until watch --adopt records them",
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
    let names = record(p, &work, &probe, revision, installs, false)?;
    let _ = std::fs::remove_dir_all(&work);
    Ok((probe, revision, names))
}

/// Hold `<target>.watch.lock` for the whole of a watch; one that another
/// watch holds is temporary.
pub fn hold(dir: &str, target: &str) -> Result<File, Fail> {
    std::fs::create_dir_all(dir)
        .map_err(|e| Fail::new(exit::FAILED, format!("cannot create {dir}: {e}")))?;
    let p = Path::new(dir).join(format!("{target}.watch.lock"));
    let f = std::fs::OpenOptions::new()
        .create(true)
        .truncate(false)
        .write(true)
        .open(&p)
        .map_err(|e| Fail::new(exit::FAILED, format!("cannot open {}: {e}", p.display())))?;
    match f.try_lock() {
        Ok(()) => Ok(f),
        Err(std::fs::TryLockError::WouldBlock) => Err(Fail::new(
            exit::TEMPFAIL,
            format!("another watch of target {target} holds {}", p.display()),
        )),
        Err(std::fs::TryLockError::Error(e)) => Err(Fail::new(
            exit::FAILED,
            format!("cannot lock {}: {e}", p.display()),
        )),
    }
}

/// A module tree that cannot be read is temporary, as when CVMFS is not
/// mounted; every dependency in it would otherwise look vanished.
pub fn check_dirs(probe: &Probe) -> Result<(), Fail> {
    for d in &probe.modules_dirs {
        std::fs::read_dir(d).map_err(|e| {
            Fail::new(
                exit::TEMPFAIL,
                format!("cannot read the EESSI module tree {d}: {e}"),
            )
        })?;
    }
    Ok(())
}

/// Recorded installs built against an EESSI generation other than `version`.
/// When `subdir` is set, an install of that generation on another CPU
/// subdirectory is included too.
///
/// [`other_eessi`] passes the subdirectory the module environment loads.
/// A plan knows the generation in `[modules] init` and passes none, so a
/// move that keeps the generation and changes only the CPU stays a watch
/// refusal.
pub fn built_against_other<'a>(
    state: &'a State,
    version: &str,
    subdir: Option<&str>,
) -> Vec<(&'a String, &'a Install)> {
    state
        .installs
        .iter()
        .filter(|(_, i)| {
            i.eessi_version != version || subdir.is_some_and(|s| i.software_subdir != s)
        })
        .collect()
}

/// Recorded installs built against another EESSI version or CPU
/// subdirectory than the module environment loads now.
pub fn other_eessi<'a>(state: &'a State, probe: &Probe) -> Vec<(&'a String, &'a Install)> {
    built_against_other(state, &probe.version, Some(&probe.software_subdir))
}

/// Whether every install was last found at the repository's revision now,
/// over the same module trees, all inside the repository: then no module
/// can have changed under them.
pub fn unchanged_at(state: &State, probe: &Probe, revision: Option<u64>) -> bool {
    let (Some(rev), Some(repo)) = (revision, &probe.repository) else {
        return false;
    };
    probe
        .modules_dirs
        .iter()
        .all(|d| Path::new(d).starts_with(repo))
        && state.installs.values().all(|i| {
            i.revision == Some(rev) && i.modules_dirs == probe.modules_dirs && i.pending.is_empty()
        })
}

/// What a watch found for one recorded install.
#[derive(Debug, Clone, Serialize)]
pub struct Verdict {
    /// `Name/version`.
    pub module: String,
    /// [`Install::easyconfig`].
    pub easyconfig: String,
    /// [`Install::args`].
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub args: Vec<String>,
    /// `rebuild`, `keep`, `unaffected`, or `gone` when its module file is.
    pub action: String,
    /// The dependencies that are not as recorded.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub changes: Vec<Change>,
}

/// A dependency of an install that is not as recorded.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Change {
    /// `Name/version` of the dependency.
    pub module: String,
    /// `changed` or `vanished` in EESSI, or `rebuilt` for a local install
    /// this watch rebuilds.
    pub change: String,
    /// [`Dep::class`].
    pub class: String,
    /// [`Dep::edge_kind`].
    pub edge_kind: String,
}

fn rebuilds(c: &Change, review: bool) -> bool {
    match c.class.as_str() {
        "must-rebuild" => true,
        "review" => review,
        _ => c.change == "vanished",
    }
}

/// Each recorded install against the EESSI modules `dirs` holds now, with
/// the changes a failed rebuild left pending. A dependency eb-stack classed
/// `must-rebuild` rebuilds its install, and one it classed `review`, which
/// the install's build alone may have used, does so with `review`. One
/// classed `no-rebuild` rebuilds it only when it vanished, as the install's
/// module then fails to load.
pub fn assess(state: &State, dirs: &[String], local_dir: &str, review: bool) -> Vec<Verdict> {
    let mut now: BTreeMap<&str, Option<Module>> = BTreeMap::new();
    let local = [local_dir.to_string()];
    state
        .installs
        .iter()
        .map(|(m, i)| {
            let mut v = Verdict {
                module: m.clone(),
                easyconfig: i.easyconfig.clone(),
                args: i.args.clone(),
                action: "gone".into(),
                changes: Vec::new(),
            };
            if eessi::find(&local, m).is_none() {
                return v;
            }
            v.changes.clone_from(&i.pending);
            for (name, d) in &i.deps {
                if v.changes.iter().any(|c| c.module == *name) {
                    continue;
                }
                let found = now
                    .entry(name.as_str())
                    .or_insert_with(|| eessi::find(dirs, name));
                let change = match found {
                    None => "vanished",
                    Some(f) if f.identity != d.identity => "changed",
                    Some(_) => continue,
                };
                v.changes.push(Change {
                    module: name.clone(),
                    change: change.into(),
                    class: d.class.clone(),
                    edge_kind: d.edge_kind.clone(),
                });
            }
            v.action = if v.changes.iter().any(|c| rebuilds(c, review)) {
                "rebuild"
            } else if v.changes.is_empty() {
                "unaffected"
            } else {
                "keep"
            }
            .into();
            v
        })
        .collect()
}

fn file_name(easyconfig: &str) -> &str {
    easyconfig.rsplit('/').next().unwrap_or(easyconfig)
}

/// Add the recorded installs that depend on one this watch rebuilds, from
/// `eb-stack rebuild-set` of the rebuilt easyconfigs over the installed
/// modules: one that eb-stack classes as for `assess` is rebuilt too.
pub fn cascade(
    p: &Planner,
    work: &Path,
    u: &Universe,
    verdicts: &mut [Verdict],
    review: bool,
    label: &str,
) -> Result<(), Fail> {
    let changed: Vec<String> = verdicts
        .iter()
        .filter(|v| v.action == "rebuild")
        .map(|v| file_name(&v.easyconfig).to_string())
        .filter(|f| u.local_files.contains_key(f))
        .collect();
    let rows = rebuild_set(p, work, u, &changed, "cascade", label)?;
    for v in verdicts
        .iter_mut()
        .filter(|v| v.action == "keep" || v.action == "unaffected")
    {
        for r in rows.iter().filter(|r| r.dependent == v.module) {
            for file in r.changed.split(", ") {
                let Some(m) = u.local_files.get(file).filter(|m| **m != v.module) else {
                    continue;
                };
                v.changes.push(Change {
                    module: m.clone(),
                    change: "rebuilt".into(),
                    class: r.class.clone(),
                    edge_kind: r.edge_kind.clone(),
                });
            }
        }
        if v.changes.iter().any(|c| rebuilds(c, review)) {
            v.action = "rebuild".into();
        } else if !v.changes.is_empty() {
            v.action = "keep".into();
        }
    }
    Ok(())
}

/// Easyconfig trees under EESSI's and the target's for `stack shard`, so it
/// can plan the local build of a vanished EESSI module: `[watch]
/// easyconfigs`, or the tree of the EasyBuild the module environment loads.
pub fn robot_trees(w: &Watch, env: &BTreeMap<String, String>) -> Vec<String> {
    if !w.easyconfigs.is_empty() {
        return w.easyconfigs.clone();
    }
    env.get("EBROOTEASYBUILD")
        .map(|r| format!("{}/easybuild/easyconfigs", r.trim_end_matches('/')))
        .filter(|d| Path::new(d).is_dir())
        .into_iter()
        .collect()
}

/// The closures `plan.json` of `eb-stack stack shard` records: per line,
/// the modules its build needs that the plan builds, dependencies first.
#[derive(Deserialize)]
struct ShardPlan {
    #[serde(default)]
    lines: Vec<ShardPlanLine>,
}

#[derive(Deserialize)]
struct ShardPlanLine {
    easyconfig: String,
    module: String,
    #[serde(default)]
    closure: Option<Vec<String>>,
}

/// The rebuild list, each install with its recorded options and
/// `--rebuild`, in the order `eb-stack stack shard` gives it: a dependency
/// before what needs it. eb-stack orders the easyconfigs alone, which the
/// staged tree holds as they were installed. A module a line's closure
/// needs that no line builds, such as a vanished EESSI one, goes before
/// that line as `Name-version.eb`, since a build of the list has no robot.
/// Each option is a build list word, so a parenthesis or a star stays bare
/// and `stack shard`, which keeps quote characters, still sees the option.
pub fn order(
    p: &Planner,
    work: &Path,
    u: &Universe,
    verdicts: &[Verdict],
    robot: &[String],
) -> Result<Vec<String>, Fail> {
    let mut rebuild: BTreeMap<&str, String> = BTreeMap::new();
    for v in verdicts.iter().filter(|v| v.action == "rebuild") {
        let mut words = Vec::with_capacity(v.args.len() + 2);
        for a in std::iter::once(v.easyconfig.as_str()).chain(v.args.iter().map(String::as_str)) {
            words.push(list_word(a)?);
        }
        words.push("--rebuild".to_string());
        rebuild.insert(file_name(&v.easyconfig), words.join(" "));
    }
    let list = work.join("rebuild.list");
    let text: String = verdicts
        .iter()
        .filter(|v| v.action == "rebuild")
        .map(|v| format!("{} --rebuild\n", file_name(&v.easyconfig)))
        .collect();
    std::fs::write(&list, text).map_err(|e| {
        Fail::new(
            exit::FAILED,
            format!("cannot write {}: {e}", list.display()),
        )
    })?;
    let out = work.join("order");
    let path = |p: &Path| p.display().to_string();
    let mut argv = vec![
        Tool::EbStack.bin(p.scope.site),
        "stack".into(),
        "shard".into(),
        "--build-list".into(),
        path(&list),
        "--shards".into(),
        "1".into(),
    ];
    for d in robot.iter().map(String::as_str).chain([
        u.eessi_dir.to_str().unwrap_or(""),
        u.local_dir.to_str().unwrap_or(""),
    ]) {
        argv.push("--easyconfigs".into());
        argv.push(d.into());
    }
    argv.extend([
        "--installed".into(),
        path(&u.installed),
        "--out-dir".into(),
        path(&out),
    ]);
    let cmd = p.plan_role_cmd("watch", argv)?;
    let log_path = work.join("order.log");
    let code = exec_logged(&cmd, &p.scope.env(), &log_path)?;
    if code != 0 {
        return Err(Fail::new(
            exit::FAILED,
            format!(
                "eb-stack stack shard exited {code}; its output is in {}{}",
                log_path.display(),
                refusal_quote(&log_path)
            ),
        ));
    }
    let plan_path = out.join("plan.json");
    let plan: ShardPlan = std::fs::read_to_string(&plan_path)
        .map_err(|e| e.to_string())
        .and_then(|t| serde_json::from_str(&t).map_err(|e| e.to_string()))
        .map_err(|e| {
            Fail::new(
                exit::FAILED,
                format!("cannot read {}: {e}", plan_path.display()),
            )
        })?;
    let listed: Vec<&str> = plan.lines.iter().map(|l| l.module.as_str()).collect();
    let closure_of: BTreeMap<&str, &[String]> = plan
        .lines
        .iter()
        .map(|l| {
            (
                file_name(&l.easyconfig),
                l.closure.as_deref().unwrap_or_default(),
            )
        })
        .collect();
    let mut ordered = Vec::new();
    let mut added: Vec<&str> = Vec::new();
    for l in read_units(&out)?.iter().flat_map(|unit| &unit.lines) {
        let file = file_name(&l.easyconfig);
        for m in closure_of.get(file).copied().unwrap_or_default() {
            if !listed.contains(&m.as_str()) && !added.contains(&m.as_str()) {
                added.push(m);
                ordered.push(format!("{}.eb", m.replacen('/', "-", 1)));
            }
        }
        ordered.push(rebuild.get(file).cloned().unwrap_or_else(|| l.raw.clone()));
    }
    Ok(ordered)
}

/// What a watch found and planned, as `--plan-out` and the summary's
/// `watch` entry carry it.
#[derive(Debug, Clone, Serialize)]
pub struct PlanDoc {
    /// [`SCHEMA_VERSION`].
    pub schema_version: u32,
    /// `watch-plan`.
    pub kind: String,
    /// The name of the target.
    pub target: String,
    /// The EESSI the module environment loads.
    pub eessi: Probe,
    /// The CVMFS revision of the repository; none outside CVMFS.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub revision: Option<u64>,
    /// `unchanged` when the revision is the recorded one, or `unaffected`,
    /// `kept` or `rebuild`.
    pub outcome: String,
    /// What the watch found for each recorded install, in the order of
    /// their module names.
    pub installs: Vec<Verdict>,
    /// The rebuild list, in order.
    pub rebuild: Vec<String>,
}

impl PlanDoc {
    /// The plan for `verdicts`, with the outcome their actions give and no
    /// rebuild list yet.
    pub fn new(target: &str, probe: &Probe, revision: Option<u64>, verdicts: Vec<Verdict>) -> Self {
        let any = |a: &str| verdicts.iter().any(|v| v.action == a);
        let outcome = if any("rebuild") {
            "rebuild"
        } else if any("keep") {
            "kept"
        } else {
            "unaffected"
        };
        PlanDoc {
            schema_version: SCHEMA_VERSION,
            kind: "watch-plan".into(),
            target: target.into(),
            eessi: probe.clone(),
            revision,
            outcome: outcome.into(),
            installs: verdicts,
            rebuild: Vec::new(),
        }
    }

    /// The plan when the revision is the one every install was recorded
    /// at: each install unaffected, with no module compared.
    pub fn unchanged(target: &str, probe: &Probe, revision: Option<u64>, state: &State) -> Self {
        let verdicts = state
            .installs
            .iter()
            .map(|(m, i)| Verdict {
                module: m.clone(),
                easyconfig: i.easyconfig.clone(),
                args: i.args.clone(),
                action: "unaffected".into(),
                changes: Vec::new(),
            })
            .collect();
        PlanDoc {
            outcome: "unchanged".into(),
            ..PlanDoc::new(target, probe, revision, verdicts)
        }
    }

    /// The plan as `watch --dry-run` prints it.
    pub fn text(&self) -> String {
        let rev = self
            .revision
            .map(|r| format!(" at revision {r}"))
            .unwrap_or_default();
        let recorded = if self.outcome == "unchanged" {
            ", as recorded"
        } else {
            ""
        };
        let count = |a: &str| self.installs.iter().filter(|v| v.action == a).count();
        let n = self.installs.len();
        let mut out = format!(
            "EESSI {} {}{rev}{recorded}: {n} recorded install{}, {} to rebuild, {} kept, {} unaffected, {} gone\n",
            self.eessi.version,
            self.eessi.software_subdir,
            if n == 1 { "" } else { "s" },
            count("rebuild"),
            count("keep"),
            count("unaffected"),
            count("gone"),
        );
        let w = self
            .installs
            .iter()
            .map(|v| v.module.len())
            .chain(
                self.rebuild
                    .iter()
                    .filter_map(|l| Some(module_rel(l.split_whitespace().next()?)?.len())),
            )
            .max()
            .unwrap_or(0);
        let why = |v: &Verdict| {
            v.changes
                .iter()
                .map(|c| format!("{} {} ({}, {})", c.module, c.change, c.class, c.edge_kind))
                .collect::<Vec<_>>()
                .join("; ")
        };
        let by_file: BTreeMap<&str, &Verdict> = self
            .installs
            .iter()
            .map(|v| (file_name(&v.easyconfig), v))
            .collect();
        if !self.rebuild.is_empty() {
            out.push_str("rebuild, in this order:\n");
            for (i, line) in self.rebuild.iter().enumerate() {
                let first = line.split_whitespace().next().unwrap_or(line);
                match (by_file.get(file_name(first)), module_rel(first)) {
                    (Some(v), _) => out.push_str(&format!("  {:>2}. {:<w$}  {}\n", i + 1, v.module, why(v))),
                    (None, Some(m)) => out.push_str(&format!(
                        "  {:>2}. {m:<w$}  not installed: a dependency eb-stack plans for the rebuild\n",
                        i + 1
                    )),
                    (None, None) => out.push_str(&format!("  {:>2}. {line}\n", i + 1)),
                }
            }
        }
        let unordered = if self.rebuild.is_empty() {
            ("rebuild", "rebuild, in an order not known without eb-stack")
        } else {
            ("", "")
        };
        for (action, title) in [
            unordered,
            ("keep", "kept"),
            ("unaffected", "unaffected"),
            ("gone", "gone, dropped from the record"),
        ] {
            let vs: Vec<&Verdict> = self
                .installs
                .iter()
                .filter(|v| v.action == action)
                .collect();
            if vs.is_empty() {
                continue;
            }
            out.push_str(&format!("{title}:\n"));
            for v in vs {
                let line = format!("      {:<w$}  {}", v.module, why(v));
                out.push_str(line.trim_end());
                out.push('\n');
            }
        }
        out
    }
}

/// The rebuild a watch ran: its lines, its result, and the listings taken
/// before it.
pub type Rebuild<'a> = (&'a [ListLine], &'a RunResult, &'a Listings);

/// Bring the record up to date after a watch: what the rebuild built is
/// recorded, with a dependency eb-stack added to it, kept and unaffected
/// installs take the EESSI now as their baseline, gone ones are dropped,
/// and one whose rebuild did not succeed keeps its baseline and the
/// changes it was rebuilt for, so that the next watch rebuilds it again.
pub fn settle(
    p: &Planner,
    probe: &Probe,
    revision: Option<u64>,
    verdicts: &[Verdict],
    rebuild: Option<Rebuild>,
) -> Result<Vec<String>, Fail> {
    let dir = p.scope.watch_state_dir().expect("[watch] is on");
    let target = &p.scope.target.name;
    let rebuilt = match rebuild {
        Some((lines, res, before)) => built_by(p, lines, res, before)?.0,
        None => Vec::new(),
    };
    let mut recorded = Vec::new();
    if !rebuilt.is_empty() {
        let work = work_dir(p, "record")?;
        recorded = record(p, &work, probe, revision, &rebuilt, false)?;
        let _ = std::fs::remove_dir_all(&work);
    }
    update(&dir, target, |s| {
        for v in verdicts {
            match v.action.as_str() {
                "gone" => {
                    s.installs.remove(&v.module);
                }
                "keep" | "unaffected" => {
                    let Some(i) = s.installs.get_mut(&v.module) else {
                        continue;
                    };
                    for c in &v.changes {
                        match eessi::find(&probe.modules_dirs, &c.module) {
                            Some(m) => {
                                if let Some(d) = i.deps.get_mut(&c.module) {
                                    d.identity = m.identity;
                                }
                            }
                            None if c.change == "vanished" => {
                                i.deps.remove(&c.module);
                            }
                            None => {}
                        }
                    }
                    i.revision = revision;
                    i.modules_dirs = probe.modules_dirs.clone();
                }
                "rebuild" if !recorded.contains(&v.module) => {
                    if let Some(i) = s.installs.get_mut(&v.module) {
                        i.pending.clone_from(&v.changes);
                    }
                }
                _ => {}
            }
        }
    })
    .map_err(|e| Fail::new(exit::FAILED, e))?;
    Ok(recorded)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn recorded(version: &str, subdir: &str) -> Install {
        Install {
            easyconfig: "x.eb".into(),
            args: Vec::new(),
            how: "built".into(),
            run: "r".into(),
            recorded_at: "2026-11-17T09:00:00Z".into(),
            eessi_version: version.into(),
            software_subdir: subdir.into(),
            modules_dirs: vec!["/m".into()],
            revision: None,
            deps: BTreeMap::new(),
            pending: Vec::new(),
        }
    }

    #[test]
    fn other_eessi_uses_the_shared_walk_and_the_probe_subdirectory() {
        let mut state = State::new("zen4");
        state
            .installs
            .insert("A/1.0".into(), recorded("2023.06", "x86_64/amd/zen4"));
        state
            .installs
            .insert("B/1.0".into(), recorded("2025.06", "x86_64/amd/zen4"));
        state
            .installs
            .insert("C/1.0".into(), recorded("2025.06", "x86_64/amd/zen2"));
        let probe = Probe {
            version: "2025.06".into(),
            software_subdir: "x86_64/amd/zen4".into(),
            repository: None,
            modules_dirs: Vec::new(),
        };
        let names = |found: Vec<(&String, &Install)>| -> Vec<String> {
            found.into_iter().map(|(n, _)| n.clone()).collect()
        };
        let via_probe = names(other_eessi(&state, &probe));
        let via_helper = names(built_against_other(
            &state,
            &probe.version,
            Some(&probe.software_subdir),
        ));
        assert_eq!(via_probe, via_helper);
        assert_eq!(via_probe, ["A/1.0", "C/1.0"]);
        assert_eq!(
            names(built_against_other(&state, "2025.06", None)),
            ["A/1.0"]
        );
    }

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
    fn the_plan_counts_a_single_recorded_install_in_the_singular() {
        let probe = Probe {
            version: "2023.06".into(),
            software_subdir: "x86_64/generic".into(),
            repository: None,
            modules_dirs: Vec::new(),
        };
        let foo = Verdict {
            module: "Foo/1.0".into(),
            easyconfig: "Foo-1.0.eb".into(),
            args: Vec::new(),
            action: "unaffected".into(),
            changes: Vec::new(),
        };
        let text = PlanDoc::new("cpu", &probe, None, vec![foo]).text();
        assert!(
            text.starts_with("EESSI 2023.06 x86_64/generic: 1 recorded install, 0 to rebuild, 0 kept, 1 unaffected, 0 gone\n"),
            "{text}"
        );
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

    #[test]
    fn a_vanished_dependency_the_install_only_built_with_follows_the_review_policy() {
        let c = |change: &str, class: &str| Change {
            module: "M/1".into(),
            change: change.into(),
            class: class.into(),
            edge_kind: "e".into(),
        };
        for (change, class, review, rebuild) in [
            ("changed", "must-rebuild", false, true),
            ("changed", "review", true, true),
            ("changed", "review", false, false),
            ("changed", "no-rebuild", true, false),
            ("vanished", "no-rebuild", false, true),
            ("vanished", "review", false, false),
            ("vanished", "review", true, true),
            ("rebuilt", "no-rebuild", true, false),
        ] {
            assert_eq!(
                rebuilds(&c(change, class), review),
                rebuild,
                "{change} {class} {review}"
            );
        }
    }
}
