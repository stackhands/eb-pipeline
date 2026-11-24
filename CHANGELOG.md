# Changelog

All notable changes to this project are documented here.

<!-- towncrier release notes start -->

## 0.5.0 (2026-11-24)

### Features

- Add `--refuse-systemic-failure` to `eb-retry-list`. The flag classifies
  failed logs and refuses output only when every failed line is one shared
  infrastructure class (command path, disk full, fetch network, or missing
  module tree). Ordinary build failures still produce retry lists.

### Fixes

- A run or a watch that a signal stopped exits 130 or 143 when its result file
  cannot be written, and no longer exits 1.
- A run stopped by SIGINT or SIGTERM while a site command of a late phase is
  running records that phase once, as interrupted, and no longer as failed too.
- Audit refuses an `eb-stack rebuild-set` result file that is not valid JSON,
  as it already does for the SBOM and component analysis files, and copies no
  rebuild list from it.
- Audit rejects empty CVE values and CVE values containing control characters.
- Avoid panic/backtrace output when `eb-pipeline init` cannot write to stderr;
  the command now exits with a controlled runtime failure instead.
- Clear the default and the configured target override variables of the European
  Environment for Scientific Software Installations before starting a command.
- Companion commands now read path arguments as byte-preserving paths via
  `args_os`, matching `eb-pipeline` behavior for non-UTF-8 file names instead of
  panicking in argument parsing.
- Decode `_ModuleTable_` with the same URL-safe base64 path as chunked
  module table exports before hashing, so equivalent exports record the
  same `module_table_sha256` and malformed singleton values are rejected.
- Explain rejects empty build types and build types containing control characters.
- From-commit checks reject empty EasyBuild versions and versions containing control characters.
- Gap checks reject empty generations and generations containing control characters.
- Isolate European Environment for Scientific Software Installations commands
  from inherited installation modes and target overrides.
- Make `watch --dry-run` fail when refusal details cannot be written to
  stdout, instead of discarding that write error.
- Preserve repeated EasyBuild amend options in memory-check easystacks.
- Readers of list, site, existing-weights and previous-metrics inputs refuse a
  FIFO promptly instead of blocking while waiting for a writer.
- Reading a directory as an input file reports "Is a directory" again, as
  it did before the shared reader, instead of "not a regular file".
- Record a compact European Environment for Scientific Software Installations
  module state in watch plans.
- Refuse a package-plan `closure.plan.json` whose `companions` key is present and
  is not an array, with exit 2 and a message naming the file.
- Refuse a shard plan whose `shards` is not an array or whose `prelude` or
  `cpu_prerequisites` is not an object, in place of building only the units
  that were left.
- Refuse unclosed patch strings instead of recording a partial patch list.
- Refuse unreadable easyblock files instead of recording a missing definition.
- Remove watch scratch work on every exit after it is created, including a dry-run stdout write failure, unless the command keeps that directory on purpose.
- Resolve nonliteral easyblock expressions before signing their class.
- Route companion stdout writes through shared util helpers so all shipped
  binaries report the same `cannot write stdout: ...` diagnostic and
  expected exit code when stdout writes fail.
- Run ends with exit 1 when `eb-stack stack sbom` exits with a status other than
  0, 130 or 143, in place of passing that status on as its own.
- Run rejects empty OOM node values and OOM node values containing control characters.
- Set `EESSI_ACCELERATOR_INSTALL` for configured accelerator targets.
- Summary writes use the shared atomic writer, which syncs the file before
  the rename. A symbolic link at the summary output path is now refused with
  an error instead of being replaced by a regular file.
- `eb-cvmfs` exits with status `2`, not `1`, when the metrics file cannot be
  prepared before any repository command starts. Status `1` now means only that
  the repository operation succeeded and the metrics write failed.
- `eb-pipeline --help` and `--version` now report stdout write failures (for
  example `/dev/full`) as runtime errors instead of exiting successfully without
  diagnostic output.
- `eb-pipeline run` now preserves `SIGINT`/`SIGTERM` exit status in
  `result.json` when a signal lands while the process is waiting on
  `.summary.lock`.
- `eb-shard-weights` rejects a succeeded build duration that cannot fit any
  whole-second weight, instead of writing the largest integer.
- `eb-test-map` rejects a FIFO passed as the result input with a prompt
  usage error instead of blocking while reading it.

### Documentation

- The README says when `eb --easystack --job` or the EESSI build-and-deploy bot is enough, and what a site stack on a shared file system needs beyond them.

### Miscellaneous

- A test pins that `eb-cvmfs` finishes with a FIFO at its metrics path.
- Duration validation for read-only result consumers is centralized in one
  helper used by eb-junit, eb-shard-weights and metrics parsing.
- Format the run_stubs wchan/stack helper block to satisfy rustfmt.
- Replace fixed sleeps in lock-contention integration tests with observable
  wait conditions and bounded deadlines.
- Synchronize the summary lock test at the lock attempt instead of relying on timeouts.
- The ci workflow runs the tests that read EB_STACK_REAL against the eb-stack 0.8.0 release.
- The summary lock unit test now proves blocking by waiting for a test-only
  post-lock-acquire signal from `record`.
- The summary-lock signal regression now writes its `SKIP:` notice directly
  to a locked stderr handle, and only after a sustained unobservable
  `/proc` wait-state window.


## 0.4.0 (2026-11-19)

### Features

