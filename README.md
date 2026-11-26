<p align="center">
  <picture>
    <source media="(prefers-color-scheme: dark)" srcset="branding/logo/logo-dark.svg">
    <img src="branding/logo/logo-light.svg" alt="eb-pipeline" width="420">
  </picture>
</p>

# eb-pipeline

A site-neutral EasyBuild pipeline driver. One per-site Tom's Obvious Minimal
Language (TOML) file describes targets, scheduler allocations, paths,
admission, modules, caches, log handling and licences. `eb-pipeline` turns that
file and a build list into an ordered set of subprocesses, so a continuous
integration (CI) system (Jenkins, GitLab CI, a cron job) becomes a thin caller
that passes a list and a target and reads an exit code and a result file.

It starts [eb-stack](https://github.com/stackhands/eb-stack) for stack
planning (`stack shard`, `stack admit`, `stack order`, `stack sbom`,
`stack oom`, `stack from-commit`, `stack bot-classify`, `rebuild-set`,
`package inspect`),
[eb-keep-logs](https://github.com/stackhands/eb-keep-logs) for `failed` and
`rotate`, and
[eb-staged-rebuild](https://github.com/stackhands/eb-staged-rebuild) for `run`.
It never links those three in. It does link the
[rfc9162](https://github.com/stackhands/rfc9162) library, which `verify` uses
to check an inclusion proof, and attestation starts `rfc9162-sidecar`. Nothing
about a site lives in this repository: the templates carry placeholders only,
and a test rejects any template value that looks site-specific.

## When EasyBuild or the EESSI bot is enough

On one machine, `eb --easystack LIST --robot --keep-going` builds a list in dependency order, skips what is installed and goes on past a failure.
On a cluster, `--job --job-backend=Slurm` hands each easyconfig to the scheduler as a job of its own.
For EESSI itself, the [EESSI build-and-deploy bot](https://github.com/EESSI/eessi-bot-software-layer) builds a pull request's easystack for each CPU target in the build container and stages the tarballs for CernVM-FS, where a publish is a transaction.

eb-pipeline adds what those leave to a script around them.
On a workstation, a lab server or a CI runner, `eb-pipeline init --template workstation` writes a site file with no scheduler, and a run gives:

- a log directory per line, a result file and exit codes a caller can branch on, and a JUnit report for CI ([publish a JUnit report](docs/orgmode/howto/publish-a-junit-report.org));
- a check that the machine has the memory a line needs before it starts ([check memory before building](docs/orgmode/howto/check-oom-before-building.org));
- a rebuild that does not empty an installation programs are using ([staged rebuilds](docs/orgmode/howto/staged-rebuilds.org));
- a rebuild of what an EESSI update reaches ([rebuild when EESSI changes](docs/orgmode/howto/rebuild-when-eessi-changes.org)).

On a site's own stack on a shared file system, the same list also builds on several node types, some lines on some of them only; scratch has an inode quota that a build can exhaust; and a failed build's logs live in a directory the job loses.
Each of those is a phase the site file turns on, and `plan` prints every command of a run before one starts.

## Commands and capabilities

- `check`, `schema`, `init`, `explain`, `targets`: validate a site file, print its reference, write a starter file, show how a target resolves ([the command](docs/orgmode/reference/cli.org)).
- `plan` and `run`: print or execute every command of a run, with a result file and exit codes ([the plan and result files](docs/orgmode/reference/plan-and-result.org)).
- `status` and `metrics`: list recent runs and print Prometheus or JSON status for a result ([follow runs](docs/orgmode/howto/follow-runs.org), [publish metrics](docs/orgmode/howto/publish-prometheus-metrics.org)).
- Attestation and `verify`: record the inputs of a plan in an RFC 9162 log and check the record later ([attest inputs](docs/orgmode/howto/attest-build-inputs.org)).
- SBOM: a clean run stores a CycloneDX file beside `summary.json` ([run summary](docs/orgmode/reference/run-summary.org)).
- `[notify]`: POST a run's `summary.json` to a webhook ([run summary](docs/orgmode/reference/run-summary.org)).
- Offline bundles: build from one reviewed set of source, test and runtime files ([offline bundle](docs/orgmode/howto/build-an-offline-bundle.org)).
- `watch`: rebuild the recorded installs an EESSI change reaches ([watch](docs/orgmode/reference/watch.org)).
- `audit`: stage vulnerability rebuild lists from eb-stack ([audit](docs/orgmode/howto/audit-with-eb-stack.org)).
- Resume, memory preflight and from-commit checks ([resume](docs/orgmode/howto/resume-a-run.org), [memory](docs/orgmode/howto/check-oom-before-building.org), [from-commit](docs/orgmode/howto/check-from-commit-pins.org)).
- `completions` and `man`: shell completions and a manual page.
- Companion commands: `eb-retry-list`, `eb-shard-weights`, `eb-test-map`, `eb-junit`, `eb-cvmfs` and `eb-admin-report` ([reference](docs/orgmode/reference/cli.org)).

## Documentation

<https://stackhands.github.io/eb-pipeline/>, built from the Org pages in
`docs/orgmode/`:

- Tutorial: [Run your first build list](docs/orgmode/tutorials/first-build-list.org).
- How-to guides: [install eb-pipeline](docs/orgmode/howto/install.org),
  [write a site file for your cluster](docs/orgmode/howto/write-a-site-file.org),
  [build a list on every target from continuous integration](docs/orgmode/howto/build-from-ci.org),
  [build in a directory that each job has on its node](docs/orgmode/howto/node-local-builds.org),
  [let users load what a target installs on top of EESSI](docs/orgmode/howto/site-installs.org),
  [rotate the log tree](docs/orgmode/howto/rotate-logs.org),
  [rebuild installed software through eb-staged-rebuild](docs/orgmode/howto/staged-rebuilds.org),
  [build some lines of a list on some targets only](docs/orgmode/howto/select-lines.org),
  [build what eb-stack plans](docs/orgmode/howto/from-eb-stack.org),
  [follow runs through their summaries](docs/orgmode/howto/follow-runs.org),
  [rebuild what an EESSI change reaches](docs/orgmode/howto/rebuild-when-eessi-changes.org).
- Explanation: [why one site file drives the build](docs/orgmode/explanation/design.org),
  [how a run goes through its phases](docs/orgmode/explanation/phases.org),
  [why the rotation takes two locks](docs/orgmode/explanation/log-rotation.org),
  [what the targets of a site file share](docs/orgmode/explanation/shared-paths.org).
- Reference: [the eb-pipeline command](docs/orgmode/reference/cli.org),
  [the site file](docs/orgmode/reference/site-file.org),
  [the plan and result files](docs/orgmode/reference/plan-and-result.org),
  [the run summary](docs/orgmode/reference/run-summary.org),
  [the watch state and plan](docs/orgmode/reference/watch.org),
  [the log tree and its locks](docs/orgmode/reference/log-tree.org),
  [the site file templates](docs/orgmode/reference/templates.org),
  [environment variables](docs/orgmode/reference/environment.org),
  [exit status](docs/orgmode/reference/exit-status.org), and the
  [API](docs/orgmode/reference/api.org) of the crate, from rustdoc.

```console
pixi run -e docs docbld
```

The HTML is written to `docs/build/html`.
[Contributing](docs/orgmode/contributing.org) says how to write a page, run
the tests and record a change.

## Install

Rust 1.90 or newer, in a checkout:

```bash
cargo install --locked --path . --root ~/.local
eb-pipeline --version
```

That install puts seven commands in `~/.local/bin`, one per binary the crate
builds (`eb-pipeline` and `eb-test-map` from `[[bin]]` entries, the rest from
`src/bin`):

```bash
for cmd in eb-pipeline eb-retry-list eb-shard-weights eb-test-map eb-junit eb-cvmfs eb-admin-report; do
  command -v "$cmd" && "$cmd" --help >/dev/null
done
```

Release archives `0.3.1` to `0.5.0` carry the first six of them; `eb-admin-report` comes from a source install. `0.3.0` carries `eb-pipeline` only.

[Install eb-pipeline](docs/orgmode/howto/install.org) has the release binaries
and what to do when the build fails.

## Licence

MIT. See [LICENSE](LICENSE).

<!-- stackhands:begin -->
## Part of stackhands

eb-pipeline is one of the [stackhands](https://github.com/stackhands) tools for building, installing and maintaining an HPC software stack with EasyBuild on top of EESSI.

| Tool | Job | Docs |
|---|---|---|
| [eb-stack](https://github.com/stackhands/eb-stack) | Turn conda-forge, Spack, PyPI, CRAN and Cargo recipes into easyconfigs; solve, shard and plan an EasyBuild build list | [docs](https://eb-stack.rgoswami.me/) |
| [eb-staged-rebuild](https://github.com/stackhands/eb-staged-rebuild) | Rebuild an installation beside the live one and swap it in, for a stack outside CernVM-FS that jobs read during the rebuild | [docs](https://stackhands.github.io/eb-staged-rebuild/) |
| [eessi-arch-guard](https://github.com/stackhands/eessi-arch-guard) | Reload EESSI in a Slurm task that inherited a stack loaded for another CPU or GPU, as srun from a login shell does | [docs](https://stackhands.github.io/eessi-arch-guard/) |
| [eb-local](https://github.com/stackhands/eb-local) | Build on EESSI-extend inside a Slurm job's node-local disk, compile your own code with RPATH, list and uninstall per CPU target | [docs](https://stackhands.github.io/eb-local/) |
| [eb-keep-logs](https://github.com/stackhands/eb-keep-logs) | Keep a failed EasyBuild command's own output, which --tmp-logdir does not; rotate a pipeline's log tree | [docs](https://stackhands.github.io/eb-keep-logs/) |
| **eb-pipeline** | Drive a site's EasyBuild builds from one site file, through eb-stack, eb-staged-rebuild and eb-keep-logs | [docs](https://stackhands.github.io/eb-pipeline/) |
| [rfc9162](https://github.com/stackhands/rfc9162) | Append-only Merkle logs hashed as RFC 9162, with tiled storage and signed checkpoints | [docs](https://stackhands.github.io/rfc9162/) |
| [zig-common](https://github.com/stackhands/zig-common) | The shared Zig library under the Zig tools: system calls, checked files, child processes | [docs](https://stackhands.github.io/zig-common/) |
<!-- stackhands:end -->
