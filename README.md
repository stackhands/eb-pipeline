# eb-pipeline

A site-neutral EasyBuild pipeline driver. One per-site Tom's Obvious Minimal
Language (TOML) file describes targets, scheduler allocations, paths,
admission, modules, caches, log handling and licences. `eb-pipeline` turns that
file and a build list into an ordered set of subprocesses, so a continuous
integration (CI) system (Jenkins, GitLab CI, a cron job) becomes a thin caller
that passes a list and a target and reads an exit code and a result file.

It starts [eb-stack](https://github.com/stackhands/eb-stack) for `stack shard`,
`stack admit` and `rebuild-set`,
[eb-keep-logs](https://github.com/stackhands/eb-keep-logs) for `failed` and
`rotate`, and
[eb-staged-rebuild](https://github.com/stackhands/eb-staged-rebuild) for `run`,
and never links them in. Nothing about a site lives in this repository: the
templates carry placeholders only, and a test rejects any template value that
looks site-specific.

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

That install puts six commands in `~/.local/bin`:

```bash
for cmd in eb-pipeline eb-retry-list eb-shard-weights eb-test-map eb-junit eb-cvmfs; do
  command -v "$cmd" && "$cmd" --help >/dev/null
done
```

A release tarball ships `eb-pipeline` only; build from source for the companions.

[Install eb-pipeline](docs/orgmode/howto/install.org) has the release binaries
and what to do when the build fails.

## Licence

MIT. See [LICENSE](LICENSE).

<!-- stackhands:begin -->
## Part of stackhands

eb-pipeline is one of the [stackhands](https://github.com/stackhands) tools for building, installing and maintaining an HPC software stack with EasyBuild on top of EESSI.

| Tool | Job | Docs |
|---|---|---|
| **eb-pipeline** | Run a site's EasyBuild build lists from one TOML file through eb-stack, eb-keep-logs and eb-staged-rebuild | [README](https://github.com/stackhands/eb-pipeline#readme) |
| [eb-stack](https://github.com/stackhands/eb-stack) | Solve, shard and plan an EasyBuild build list; admit builds under a scratch inode budget | [docs](https://eb-stack.rgoswami.me/) |
| [eb-staged-rebuild](https://github.com/stackhands/eb-staged-rebuild) | Rebuild an installation in a staged tree and swap it in, so running jobs keep their paths | [docs](https://stackhands.github.io/eb-staged-rebuild/) |
| [eessi-arch-guard](https://github.com/stackhands/eessi-arch-guard) | Reload EESSI in a Slurm task that inherited it for another CPU or GPU | [docs](https://stackhands.github.io/eessi-arch-guard/) |
| [eb-local](https://github.com/stackhands/eb-local) | Install software with EasyBuild on top of EESSI into a directory a user or project owns | [docs](https://stackhands.github.io/eb-local/) |
| [eb-keep-logs](https://github.com/stackhands/eb-keep-logs) | Keep a failed build's EasyBuild log and command output; rotate a pipeline's log tree | [docs](https://stackhands.github.io/eb-keep-logs/) |
| [rfc9162](https://github.com/stackhands/rfc9162) | Append-only Merkle logs hashed as RFC 9162, with tiled storage and signed checkpoints | [docs](https://stackhands.github.io/rfc9162/) |
| [zig-common](https://github.com/stackhands/zig-common) | The shared Zig library under the Zig tools: system calls, checked files, child processes | [docs](https://stackhands.github.io/zig-common/) |
<!-- stackhands:end -->