- =eb-pipeline run --metrics-file= and =eb-cvmfs --metrics-file= atomically publish their final Prometheus metrics for a node exporter textfile collector while preserving an existing file's permission bits. Pipeline metrics remain independent of =--result= and refuse its destination; CVMFS metrics preserve the last successful publish and serialize invocations that share a file.
- Add an opt-in `[watch.abi_drift]` table that lets `watch` run host ABI probe commands, map changed fields to provider easyconfigs, and feed that provider set into `eb-stack rebuild-set` for selective rebuild planning.
- Add an opt-in `eb-cvmfs --verify-cmd CMD` check between synchronization and publish, aborting the transaction when verification fails.
- Add an opt-in `eb-cvmfs` `--archive-manifest` check that refuses to open a CernVM File System (CVMFS) transaction unless one directory contains exactly the expected non-empty archive, with normalized paths, explicit parent directories, and user and group ids that fit the configured integer type.
- Add opt-in `eb-cvmfs --sync-timeout SEC` and `--verify-timeout SEC` deadlines that terminate the command process group, abort the transaction, record the timed-out phase, and prevent retries and publication.
- Add opt-in `eb-cvmfs --tag`, `--tag-description`, and `--check` for named publish tags and post-publish integrity checks.
- Add opt-in checksum-indexed offline bundles for prefetched EasyBuild files and configured test or runtime assets, with eb-stack planner artifacts and a site-provided network-isolation wrapper for later phases.
- Adds `eb-pipeline audit`, an opt-in command that runs eb-stack vulnerability subcommands and copies generated rebuild easystacks to `--out-dir/rebuilds/*.yml` for `eb-pipeline run --list`.
- The JUnit tool adds custom suite properties when requested.
- The `eb-cvmfs` command bounds integrity checks with an optional deadline.
- The `eb-cvmfs` command bounds publishing with an optional deadline.
- The `eb-junit` command returns 1 for failing reports when requested.
- The `status` command lists only problematic runs when requested.
- The retry-list command reports empty selections through its status.
- The retry-list tool reuses the recorded build list when requested.
- The run command writes its execution plan when requested.
- The shard-weights tool requires a minimum sample count when requested.
- The test-map tool includes resumed lines when requested.
- The verify command reuses a recorded list path when requested.
- `[attest]` records the exact inputs of a finalized plan in an rfc9162 sidecar log. `plan` and `run` append that record when the table is enabled, and `verify` recomputes it and checks the inclusion path. Without the table, a plan and a result stay unchanged.
- `[targets.select]` takes `aliases`, further tokens that name the target such as a group name shared by several targets, and `exclude_option`, a second option whose tokens leave a line out for the target. An exclusion takes priority over an inclusion, and `strip` removes both options.
- `check --gap-out DIR --generation GEN` delegates site-layer gap analysis to `eb-stack stack gap`; `--shadow` also writes the `stack shadow` JavaScript Object Notation (JSON) report.
- `check --stack-checks buildenv` runs `eb-stack stack buildenv --build-list FILE --strict` and fails while a toolchain of a plain build list has no `buildenv` line. `provided` and `buildenv` can be requested together. Needs eb-stack 0.9.0 or newer.
- `check --stack-checks` can delegate selected build-list checks to the configured `eb-stack` before any scheduler allocation.
- `eb-junit` now accepts `--reframe-report FILE` and folds ReFrame `--report-file` JSON testcases into extra JUnit suites in the same XML output.
- `eb-pipeline check --list FILE --check-from-commit` is opt-in. It runs
  `eb-stack stack from-commit --json` with JavaScript Object Notation (JSON)
  output on an easystack before a pipeline run.
  Continuous integration (CI) can then refuse stale pins already in the EasyBuild
  release tag.
- `eb-pipeline init --hints` and `EB_PIPELINE_INIT_HINTS=1` print next steps on stderr after writing a template, including `targets --site` for `eessi-site` and `small-cluster`. `docs/examples/init-hints.sh` checks the lines.
- `eb-pipeline run --resume-result FILE` continues a run whose `[resume]` table is enabled. It asks `eb-stack stack order --hashes-out` for the input identity, refuses when that digest differs from `FILE`, and skips a successful prelude or shard only when every module file that unit declared is still present. A requeued Slurm job uses a workspace directory qualified by `filesystems.configs_repo`. Without `[resume]`, `run` is unchanged.
- `eb-retry-list --classify --class CLASS,...` keeps unfinished lines whose `eb-stack stack bot-classify --json` `class` or `sub_class` is one of those names. Without `--class`, the command still keeps `interrupted`, `resource` and `timeout`. `--node CORES:MEMORY_MIB[:TARGET]` asks `eb-stack stack oom` about the lines that remain, warns on standard error when one may run out of memory, and `--drop-oom` leaves those lines out. `--easyconfigs` and `--arch` are passed through to that command. The new options do nothing unless they are set, and each of them needs `--classify`.
- `eb-test-map --format eessi-modules` prints one `reframe --setvar modules=Name/Version,...` argument for the applications in a build list. The European Environment for Scientific Software Installations (EESSI) test suite reads that comma-separated list in `set_modules`. `--map` is not required for this format. The default output, one test name per line, is unchanged.
- `eb-test-map` now reads `--list` as either a plain build list or a `.yml`/`.yaml` easystack, accepts `--result` to map only `status == "success"` lines from a run result, and offers `--format reframe` to print escaped `-n ^...$` selectors.
- `plan` and `run` can take an eb-stack package-plan bundle, build its emitted recipes in declared order, and record the SHA-256 of `package.plan.json`. An explicit verification option runs `pip check` for PyPI products and an installed R DESCRIPTION dependency check for CRAN products.
- eb-pipeline now generates bash, zsh and fish completion scripts and man pages for every binary in a release from their live command help during CI and release packaging.

### Fixes

