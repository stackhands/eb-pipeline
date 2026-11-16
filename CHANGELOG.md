# Changelog

All notable changes to this project are documented here.

<!-- towncrier release notes start -->

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
