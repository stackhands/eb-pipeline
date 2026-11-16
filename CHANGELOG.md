# Changelog

All notable changes to this project are documented here.

## Unreleased

### Changed

- The README tutorial builds `pigz-2.8-GCCcore-15.2.0.eb` on GCCcore 15.2.0,
  the compiler EESSI 2026.06 provides. Its
  `zlib-1.3.1-GCCcore-14.2.0.eb` needed GCCcore 14.2.0 built from source
  first.
- The README lists the other stackhands tools.

### Fixed

- The module wrapper sources `modules.shell_init` without `set -e` or
  `set -u`. EESSI 2026.06's Lmod init reads `EESSI_NO_MODULE_PURGE_ON_INIT`
  without a default, so with it as `shell_init` every module-wrapped step
  failed before its command started. A `module` call that fails still stops
  the step.
- With `[modules.eessi]` enabled, `EESSI_SITE_SOFTWARE_PREFIX` is the target
  install prefix: `site_prefix` when set, otherwise `filesystems.install_root`,
  the same value as `EASYBUILD_INSTALLPATH`.
- The schema text for `filesystems.log_root` names
  `<log_root>/<log-stamp>/<target>/`.
- Admission waits are capped at `max_wait_seconds` even when that is below
  `wait_seconds`.
- `plan.json` entries whose `file` is absolute or leaves the shard directory
  are refused. `stack shard` writes bare names beside `plan.json`.
- A signal records the build lines that had not started as `skipped`.
- `templates/eessi-site.toml` marks only `cpu-a` as the default target.
- Keep-failed steps run with `EASYBUILD_OUTPUT_STYLE=basic`. Without a
  terminal, EasyBuild 5 wraps the log path that `eb --last-log` prints at 80
  columns, and `eb-keep-logs failed` kept nothing unless the site had set the
  style itself.
- `[logs.rotate]` requires `compress_after_days`, and a `delete_after_days`
  that is not greater is refused. Both passed `check`, and `run` failed its
  logs phase after the builds with exit 2 from `eb-keep-logs rotate`.
- `--log-stamp` and `EB_PIPELINE_LOG_STAMP` must be a UTC time as
  `YYYYMMDD_HHMMSS`; anything else is refused with exit 2. `eb-keep-logs
  rotate` never aged a run directory of another name, and a stamp with `..` or
  `/` put the logs outside `log_root`.
- The `logs` phase runs `eb-keep-logs rotate` with `TZ=UTC`. Run directories
  are named in UTC and rotate counts ages from the local midnight of the
  name's date, so away from UTC a run's age was a day off between the two
  midnights.
- `stack shard` runs inside the module wrapper and sees what the `[modules]`
  init list sets. Without EESSI-extend's `EASYBUILD_FILTER_DEPS`, the planner
  put filtered tools such as M4, Bison and zlib into the prelude list.
- `run` leaves out an `installed_modules_dirs` tree that does not exist yet and
  logs a WARN. On a new site prefix, `stack shard` refused the missing
  `modules/all` and the run exited 2 before any build.
- `--disable-robot` in `[sources] extra_args` turns the robot off for source
  prefetch. The fetch then leaves out `--robot`, drops a line's `--robot`, and
  keeps the paths of a line's `--robot=PATH` at the front of `--robot-paths`.
  `--fetch` ignores installed modules, so with the robot an EESSI site fetched
  the sources of the whole toolchain and created empty `software/<Name>`
  directories for it. `templates/eessi-site.toml` sets `--disable-robot`.
- Each line rebuilt through eb-staged-rebuild passes its own `--run-id`,
  `<run-id>-<easyconfig stem>`. eb-staged-rebuild refuses a run id it already
  holds, so every staged line after the first in a run was refused.
- The README gives the `software_root` and `modules_root` defaults under the
  target's `site_prefix`, as `explain` resolves them. It said
  `<install_root>/software` and `<install_root>/modules`.
- `run` removes the job's reservation, `<reservations>/<job>.json`, under the
  admission lock when an admitted unit's builds end and when the run ends
  first. Before, the reservation of the last unit stayed, and without
  `live_jobs_cmd` every later job counted it against its own admission. A
  record another writer has replaced since, such as a `forecast` or `waiting`
  one, is kept with a WARN.
- `check` warns when admission is configured without `live_jobs_cmd`.
- The `build` phase is `skipped` with a note when admission refused before
  any line started, and `refused` when every line that ran was refused. It
  was `failed` in both cases although no line failed.
- `eb-pipeline --help` lists `run --dry-run` output among what stdout carries,
  as the README does.
- The README says when `run` writes the result file: never for `--dry-run`, and
  never when an input error stops it first, which leaves an older file in place.
- The README says that `run` builds its units one at a time, with no option to
  build one shard alone, and that a shard list holds input lines only, so a
  line needs `--robot` to build missing dependencies.