- ABI drift checks preserve gone installs so watch can drop their stale records.
- Accept `--archive-manifest=PATH` in `eb-cvmfs`, matching the `=` syntax already supported for the other value options.
- Accept `-V` and `--version` on `eb-test-map`, printing `eb-test-map <version>` on stdout and exiting `0` like its sibling binaries.
- Accept an ingest directory supplied through a symbolic link.
- Admission retries retain the last valid JSON report when later output is malformed.
- Allow a caches table to omit `targets`, using the documented empty-list default.
- Apply `patches =` replacement and `patches +=` extension in execution order.
- Archive manifests reject tar numeric fields with data after their terminator.
- Attest patch literals only from uncommented `patches` assignments.
- Attest the last easyblock assignment in an easyconfig, matching Python execution.
- Attestation reports an unavailable helper executable as a usage error.
- Audit preserves SIGINT and SIGTERM exit codes from eb-stack subprocesses.
- CVMFS value options reject a following long option.
- Companion value options report a missing value instead of consuming the next long option.
- Compare resolved module trees before skipping a watch assessment.
- Complete the truncated `watch` exit status `1` sentence in `WATCH_EXIT` (`eb-pipeline watch --help`) and `docs/orgmode/reference/cli.org`.
- Concatenate adjacent patch string literals before recording attested paths.
- Derive missing metrics counts from the result's recorded lines instead of reporting every status as zero.
- Easystack diagnostics point to the repeated key.
- Easystack errors name the source line and column.
- Easystack parsing applies YAML merge keys before reading options.
- Easystack-only stack checks reject plain build-list paths before starting eb-stack.
- Example lint accepts placeholder URLs whose authority contains user information.
- From-commit checks report an unavailable eb-stack executable as a usage error.
- Gap checks report an unavailable eb-stack executable as a usage error.
- Ignore an initial UTF-8 byte-order mark in plain build lists.
- Ignore unrelated non-Unicode environment values while loading site overrides.
- Include the source archive in release build-provenance attestations alongside the binary archives.
- Input attestation finds an explicit easyblock assignment after other easyconfig settings.
- Input attestation reports a helper signal death with its conventional exit status.
- Input attestation writes records through unique temporary files that tolerate stale writer state.
- Interrupted source prefetch records the active line and every unbuilt line.
- Let standalone command help and version output exit cleanly when a pipeline consumer closes early.
- Licence thresholds apply their group when a package version is not dotted numbers.
- Metrics reject result counts whose status is outside the result schema.
- Module scanning reads quoted Lua and Tcl root paths without retaining quote characters.
- Name the full `eb-stack stack admit` and `eb-stack stack shard` commands in CLI exit-code help.
- Offline bundles reject conflicting files that share one consumer locator.
- Output commands protect input files from aliased destinations.
- Output writers replace completed files atomically.
- Package plans reject duplicate build-order recipes instead of building one recipe twice.
- Package plans reject verification recipes that are absent from the build order.
- Plan and dry-run commands exit cleanly when an output consumer closes.
- Plan and run reject input paths that JSON cannot represent without changing their bytes.
- Preserve a separately written EasyBuild robot path when source prefetch disables dependency resolution.
- Preserve one-letter EasyBuild options when generating an easystack for memory checks.
- Recognize uppercase `.YML` and `.YAML` build-list extensions as easystacks, matching the check commands.
- Record the aggregate build phase as pending when a staged rebuild waits for a ledger event.
- Refuse duplicate `--map`, `--list`, `--result`, and `--format` options in `eb-test-map` with exit status `2`.
- Refuse duplicate `--result`, `--summary`, `--target`, `--list`, and `--out` options in `eb-retry-list` with exit status `2` instead of silently overwriting earlier values.
- Refuse duplicate single-value options (`--log-root`, `--last`, `--target`, `--existing`, `--min-samples`, `--mode`, and `--out`) in `eb-shard-weights` with exit status `2`.
- Refuse invalid JSON emitted by `eb-stack stack from-commit`.
- Refuse invalid JSON from `eb-stack stack shadow`.
- Refuse retry inputs whose easyconfig paths disagree instead of dropping outcomes.
- Refuse special files before reading the test map.
- Refuse successful summary packages without valid shard-weight durations.
- Refuse symbolic-link destinations for atomic file outputs.
- Refuse unreadable completed summaries instead of emitting partial shard weights.
- Reject an empty CycloneDX file from a successful SBOM command instead of reporting that it was written.
- Reject duplicate phase names before rendering contradictory status series.
- Reject empty or control-filled build types and run identifiers.
- Reject environment variable names that process launchers cannot export.
- Reject malformed JSON written by a successful SBOM command.
- Reject mixing the `provided` stack check with checks that require an easystack, instead of passing one file as both incompatible input formats.
- Reject package-plan recipe and easyblock paths that escape the bundle through symbolic links.
- Reject release archives in which a declared binary is present but lacks executable permissions.
- Reject selector values made only of separators instead of silently deselecting the build-list line.
- Remove a newly created site template when writing or syncing it fails.
- Report `from-commit` standard-output write failures instead of returning success after losing its JSON report.
- Report malformed documentation URLs as link errors instead of stopping the whole site check.
- Resume workspace names distinguish repository paths that differ by separators.
- Retry errors name the source build list.
- Route standard output writes across `eb-pipeline` subcommands (`explain`, `targets`, `status`, `metrics`, `init`, `watch --dry-run`, `verify`, and `schema`) through `util::write_stdout` so early closed pipes exit `0` instead of panicking with exit `101`.
- Routed commands create phase directories referenced only by route environment values.
- Run each package-plan verification once when closure references share an emitted recipe.
- Run plans and dry runs use supplied shard units without replanning them.
- Run signals now reach every process in the active phase command.
- Runs preserve a summary that does not parse instead of replacing every target.
- SBOM generation preserves signal exit codes.
- SBOM generation refuses success when eb-stack leaves only a stale document.
- Saved shard plans without build units fail instead of completing without builds.
- Shard execution atomically materializes selected and easystack build lists.
- Shard plan loading rejects duplicate unit names before commands or logs collide.
- Shard plan loading rejects shard entries that omit their build-list file.
- Shard weights refuse duplicate modules in an existing weights file.
- Share the EESSI template's routed build work directory as writable so CPU prerequisite builds can use it.
- Site validation rejects duplicate shard module paths and variable names.
- Stabilize the shared `eb-cvmfs` metrics-lock test.
- Stack checks preserve subprocess exit statuses for pipeline callers.
- Status reports when a selected target has not written its summary entry yet.
- Target selectors reject an empty equals-form value.
- Target selectors reject missing values.
- Target-filtered status output derives its run status from the selected target.
- Test stubs now use a shared executable writer and synchronized command launch path with bounded ETXTBSY retries to stabilize parallel test runs.
- The JUnit companion now uses EB_PIPELINE_RESULT when no input flag is present.
- The documentation checker reports invalid forbidden patterns as usage errors.
- The documentation checker resolves root-relative links within the built site.
- The easystack reader treats floating-point one as a bare option.
- The from-commit check now preserves the conventional exit status when its eb-stack process dies from a signal.
- The init command now reports output filesystem errors with exit status 1.
- The metrics output option rejects the argument separator as a file path.
- The planner keeps leading-dash easyconfig names as robot-searchable operands.
- The retry command now preserves each selected line's EasyBuild options during memory checks.
- The retry companion now uses common pipeline input variables when their flags are absent.
- The retry-list command protects its input files from output writes.
- The run command rejects dry-run resume combinations.
- The shard weights command now rejects a target that matches none of its inputs.
- The shard weights command now rejects successful result lines with invalid durations.
- The shard weights companion now uses common result and target variables when their flags are absent.
- The test map command now parses per-line EasyBuild options in plain build lists.
- The test map companion now uses common list and result variables when their flags are absent.
- The test map companion prefers result input when both pipeline input variables are set.
- The watch command writes assessed plans when rebuild preparation stops.
- Tool version checks reject a command that prints a version and exits nonzero.
- Treat `BrokenPipe` on standard output writes as clean exit `0` across `eb-junit`, `eb-retry-list`, `eb-shard-weights`, `eb-test-map`, and `eb-cvmfs --dry-run`.
- Treat a closed gap-report output consumer as successful after the report files have been written.
- Treat only a parent path component as leaving the built documentation site, not a file name that begins with two dots.
- Unbracketed IPv6 webhook addresses are rejected before notification delivery.
- Use the final module-file root assignment for watch identity and easyconfig discovery.
- Watch preserves an install that another run records while a rebuild is in progress.
- Webhook validation rejects URLs whose authority contains only a port.
- `eb-junit` preserves distinct testcase names when XML character removal makes their recorded names equal.
- `eb-pipeline --help` now lists the companion commands `eb-retry-list`, `eb-shard-weights`, `eb-test-map`, `eb-junit` and `eb-cvmfs`, `docs/examples/tutorial.sh` builds the binary when a fresh checkout has no `target/debug/eb-pipeline` yet, and a `rust-toolchain.toml` pins the local toolchain to `1.90.0` to match CI.
- `eb-pipeline audit` refuses a malformed SBOM from `eb-stack stack sbom` before component analysis.
- `eb-pipeline audit` refuses a malformed `component-reach.json` from `eb-stack component-reach`.
- `eb-retry-list` leaves verified resume-skipped lines out of its default retry selection.
- eb-cvmfs reports 128 plus the signal for terminated commands.

### Documentation

