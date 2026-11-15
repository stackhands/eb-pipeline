# Changelog

All notable changes to this project are documented here.

## 0.1.0 - Unreleased

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