- With `[modules.eessi] site_install = true` and a single install path,
  `check` refuses a target that takes staged rebuilds unless it sets
  `software_root` and `modules_root`. EESSI-extend replaces
  `EASYBUILD_INSTALLPATH` when it loads, so the default roots named a tree
  EasyBuild does not write, the installed check found nothing, and staged
  rebuilds ran as in-place `eb --rebuild`. `templates/eessi-site.toml` sets
  both roots under the EESSI-extend layout.
- `explain` names the install path EESSI-extend sets, as `install_note`.
- A rebuild line on a staged target whose module file is missing under
  `modules_root` is noted on its planned build command and logged as a WARN
  when it runs with plain `eb`.
- A target's `ledger_config` overrides `[rebuild] ledger_config`, `explain`
  shows it, and `check` refuses two targets that take staged rebuilds through
  one ledger configuration. That file names one `target`, so on a site with a
  sapphirerapids and an icelake target the icelake rebuilds were recorded as
  `x86_64/intel/sapphirerapids`.
- The result file lists `phases` in pipeline order. Phases that were not
  configured came first, ahead of `preflight`.

## 0.3.0 - 2026-11-16

The first tagged release. It also carries the changes of 0.2.0 and 0.1.0,
listed below, which were never tagged.

### Added

- `init --template NAME [--out FILE]` and `init --list`: write a starter site
  file from a built-in template (`small-cluster`, `eessi-site`). An existing
  output file is refused with exit 2.
- Every command line flag falls back to an `EB_PIPELINE_<FLAG>` variable
  (`EB_PIPELINE_SITE`, `EB_PIPELINE_LIST`, `EB_PIPELINE_TARGET`,
  `EB_PIPELINE_BUILD_TYPE`, `EB_PIPELINE_RUN_ID`, `EB_PIPELINE_LOG_STAMP`,
  `EB_PIPELINE_LISTS_DIR`); the flag wins.
- `EB_PIPELINE__SECTION__KEY=value` sets one site file value over the file;
  the result is validated by the same schema. Values inside `[[targets]]` are
  not overridable.
- `EB_PIPELINE_LINT_DENY`: extra substrings the template lint refuses.

### Changed

- `examples/` is now `templates/`, and the templates are built into the binary.
- The template lint no longer names any one site's directory layout.

## 0.2.0 - not tagged; released as part of 0.3.0

Every new key is optional and its default reproduces 0.1.0 behaviour;
`schema_version` stays 1.

### Added

- `[targets.select]`: per-target line selection by a per-line build list option
  (`option`, `value`, `separator`, `when_absent`, `strip`). `plan` and `run`
  apply it; deselected lines are listed under `deselected` in the plan and the
  result, apart from failures.
  The kept lines are written to `<log_dir>/selected-<target>.list` and that file,
  not the original list, goes to `stack shard --build-list`, so deselected lines
  do not weigh in shard balancing. `plan` shows the path under `selected_list`
  and the kept count under `selected`. A target without `[targets.select]`
  shards the list as given.
- `[targets.route]` contract: `args` templates with a `--` before the phase
  command, `env`, `runtime_root`, `shares_ro`, `shares_rw`,
  `receipt_digest_env` (refused with 78 when unset or empty) and `phases`
  (`build`, `sources`; default `build`).
- `[targets.cpu_hardware]`, exported with `[targets.hardware]` as
  `EB_PIPELINE_HW_*` per command role, and `[targets.fetch_profile]`, passed to
  source prefetch as `EB_PIPELINE_FETCH_PROFILE` (JSON).
- `[targets.package_env.<Name>]`, merged over `[modules.package_env.<Name>]` per
  key for one target.
- `filesystems.export_install_vars` (`single` or `split`) and
  `filesystems.read_only_installdir_value`.
- `[caches] init_module` as a default that `[[caches.targets]]` entries override.
- `shards.installed_modules_dirs_from_env`: module trees read from environment
  variables when `run` starts; `plan` shows `${NAME}`.
- `examples/eessi-site.toml` exercises selection, the route contract, a CPU
  hardware class, a fetch profile and per-target package environment.

## 0.1.0 - not tagged; released as part of 0.3.0

### Added

- Site file schema, `kind = "site"` and `schema_version = 1`, with unknown-key
  rejection, per-target overrides, optional sections that the driver skips as
  "not configured", and `[site.tools]` minimum versions checked before any work.
- `check`, `schema`, `explain`, `plan` and `run` subcommands.
- `plan` output is deterministic JSON with sorted keys: phase order, scheduler
  launch lines, exported environment and tool argv.
- `run` executes the phases as subprocesses with admission retry and backoff,
  per-line results, a result JSON, and SIGTERM/SIGINT forwarding.
- Exit-code contract: 0, 1, 2, 3, 75, 78, 130, 143, with explicit mapping of the
  eb-stack and eb-staged-rebuild codes.
- Staged rebuild only for installed modules, whole rebuild options and an enabled
  target; `software_root` and `modules_root` per target.
- `admission.job_id_env`, `admission.default_footprint`, `plan --shard-plan`,
  `--log-stamp` and `<log_root>/<stamp>/<target>/` log directories.
- Example site files for a small Slurm cluster and an EESSI site, and a lint
  that rejects site-specific values in examples.