- A how-to records a plan's inputs in the sidecar log and checks the inclusion path. An explanation says why the record covers the resolved files rather than a commit.
- Clarify that an ingest directory may be a symbolic link while its archive entry may not.
- Document `EB_PIPELINE_RESULT` input fallback for `eb-junit` in `junit.org`.
- Document `EB_PIPELINE_RESULT`, `EB_PIPELINE_INIT_HINTS`, `EB_PIPELINE_WORKSPACE_REPO`, `EB_PIPELINE_CHECK_*`, and `EB_PIPELINE_LINT_DENY` in `reference/environment.org`.
- Document `audit` exit statuses (`0`, `1`, `2`, `75`, `78`) in `exit-status.org` and `cli.org`, note the hidden `completions` and `man` packaging subcommands, and update `run-summary.org` examples to `eb-pipeline 0.3.1`.
- Document `init --hints` (`EB_PIPELINE_INIT_HINTS`), update `audit` synopsis and `0.3.1` version output, and cross-reference `eb-test-map` and `eb-cvmfs` in `reference/cli.org`.
- Document default omission of `resumed:` skipped lines by `eb-retry-list` in `cli.org`.
- Fix the `v0.3.0` release contents in `howto/install.org`, `docs/examples/install.sh`, and `README.md`, and update source-build `--version` output to `0.3.1`.
- The CLI reference and a how-to describe selecting and configuring pre-allocation eb-stack checks.
- The README and documentation carry the stackhands logo and the table of the other tools.
- The how-to and command/result references explain how to build an eb-stack package-plan bundle, opt in to Python or R consistency verification, and interpret its digest and verification logs.
- Update `docs/examples/init-hints.sh` to match the full target-hint message printed by `eb-pipeline init --hints`.
- Updated the `eb-junit` how-to guide to show the actual `--result or --reframe-report is required` error message when neither input option is provided.
- Updated the `eb-test-map` how-to guide to document `--allow-unmapped` for `--format reframe`, `--result` alongside `--list`, and exact error messages for missing inputs, unknown `--format`, and malformed list entries.
- `docs/orgmode/howto/resume-a-run.org` explains `run --resume-result`, and the command, result, exit status and phase pages name `[resume]`, `input_digest` and `outputs`.

### Miscellaneous

- A release is cut with `cog bump`: `cog.toml` runs `tools/release-prepare`, which builds the changelog and sets the version wherever the release workflow or a download command names it. `RELEASING.md` has the steps.
- Add one nonblocking regular-file reader for hostile CLI inputs.
- Main command handlers share validation for user-provided option values.
- Read JUnit source JSON through the shared subprocess output helper.
- Read metrics result JSON through the shared subprocess output helper.
- Read shard-weight result JSON through the shared subprocess output helper.
- Read shard-weight summary JSON through the shared subprocess output helper.
- Read verification plan JSON through the shared subprocess output helper.
- Read watch rebuild-set JSON through the shared subprocess output helper.
- Read watch shard plan JSON through the shared subprocess output helper.
- Subprocess JSON files share one reader that names the producer and path on a parse failure.
- The CVMFS tests write stub executables atomically and serialize test forks so concurrent test runs execute stubs reliably.


## 0.3.1 (2026-11-17)

### Features

- A `[notify]` table makes each run POST its run directory's `summary.json` to a webhook once it has written it: `webhook`, or `webhook_env` naming a variable that holds the URL, `when = "always"` or `"problems"`, `timeout_seconds` and `curl`. Without the table nothing is sent. The URL is treated as a secret: eb-pipeline prints only its scheme and host, and curl reads it on standard input. A POST that fails is a warning and leaves the run's exit code as it was.
- A `[sbom]` table makes a clean `run` start `eb-stack stack sbom --lock` and keep the CycloneDX document as `sbom.cdx.json` beside `summary.json`. A build that did not exit 0 does not ask. A command that fails, or writes no document, fails the run and leaves no file. eb-stack reads the lock; eb-pipeline does not compute the bill of materials.
- A result file can be read into Prometheus text and a short JSON status. The gauges are `eb_pipeline_info`, `eb_pipeline_exit_code`, `eb_pipeline_lines` and `eb_pipeline_phase`. Log paths stay in the result file.
- A run with a `[watch]` table records each install it built against the EESSI its module environment loads, in `<state_dir>/<target>.json`: the EESSI version and CPU subdirectory, the CVMFS revision, and each EESSI module `eb-stack rebuild-set` says the install depends on, with its class and an identity that changes when the module is rebuilt or its module file changes. A record that fails is a warning and leaves the run's exit code as it was.
- A site file can name a test command. The plan runs it after the build, or only when the list rebuilds an installed module.
- Every `run` but `--dry-run` adds its target to `summary.json`, schema version 1, and `summary.html` in its run directory, `<log_root>/<log-stamp>/`: for the target and each of its phases and packages, the status, exit code, duration and the reason it came to that status, and each package's log. Targets that share a log stamp share the two files, which each run updates under `.summary.lock` in the run directory. The page has no script and fetches nothing. A summary that cannot be written is a `WARN` line and leaves the exit status as it was.
- The result file records, as `duration_seconds`, how long the run took, how long each line's command ran, and how long each phase's commands ran together. A phase that ran no command, such as `preflight`, has none.
- `--list` takes an EasyBuild easystack, a `.yml` or `.yaml` file such as `eb-stack rebuild-set`, `eb-stack stack order --easystack-out` and `eb-stack stack easystack` write, and builds each of its `easyconfigs` entries as a line: the easyconfig, with `.eb` added to a name without it, and its `options` as `eb` options, as EasyBuild 5 writes them, with the bare option for `true`, no value or a plain `1`. An entry `eb --easystack` would not build as written, such as an `option` key for `options` or a number with a point without quotes, is refused with exit 2, naming the file and the entry. With `[shards]`, the run writes the lines to `easystack-TARGET.list` in its log directory, which `stack shard` reads. A how-to guide says which eb-stack command writes which file, in which order, and what each entry becomes.
- `[filesystems] job_dir` names a directory that each Slurm job has to itself on its node, such as the node-local mount a prolog makes, with `{job}` for the job id and `{user}` for the job's user, and `{job_dir}` in `build_path` and `tmp` puts them in it. The nightly then builds in the same per-job mount that eb-local builds in, and no longer on shared scratch. Each job resolves the directory before its command starts, from `SLURM_JOB_ID` and `SLURM_JOB_USER`, exports it as `EB_PIPELINE_JOB_DIR`, and exits 1 when the job cannot write to it. `check` refuses a `job_dir` without `{job}` or without Slurm, `{job_dir}` in the other paths, and `{work_dir}` in a route when `build_path` has `{job_dir}`. A how-to guide shows the setup, and why `tmp` stays on scratch when failed builds keep their logs.
- `eb-cvmfs` publishes a configured cache synchronization inside a CVMFS transaction and force-aborts the transaction when synchronization fails.
- `eb-junit` writes a JUnit XML report from one or more `eb-pipeline run --result` files, one suite per file and one case per recorded line, without changing `run`.
- `eb-pipeline metrics --result FILE` prints the Prometheus text of a result file, or `--format json` for a one-run status. It exits 0 when the file can be read, including when the run failed, and 2 when the file is not a result.
- `eb-pipeline run --oom-preflight --node NODE` asks `eb-stack stack oom` about the planned packages and skips the build when its `oom` count is greater than zero.
- `eb-pipeline run --shard-plan DIR` reuses saved `eb-stack stack shard` output, so an inspected plan can be executed without replanning.
- `eb-pipeline status --site FILE [--target T] [--last N] [--format text|json]` prints the newest N runs under the log root, 10 by default, from their `summary.json`: a line per run with its status, exit code and `summary.html`, and a line per target with its command, status, exit code, duration, packages per status, and the phases that were neither `ok` nor `not configured`, such as `logs skipped (exit 75)`. A run directory without a summary says so. `--target` keeps the runs of one target, and `--format json` prints one JSON document.
- `eb-pipeline targets --site FILE` prints the site file's target names in file order, one per line, or with `--format json` as a JSON array on one line, so a CI system can run one job per target. An invalid site file exits 2 with nothing on stdout.
- `eb-pipeline watch` compares a target's record with the EESSI its module environment loads now and rebuilds only the installs a change reaches: one whose EESSI dependency vanished, or changed where `eb-stack rebuild-set` classes it `must-rebuild` or `review`, and the recorded installs that depend on one it rebuilds. `eb-stack stack shard` gives the order, and the rebuild runs through the target's phases, each line with the options the record keeps for it and `--rebuild`, whatever the target's `[targets.select]`. While the CVMFS revision is the recorded one it compares nothing. `--dry-run` prints the plan, `--plan-out` writes it as JSON, `--adopt` records what is installed, and a new EESSI version is refused rather than mixed into the stack.
- `eb-pipeline watch` exits 75 when `REPOSITORY/versions` holds a `YYYY.MM` directory newer than the `EESSI_VERSION` the module environment loads. It only detects that directory: it does not ask eb-stack for a rebuild set, and it rebuilds nothing. `watch --adopt` still records the loaded generation.
- `eb-retry-list --classify` asks `eb-stack stack bot-classify` to keep only unfinished lines whose logs show an interrupted, resource or timeout failure.
- `eb-retry-list` extracts the unfinished lines (`failed`, `refused`, `pending`, `skipped`, or unreached after a non-zero exit) of a `--list` build list or easystack from `eb-pipeline run --result` or `summary.json` (`--summary` with `--target`), preserving original list order and per-line `eb` options.
- `eb-shard-weights` aggregates `duration_seconds` of succeeded `build` lines from `--result`, `--summary`, or recent `--log-root` runs into a sorted `MODULE<TAB>SECONDS` table for `eb-stack stack shard --weights` and `[shards] weights`.
- `eb-test-map --map FILE --list FILE` prints the tests that cover the applications in a build list. A toolchain is skipped. An application with no row exits 2 and prints nothing, which fails a `[checks]` command before the caches phase runs.
- `run` starts the checks command after a clean build and records a non-zero status as a failed phase. A failed build does not start it, and a failed check does not start the cache command.
- `{target}` in `[filesystems] build_path`, `[filesystems] source_paths` and `[rebuild] state_root` expands to the target name, so targets that build at once get their own `EASYBUILD_BUILDPATH`, `{work_dir}`, source download directory and eb-staged-rebuild state root. `check` names the known placeholders when it refuses another one, refuses `{target}` in the other `[filesystems]` paths and the target names `.` and `..`, and refuses a `{target}` state root for two staged targets that share a software root. `explain` shows each target's `state_root`.

### Fixes

- A line that ran through eb-staged-rebuild carries a `note` naming it, whatever its status. A successful staged line read like a plain `eb` build.
- A rebuild line on a staged target that ran as plain `eb`, because its module file was missing, is recorded as plain `eb` even when the build installs that module file. The result file said it `ran through eb-staged-rebuild`, against the run log's WARN, and a failing build's exit 2 or 75 was read as an eb-staged-rebuild refusal (78) or pending ledger event.
- A rebuild line on a staged target whose module file is missing under `modules_root` is noted on its planned build command and logged as a WARN when it runs with plain `eb`.
- A signal records the build lines that had not started as `skipped`.
- A staged line no longer waits with no deadline for another eb-staged-rebuild's lock on its state root: `run` passes `--lock-timeout` from the new `[rebuild] lock_timeout`, 600 seconds by default and 0 to refuse at once. A line whose wait runs out is recorded as `refused`, and its note and the ERROR line name the lock.
- A staged line refused on a held lock, with `[rebuild] state_root` unset and the lock away from the state root eb-pipeline expects, has a note that says `rebuild.lock_timeout` ran out if that is the state root's lock, and that eb-pipeline cannot see a state root that a module sets in `EB_STAGED_REBUILD_STATE_ROOT`. The note named the lock alone, as it still does for a lock other than the state root's when `state_root` is set.
- A target's `ledger_config` overrides `[rebuild] ledger_config`, `explain` shows it, and `check` refuses two targets that take staged rebuilds through one ledger configuration. That file names one `target`, so on a site with a sapphirerapids and an icelake target the icelake rebuilds were recorded as `x86_64/intel/sapphirerapids`.
- Admission waits are capped at `max_wait_seconds` even when that is below `wait_seconds`.
- An `eb-keep-logs rotate` that exits 75 leaves the `logs` phase `skipped`, with a note that names `<log_root>/.eb-keep-logs-rotate.lock`, and the run's exit code as it was. An eb-keep-logs that takes a lock of its own exits 75 when another rotate of the log root holds that lock past `--lock-timeout`, having changed nothing, and the run recorded `logs` as `failed` and exited 1 although nothing had gone wrong. Any other non-zero exit of `rotate` is still `failed`.
- An easystack option, a stripped build list line and a watch rebuild line are written as list words, so a parenthesis or a star stays bare and `stack shard`, which splits on white space and keeps quotes, still sees the option. With `[shards]`, a line whose words are not what that split returns, because a word holds a space or a quote, is refused; without `[shards]` that argument is kept.
- Avoid a needless `Option::as_deref()` in the EESSI generation test so clippy passes on current stable Rust.
- Each line rebuilt through eb-staged-rebuild passes its own `--run-id`, `<run-id>-<easyconfig stem>`. eb-staged-rebuild refuses a run id it already holds, so every staged line after the first in a run was refused.
- Keep-failed steps run with `EASYBUILD_OUTPUT_STYLE=basic`. Without a terminal, EasyBuild 5 wraps the log path that `eb --last-log` prints at 80 columns, and `eb-keep-logs failed` kept nothing unless the site had set the style itself.
- Release archives now include every binary the workspace builds, and the release workflow fails if an archive misses any binary declared in `.github/release-binaries.txt`.
- The EESSI site template puts NVIDIA software and module roots below `<accel_target>`, matching the path EESSI-extend selects when `EESSI_ACCELERATOR_INSTALL` is set.
- The `build` phase is `skipped` with a note when admission refused before any line started, and `refused` when every line that ran was refused. It was `failed` in both cases although no line failed.
- The `eessi-site` template no longer sets `modules.unuse_path`, which emptied MODULEPATH after the shell init so the EESSI module could not load. `check` warns when `unuse_path` is set and the first `modules.init` entry is not an absolute path.
- The `logs` phase runs `eb-keep-logs rotate` with `TZ=UTC`. Run directories are named in UTC and rotate counts ages from the local midnight of the name's date, so away from UTC a run's age was a day off between the two midnights.
- The `shard` phase note of an unsharded plan names the exit code the planner returned, the configured `shards.planner_exit_unsharded`. It said 3 whatever the setting.
- The `small-cluster` template gives all five roles `-N 1 --ntasks-per-node=1`. Inside an allocation srun otherwise starts one task per allocated task.
- The executable watch example limits its newer EESSI version to the version-switch scenario, so unrelated watch commands are not stopped by a generation notice.
- The exit code table in `--help` and the README no longer lists 3, which `run` never returns, and says that 78 also follows an admission or eb-staged-rebuild refusal after earlier lines built. A signal (130, 143) or a usage error (2) ends the run with that code ahead of the 1, 78, 75, 0 order.
- The module wrapper gives each name of `modules.init` to `module load` as written. It split `EB_PIPELINE_MODULES` with pathname expansion on, so a name with a `*`, `?` or `[` that matched files, in the directory the command started in or under an absolute path, loaded the names of those files instead. `modules.shell` is documented as a POSIX shell, such as `bash` or `sh`, which the wrapper has always needed.
- The module wrapper sources `modules.shell_init` without `set -e` or `set -u`. EESSI 2026.06's Lmod init reads `EESSI_NO_MODULE_PURGE_ON_INIT` without a default, so with it as `shell_init` every module-wrapped step failed before its command started. A `module` call that fails still stops the step.
- The result file lists `phases` in pipeline order. Phases that were not configured came first, ahead of `preflight`.
- The schema says that `[targets.select] option` includes the leading dashes, and `check` suggests the dashed form when they are missing. The schema called it an option name, which reads as `for`, a value `check` rejects.
- The schema text for `filesystems.log_root` names `<log_root>/<log-stamp>/<target>/`.
- When the job directory wrapper of `filesystems.job_dir` stops the job of `eb-stack rebuild-set`, as when the job's node has no directory for it, the warning of a record and the error of `watch` quote the first line in which the wrapper says why, after the path of the job's log, as they do for the probe. Before, they gave the path only.
- When the job directory wrapper of `filesystems.job_dir` stops the job of `eb-stack stack shard` that `watch` runs to order a rebuild, as when the job's node has no directory for it, the error of `watch` quotes the first line in which the wrapper says why, after the path of `order.log`, as it does for the probe and for `rebuild-set`. Before, it gave the path only.
- When the job directory wrapper of `filesystems.job_dir` stops the probe of a target's module environment, as when the job's node has no directory for it, the error of `watch` and the warning of a record quote the first line in which the wrapper says why, after the path of `eessi-probe.log`. Before, they gave the path only.
- With `[logs.rotate]`, a run holds `<log_root>/.eb-pipeline-rotate.lock` shared from its start, and rotates only when it can take that lock alone. eb-keep-logs 0.2.1 takes no lock: two targets that ended at the same moment rotated at once and lost each other's updates to a run's `.licensed-logs`, which then listed compressed logs under their old names instead of their `.gz` names, and a rotation compressed the logs of another target's run that was still writing them once its log stamp was a day old in UTC. A run that ends while another run under the log root is live or rotating now records `logs` as `skipped` and leaves the rotation to a run that ends alone; a run that starts while another rotates waits for it.
- With `[modules.eessi] site_install = true` and a single install path, `check` refuses a target that takes staged rebuilds unless it sets `software_root` and `modules_root`. EESSI-extend replaces `EASYBUILD_INSTALLPATH` when it loads, so the default roots named a tree EasyBuild does not write, the installed check found nothing, and staged rebuilds ran as in-place `eb --rebuild`. `templates/eessi-site.toml` sets both roots under the EESSI-extend layout.
- With `[modules.eessi]` enabled, `EESSI_SITE_SOFTWARE_PREFIX` is the target install prefix: `site_prefix` when set, otherwise `filesystems.install_root`, the same value as `EASYBUILD_INSTALLPATH`.
- With `[modules.eessi]` on, `cpu_target` must be an EESSI CPU subdirectory (`x86_64/amd/zen4`, `aarch64/neoverse_v1`) and `accel_target` an EESSI accelerator subdirectory (`accel/nvidia/cc90`, `accel/amd/gfx90a`). A short `cpu_target` such as `amd/zen4` passed `check`, then the EESSI module failed its architecture check at load time. The `eessi-site` template uses the full forms.
- `--disable-robot` in `[sources] extra_args` turns the robot off for source prefetch. The fetch then leaves out `--robot`, drops a line's `--robot`, and keeps the paths of a line's `--robot=PATH` at the front of `--robot-paths`. `--fetch` ignores installed modules, so with the robot an EESSI site fetched the sources of the whole toolchain and created empty `software/<Name>` directories for it. `templates/eessi-site.toml` sets `--disable-robot`.
- `--log-stamp` and `EB_PIPELINE_LOG_STAMP` must be a UTC time as `YYYYMMDD_HHMMSS`; anything else is refused with exit 2. `eb-keep-logs rotate` never aged a run directory of another name, and a stamp with `..` or `/` put the logs outside `log_root`.
- `[logs.rotate]` requires `compress_after_days`, and a `delete_after_days` that is not greater is refused. Both passed `check`, and `run` failed its logs phase after the builds with exit 2 from `eb-keep-logs rotate`.
- `check` refuses a route that still names the per-job directory when `build_path` uses `{job_dir}`, including `{work_dir}` that arrives through `{runtime_root}` or is left in the route command. The route starts before the job, so that path is not a directory yet.
- `check` warns when admission is configured without `live_jobs_cmd`.
- `check`, and every command that loads the site file, refuses a `modules.shell` that cannot run the module wrapper. It accepts `sh`, `bash`, `dash`, `ash`, `ksh`, `ksh93`, `mksh` and `yash`, by name or by path. zsh ran the wrapper without splitting `EB_PIPELINE_MODULES`, so the modules of `modules.init` loaded as one name, and csh, tcsh and fish could not parse it. A site file that names zsh and loads one module, which worked, must name one of those shells.
- `eb-keep-logs rotate` gets `--lock-timeout 0` where `eb-keep-logs --help` lists the option, so a run that rotates while another rotate holds `<log_root>/.eb-keep-logs-rotate.lock` records `logs` as `skipped` at once. Without it, eb-keep-logs waited up to 600 seconds for that lock while the run held `<log_root>/.eb-pipeline-rotate.lock` alone, and every run under the log root that started meanwhile waited for it at its start. eb-keep-logs 0.2.1 refuses `--lock-timeout`, and the builds that take it print the same version, so the rotation leaves the option out, with a note on the `logs` phase, when `--help` does not list it or fails.
- `eb-pipeline --help` lists `run --dry-run` output among what stdout carries, as the README does.
- `eb-pipeline schema` lists all five placeholders that `targets.route.shares_ro` and `shares_rw` take: `{phase_dir}`, `{log_dir}`, `{work_dir}`, `{target}` and `{phase}`. It left out `{target}` and `{phase}`, which `check` accepts and a run replaces in the shares as it does in the route's `args`.
- `eb-pipeline schema` says which commands get the hardware class of `[targets.cpu_hardware]`: the source prefetch and build commands of each line, which build for one class of node, as they always have. It said that every command did, and did not say that `[targets.hardware]` is exported only when `cpu_hardware` is set.
- `explain` names the install path EESSI-extend sets, as `install_note`.
- `plan.json` entries whose `file` is absolute or leaves the shard directory are refused. `stack shard` writes bare names beside `plan.json`.
- `plan` and `run` refuse, with exit 78, when `[modules] init` names an EESSI generation other than the one the target's watch record was built against. The message says to give the new generation its own prefix, to plan `roots.txt` from `eb-stack stack gap`, and to record that prefix with `watch --adopt`.
- `run` leaves out an `installed_modules_dirs` tree that does not exist yet and logs a WARN. On a new site prefix, `stack shard` refused the missing `modules/all` and the run exited 2 before any build.
- `run` removes the job's reservation, `<reservations>/<job>.json`, under the admission lock when an admitted unit's builds end and when the run ends first. Before, the reservation of the last unit stayed, and without `live_jobs_cmd` every later job counted it against its own admission. A record another writer has replaced since, such as a `forecast` or `waiting` one, is kept with a WARN.
- `stack shard` runs inside the module wrapper and sees what the `[modules]` init list sets. Without EESSI-extend's `EASYBUILD_FILTER_DEPS`, the planner put filtered tools such as M4, Bison and zlib into the prelude list.
- `templates/eessi-site.toml` marks only `cpu-a` as the default target.

### Documentation

- A how-to guide builds a list on every target from continuous integration (CI): the GitHub Actions workflow of `examples/ci/`, with why each target gets its own run id, when to raise `max-parallel` and what the `concurrency` group holds, and a caller script for other CI systems that exits as a run does.
- A how-to guide builds some lines of a list on some targets only, with a `[targets.select]` table in each target, and shows what each target plans and runs of the list, `selected-TARGET.list` and the `deselected` lines of the plan and the result.
- A how-to guide installs eb-pipeline from the 0.3.0 release, checked against its checksum, or from source with `cargo install --locked`, and says what the release lacks of the code the documentation describes.
- A how-to guide rebuilds installed software through eb-staged-rebuild: the `[rebuild]` table with a state root per target, the line and module that make a line staged, the run id each staged line passes, and the messages of a refused or pending line and of the site files `check` refuses.
- A how-to guide rotates the log tree, at the end of each run and from cron under the same lock with `flock`, with the messages of a run that leaves the rotation to another and of a rotation that fails.
- A how-to guide says how to let users load what a target installs on top of EESSI: with `EESSI_SITE_SOFTWARE_PREFIX` set to the target's `site_prefix` before EESSI loads, without which the EESSI module takes only its `host_injections` tree, and with a `cpu_target` that is the target their jobs on those nodes load. With eessi-arch-guard as the Slurm `TaskProlog`, that is the target the guard selects, which can differ from the one EESSI selects on the same CPU; a tree built for another target is on no user's `MODULEPATH`. The guide also says that a job keeps the prefix of the shell that submitted it.
- A how-to guide writes a site file for a cluster from the small-cluster template, checks it with `check`, `--lists-dir` and `--probe-tools`, reads it back with `explain` and a dry run, and lists the messages of the common mistakes.
- A weekly and manually started workflow runs `examples/integration-smoke.sh` on a GitHub-hosted runner. The script downloads the published release of each stackhands tool and of eb-stack, mounts the European Environment for Scientific Software Installations (EESSI) with the EESSI GitHub Action, and builds one tiny easyconfig through the tools. The workflow is limited to a weekly schedule and a manual start.
- An explanation page says what the targets of a site file share, what goes wrong when two of them build into one build directory, download directory or state root at once, and when `{target}` in those paths lets them build at the same time.
- An explanation page says why a rotation of the log tree takes eb-pipeline's lock and eb-keep-logs' lock, always in that order, why a run does not wait for another rotate, and what the locks do not cover, with a figure of who takes which lock.
- Code and verbatim text that starts with `*` shows as written: the log rotation page showed `*.log` and `*.txt` as `\*.log` and `\*.txt`. `docs/export.el` writes such text as a plain reStructuredText literal, and `pixi run -e docs docbld` runs `docs/tools/test_export.py`, which exports a page of literals and checks each one.
- Every public item of the `eb_pipeline` library has a doc comment, and `pixi run -e docs docbld` fails on one without. The field of a site file key in `eb_pipeline::schema` carries the text `eb-pipeline schema` prints for that key.
- Explanation pages say why one site file drives the build and the caller stays thin, and how a run goes through its phases and units, what stops it, and how one exit status sums up its lines, with a figure of the phases; the tutorial links them.
- The README and `examples/ci/github-actions.yml` say when `max-parallel` may be raised: when no two targets that build at once share a build directory, a download directory or, with staged rebuilds, a state root. `{target}` in `[filesystems] build_path`, in the first `[filesystems] source_paths` entry and in `[rebuild] state_root` gives each target its own. `eessi-site.toml` has `{target}` in `build_path` and `state_root`, and says when to give each target a first source path of its own.
- The README and install how-to document the five companion binaries, verify them after a source install, and use `--root ~/.local` consistently with the first-build tutorial.
- The README gives the `software_root` and `modules_root` defaults under the target's `site_prefix`, as `explain` resolves them. It said `<install_root>/software` and `<install_root>/modules`.
- The README is short and links the pages of the documentation, which now hold its sections, log rotation included. The tests that read the README read those pages instead, and one fails when the `--help` text on the command reference differs from what `eb-pipeline --help` prints.
- The README lists the other stackhands tools.
- The README says that `run` builds its units one at a time, with no option to build one shard alone, and that a shard list holds input lines only, so a line needs `--robot` to build missing dependencies.
- The README says when `run` writes the result file: never for `--dry-run`, and never when an input error stops it first, which leaves an older file in place.
- The README says why the targets of one site file build one at a time when they share the EasyBuild build path: two targets that build one easyconfig at once remove each other's build directory. Staged lines of targets that share `[rebuild] state_root` wait for each other's lock. `examples/ci/github-actions.yml` sets `max-parallel: 1`.
- The README tutorial builds `pigz-2.8-GCCcore-15.2.0.eb` on GCCcore 15.2.0, the compiler EESSI 2026.06 provides. Its `zlib-1.3.1-GCCcore-14.2.0.eb` needed GCCcore 14.2.0 built from source first.
- The README's new Log rotation section describes the two locks in the log root, eb-pipeline's `.eb-pipeline-rotate.lock` and eb-keep-logs' own `.eb-keep-logs-rotate.lock`, and the order that whatever takes both keeps, eb-pipeline's first. It documents a rotation from cron that keeps that order, `TZ=UTC flock -w 3600 -E 75 <log_root>/.eb-pipeline-rotate.lock eb-keep-logs rotate <log_root> ...`, which waits up to an hour for the runs under the log root to end and exits 75 when the tree stays busy, as `rotate` does when another rotate holds its lock.
- The build job of `examples/ci/github-actions.yml` has a `concurrency` group per target, `eb-pipeline-<target>`, with `cancel-in-progress: false`: while one dispatch builds a target, another dispatch's job for that target waits. The README says that GitHub keeps only one pending job per group, so that a third dispatch replaces the waiting one, why the group keeps two dispatches off one target but not off a build directory, download directory or state root that targets share, and which group to use then.
- The command reference and the page on following runs describe `eb-pipeline metrics`, including the Prometheus gauges and the JSON status.
- The commands on the reference pages are in blocks of their own, without a prompt, so that copying a block gives commands that run.
- The documentation is a site built from Org pages in `docs/orgmode/`, published at <https://stackhands.github.io/eb-pipeline/>, with the API of the `eb_pipeline` library from `cargo doc` beside it at `api/`. `pixi run -e docs docbld` builds it with the tools `pixi.lock` pins, and fails on a Sphinx or rustdoc warning, an internal link or anchor that does not exist, or the name of a real site. The `docs` workflow builds it for every pull request, checks its external links, also weekly, and publishes it from `main`.
- The first-build tutorial checks out `main`, and its executable example rejects a moving commit pin that would become stale after the next merge.
- The first-build tutorial's dry run, run log and result include the checks phase, which a build of the published main prints between build and caches when the site file has no `[checks]` table. The checkout the tutorial names is that main.
- The phase explanation and the plan reference name the checks phase, including a ReFrame command that runs after a staged rebuild.
- The reference has a page for the `eb-pipeline` command, with the usage line, options, variables and exit statuses of every subcommand, and a page for the environment variables it reads, the `EB_PIPELINE__SECTION__KEY` site file values and the variables a run sets for the commands it starts. `docs/examples/cli.sh` prints the help texts the page shows.
- The reference has a page on exit statuses: what leads to each of 0, 1, 2, 75, 78, 130 and 143, the order in which `run` picks one when several apply, and what a run does with each exit status of `eb`, eb-stack, eb-staged-rebuild, eb-keep-logs and the cache command.
- The reference has a page on the log tree: the files a run writes in its log directory, the files of the log root, and the locks a run takes, with the order in which it takes the two rotation locks.
- The reference has a page on the plan and result files, with every key of each object, the statuses of a line and of a phase, and a plan and a result from `docs/examples/files.sh`.
- The reference has a page on the site file templates that `eb-pipeline init` writes, with each template in full and what `check` and `targets` say about it.
- The reference has a page on the site file, with a table per section that gives every key its type, default and meaning, and sections on the placeholders, `{target}` in paths, the `[logs]` rotation and the lines that go through `[rebuild]`. `tests/docs.rs` fails when a table and `eb_pipeline::schema::SECTIONS` differ in a key, its order, its type or its default.
- The staged-rebuild and eb-stack how-tos show the checks phase a site file without `[checks]` leaves out, as the run prints it.
- The tutorial and install how-to tell readers whose default `cargo` is older than 1.90 to run `rustup toolchain install 1.90` and `cargo +1.90 install`. `docs/examples/tutorial-rust-floor.sh` fails until both pages carry those lines.
- The tutorial, "Run your first build list", installs eb-pipeline from source, writes a site file with the `local` scheduler, runs a build list against a stand-in for `eb`, reads the result file and a line's log, and makes a line fail. `docs/examples/tutorial.sh` runs its commands in order and checks what the page says they print, and the `docs` job of the `ci` workflow runs it.
- Two how-to guides. One rebuilds what an EESSI change reaches: the `[watch]` table, the record of each run, a dry run of `watch` before and after EESSI changes, `watch` from cron and what each of its exit statuses asks for, the rebuild, the refusal of a new EESSI version, `watch --adopt`, and a plan against a changed copy of the EESSI module tree through `[watch] modules_dirs`. The other follows runs through their summaries: `status`, the failed lines of a run and their logs with `jq`, and a `[notify]` webhook, tried first against a listener on `127.0.0.1`.
- `docs/examples/tutorial.sh` also runs the `cargo --version` that the tutorial starts with; `RUSTUP_TOOLCHAIN=1.90.0` prints the version the page shows.
- `examples/ci/github-actions.yml` is a GitHub Actions workflow that builds one list on every target, one job per target, from the array `eb-pipeline targets --format json` prints. The README shows the same fan-out as a shell loop, and why each target needs a run id of its own.

### Removals

- The commands of a run get the site file in `EB_PIPELINE_SITE`, as an absolute path, and the site name in the new `EB_PIPELINE_SITE_NAME`; `explain` and the plan show both. `EB_PIPELINE_SITE` held the name, so an `eb-pipeline` that a command started without `--site` tried to read a file of that name and exited 2. A command that reads the site name from `EB_PIPELINE_SITE` must read `EB_PIPELINE_SITE_NAME` instead. In the library, `resolve::explain` takes the site file as its second argument and `Scope` has a `site_file` field.
- `check`, and every command that loads the site file, refuses a `modules.init` entry with white space, since an entry is one module name. The module wrapper split such an entry at the white space, so `init = ["A B"]` loaded `A` and `B`, and a pasted `module load A` line loaded `module`, `load` and `A`. Write `init = ["A", "B"]`.
- `exit::UNSHARDED` and the remap of 3 to 0 at the end of `run`; no path produced it.

### Miscellaneous

- A test covers a record whose probe the job directory wrapper of `filesystems.job_dir` stops. It checks that the record's warning quotes the wrapper's line, and that the run and the record otherwise go as for any record that fails.
- Changes are recorded as news fragments in `changelog.d/`, and towncrier assembles them into `CHANGELOG.md` at release, with zig-common's fragment types. CI drafts the release notes and fails on a fragment whose name has no type.
- Dependabot updates the action pins in `examples/ci/` along with those in `.github/workflows/`.
- `list_word` writes one word of a build list so the list reader returns that word, and leaves a parenthesis or a star bare. `quote` stays the shell quoter: an apostrophe is written `'"'"'`, which a shell and the argument splitter both read back, and a parenthesis is quoted.


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
