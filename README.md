# eb-pipeline

A site-neutral EasyBuild pipeline driver. One per-site TOML file describes
targets, scheduler allocations, paths, admission, modules, caches, log handling
and licences. `eb-pipeline` turns that file and a build list into an ordered set
of subprocesses, so a CI system (Jenkins, GitLab CI, a cron job) becomes a thin
caller that passes a list and a target and reads an exit code and a result file.

The driver composes existing stackhands tools and never links them in:

| tool | called for |
|---|---|
| [eb-stack](https://github.com/stackhands/eb-stack) | `stack shard` (planning), `stack admit` (inode admission) |
| [eb-keep-logs](https://github.com/stackhands/eb-keep-logs) | `failed` (keep a failed build's logs), `rotate` |
| [eb-staged-rebuild](https://github.com/stackhands/eb-staged-rebuild) | `run`, for lines that rebuild a live install |

Nothing about a site lives in this repository. The templates carry placeholders
only, and a test rejects any template value that looks site-specific.

## Install

Needs Rust 1.90 or newer.

```bash
cargo install --path .
eb-pipeline --version
```

At run time the driver needs `eb` (through the site's modules) and whichever of
the three tools above the site file enables.

## Tutorial: plan and dry-run a list on a small cluster

1. Write the site file from a template. `small-cluster` is Slurm, one CPU
   target, no EESSI, no quota, no licences; `eb-pipeline init --list` names the
   others.

   ```bash
   eb-pipeline init --template small-cluster --out site.toml
   ```

   The paths in it are placeholders under `/opt` and `/scratch`; edit them for
   your cluster, or set them from the environment (see
   [Configuration](#configuration)).

2. Check it. `check` exits 0 for a valid file and 2 otherwise.

   ```bash
   eb-pipeline check --site site.toml
   ```

   ```text
   INFO [check] small-lab: site file is valid
   ```

3. Write a build list: one easyconfig per line, EasyBuild options after it.
   This one builds pigz on GCCcore 15.2.0, the compiler EESSI 2026.06
   provides.

   ```bash
   printf 'pigz-2.8-GCCcore-15.2.0.eb\n' > list.txt
   ```

4. Plan it. `plan` computes everything and executes nothing; the JSON is the
   same on every invocation, so it can be diffed.

   ```bash
   eb-pipeline plan --site site.toml --list list.txt --target cpu \
       --build-type production --run-id demo --out plan.json
   ```

5. Run with `--dry-run` to see the commands that `run` would start:

   ```bash
   eb-pipeline run --site site.toml --list list.txt --target cpu \
       --build-type production --run-id demo --dry-run
   ```

   Drop `--dry-run` and add `--result result.json` to execute.

## Subcommands

| command | purpose |
|---|---|
| `init --template NAME [--out FILE]`, `init --list` | write a starter site file from a built-in template; an existing file is refused |
| `check --site FILE [--lists-dir DIR] [--probe-tools]` | parse and validate; exit 0 or 2 |
| `schema` | print every key with type, default, required flag and meaning |
| `explain --site FILE [--target T] [--build-type B] [--format toml\|json]` | resolved per-target configuration |
| `targets --site FILE [--format text\|json]` | target names in file order, one per line or as a JSON array on one line |
| `plan --site FILE --list LIST --target T --build-type B [--run-id ID] [--log-stamp S] [--shard-plan DIR] [--out FILE.json]` | phases, launch lines, environment, tool calls |
| `run` (arguments of `plan`, plus `--result FILE.json`, `--dry-run`) | execute the plan |

`eb-pipeline schema` is generated from the same declarations the parser uses,
and a test asserts that every field of every section is documented there.

Streams: stdout carries `plan`, `explain`, `schema`, `init`, `targets` and `--dry-run` output only.
Events go to stderr, one per line, as `LEVEL [phase] target: message`. The
result JSON goes to `--result`.

## Configuration

Configuration lives in two places: the site file, which describes the site,
and the environment, which describes one deployment of it.

Every command line flag falls back to a variable; a flag given on the command
line wins.

| flag | variable |
|---|---|
| `--site` | `EB_PIPELINE_SITE` |
| `--list` | `EB_PIPELINE_LIST` |
| `--target` | `EB_PIPELINE_TARGET` |
| `--build-type` | `EB_PIPELINE_BUILD_TYPE` |
| `--run-id` | `EB_PIPELINE_RUN_ID` |
| `--log-stamp` | `EB_PIPELINE_LOG_STAMP` |
| `--lists-dir` | `EB_PIPELINE_LISTS_DIR` |

`EB_PIPELINE__SECTION__KEY=value` sets one site file value. The name after the
prefix is split on `__` and lowercased. `EB_PIPELINE__FILESYSTEMS__SCRATCH`
sets `filesystems.scratch`; `EB_PIPELINE__SITE__TOOLS__EB_STACK__BIN` sets
`site.tools.eb_stack.bin`.

The value is read as a TOML value: `true`, `16`, `["/opt/a", "/opt/b"]`. Text
that does not parse as one is taken as a string. Quote a string that looks like
a number: `'"1.0"'`.

The file plus the overrides go through the same schema as the file alone. An
unknown key or a wrong type is a usage error (exit 2). Overrides apply in name
order, and each one is logged to stderr by key, never by value. Values inside
`[[targets]]` cannot be overridden; they stay in the file.

```bash
export EB_PIPELINE_SITE=/opt/eb/site.toml
export EB_PIPELINE__FILESYSTEMS__SCRATCH=/scratch/$USER/eb
eb-pipeline explain --target cpu
```

## Phases

`preflight`, `licences`, `shard`, `sources`, `admission`, `build`, `caches`,
`logs`, in that order. An optional section that is absent, or has
`enabled = false`, is skipped and recorded as "not configured" in the plan and
the result.

With `scheduler.kind = "slurm"`, every command starts as `launch`,
`common_args`, `--job-name=<job_name_template>`, then the launch arguments of
its role from `[targets.phases]` or `[phase_defaults]`: `plan` for `stack
shard`, `short` for admission, prefetch and log handling, `build`, `cpu` for
CPU-role units, and `cache`. A role with no arguments launches a bare
`srun --job-name=…`. Inside an allocation srun then starts one task per
allocated task, so give each role a task count; the templates use
`-N 1 --ntasks-per-node=1` for the roles they run.

- `shard`: `eb-stack stack shard`. The exit code `shards.planner_exit_unsharded`
  (default 3) means the planner built the plan unsharded. The list then builds
  as one unit. The planner runs inside the module wrapper, so what the
  `[modules]` init list sets, EESSI-extend's `EASYBUILD_FILTER_DEPS` for
  example, reaches it. `run` leaves out an `installed_modules_dirs` tree that
  does not exist yet, such as the site tree before its first install, and logs
  a WARN.
- `sources`: `eb <easyconfig> --fetch --robot` for every list line, with the
  line's options other than `--rebuild*`, then `[sources].extra_args`. A line
  whose fetch fails is recorded as failed and is not built. `--fetch` ignores
  installed modules, so `--robot` fetches the sources of every dependency,
  those EESSI provides included. `--disable-robot` in `extra_args` fetches each
  line's own sources only: the driver leaves out its `--robot`, drops a line's
  `--robot`, and keeps the paths of a line's `--robot=PATH` at the front of
  `--robot-paths` so the easyconfig is still found.
- `admission`: `eb-stack stack admit` before each build unit. Exit 78 retries
  with the backoff in `[admission]` (`attempts`, `wait_seconds`, doubling up to
  `max_wait_seconds`). The job id comes from the variable named by
  `job_id_env` (default `SLURM_JOB_ID`); `run` refuses with 78 before any build
  when it is unset or not a positive integer without a leading zero, and `plan`
  shows `${SLURM_JOB_ID}`. A unit's footprint is the shard plan's
  `inode_footprint`, else `default_footprint`, which `check` requires when
  admission is enabled. `unknown_reserve` stays the reserve for live jobs that
  have no record. A refusal after the last attempt ends the run with exit
  78 and a message that names the attempt count and the tool's last JSON line.
  With `required = true`, a missing `quota_cmd` or `reservations` refuses with 78.
  When an admitted unit's builds end, or the run ends first, `run` removes the
  job's reservation `<reservations>/<job>.json` under `lock`, taken as
  `lock_kind`. A record another writer has replaced since, such as a
  `forecast` or `waiting` one, is kept with a WARN. A job the scheduler kills
  leaves its reservation behind, so set `live_jobs_cmd` to print the running
  jobs of the build account, for example `squeue -h -t R -u <build-user> -o %A`.
  Without it `stack admit` counts every other job's reservation, and `check`
  warns.
- `build`: one `eb` call per list line, started through the scheduler launch
  line of the target. A line goes through `eb-staged-rebuild run` only when
  all three hold: `[rebuild] staged = true` (a target can override it with
  `staged`), the line carries `--rebuild`, `-f` or `--force` as a whole option
  (`--force-download` and `--rebuild-from-scratch` do not count), and the module
  is installed, meaning `<modules_root>/all/<Name>/<version>.lua` or a file of
  that name without `.lua` exists. Otherwise it is a normal `eb` call.
  `software_root` and `modules_root` are per-target keys, defaulting to
  `software` and `modules` under the target's `site_prefix`, or under
  `filesystems.install_root` when the target sets none; they go to
  `eb-staged-rebuild --software` and `--modules` and to the installed check.
  Each staged line passes its own `--run-id`: `<run-id>-` followed by the
  easyconfig file name without `.eb`, with `.2`, `.3` appended when a list
  repeats a line. eb-staged-rebuild refuses a run id it already holds, so a
  staged line runs again under a new `--run-id`. eb-staged-rebuild holds
  `<state root>/.lock` for the whole of a staged build and publish, and each
  staged line passes `--lock-timeout` from `[rebuild] lock_timeout` (default
  600 seconds; 0 refuses at once). A line whose wait runs out is `refused`; its
  note and the ERROR line name the lock that another eb-staged-rebuild holds
  and, when that is the state root's, say that `lock_timeout` ran out. Without
  `[rebuild] state_root`, a module that the line loads can set the state root
  in `EB_STAGED_REBUILD_STATE_ROOT`, where eb-pipeline cannot see it, so for a
  lock away from the state root eb-pipeline expects, the note says that
  `lock_timeout` ran out if that is the state root's lock, and why eb-pipeline
  cannot tell. A failed line triggers
  `eb-keep-logs failed` when `[logs].keep_failed` is set.
  That step runs with `EASYBUILD_OUTPUT_STYLE=basic`, whatever `[scheduler.env]`
  sets, so that `eb --last-log` prints the log path on one line.
- `caches`: runs `[caches].command` once for the selected target, after a clean
  build, with `EB_PIPELINE_CACHE_*` in the environment.
- `logs`: `eb-keep-logs rotate` on `filesystems.log_root`. `[logs.rotate]` needs
  `compress_after_days`, passed as `--compress-after`; `delete_after_days`,
  passed as `--delete-after`, must be greater. eb-keep-logs 0.2.1 takes no
  lock and passes over at most the run that it writes into itself: two
  rotations at once lose each other's updates to a run's `.licensed-logs`,
  and a rotation compresses the logs of a run that is still writing them once
  that run's log stamp is a day old in UTC, as it is for a target that
  started before midnight. So a run holds `<log_root>/.eb-pipeline-rotate.lock`
  shared from its start and rotates only when it can take that lock alone.
  While another run under the log root is live or rotating, `logs` is
  `skipped` and the rotation is left to a run that ends alone; a run that
  starts while another rotates waits for it. The lock is a `flock`, so it
  holds between nodes only where the log root's filesystem carries `flock`
  between them. An eb-keep-logs that takes a lock of its own holds
  `<log_root>/.eb-keep-logs-rotate.lock` for the whole rotation, and exits 75
  when another rotate holds it past `--lock-timeout`, having changed nothing.
  `logs` is then `skipped`, with a note that names that lock, and the
  rotation is left to the rotate that holds it: that rotate lists the log
  root after taking the lock, so it does this rotation's work, and a later
  run that ends alone does what is left. Any other non-zero exit of `rotate`
  is `failed`. `run` passes `--lock-timeout 0`, so that the rotation does not
  wait: the run holds `.eb-pipeline-rotate.lock` alone while it rotates, and
  every run under the log root that starts meanwhile waits for it.
  eb-keep-logs 0.2.1 refuses `--lock-timeout` with exit 2, and the builds
  that take it print the same version, so `run` passes the option only when
  `eb-keep-logs --help`, run where eb-pipeline runs as the `min_version`
  checks are, lists it. Otherwise the rotation runs without it, and the
  `logs` note and an INFO line say why.

### Per-target line selection

A build list can mark lines for a subset of targets with a per-line option.
`[targets.select]` names the option and this target's token:

```toml
[targets.select]
option = "--only-targets"   # long option that carries the selection
value = "target-a"          # this target's token
separator = ","             # between tokens in the option's value
when_absent = "all"         # "all" plans lines without the option, "none" skips them
strip = false               # true removes the option before eb is called
```

`option` includes the leading dashes, and a line carries it as `--name=a,b` or
`--name a,b`. A line is planned for the target when the option is absent and
`when_absent = "all"`, or when its value list contains `value`. `plan` and `run`
both apply it, to the whole list or to each shard unit; a unit left without
lines is dropped. With a selector, `run` writes the kept lines, in list order
and with `strip` applied, to `<log_dir>/selected-<target>.list` and passes that
file to `stack shard --build-list`, so deselected lines do not weigh in shard
balancing. `plan` shows the same path in the shard command, under
`selected_list`, and records the kept count under `selected`. A shard plan given
to `plan --shard-plan` must come from that file. `plan` lists the dropped lines
under `deselected`, and the result file does the same, apart from `lines` and
`counts`, so a deselected line never reads as a failure. Without
`[targets.select]` every line is planned.

### Route contract

`[targets.route]` wraps the commands of CPU-role units (the CPU prerequisites
that `stack shard` lifts out) in a command. Beyond `kind` and `command`:

| key | meaning |
|---|---|
| `args` | argument templates after `command`; a `--` follows them and the phase command comes after it |
| `env` | environment of the routed command |
| `runtime_root` | exported as `EB_PIPELINE_ROUTE_RUNTIME_ROOT` |
| `shares_ro`, `shares_rw` | shared paths, exported joined with `:` as `EB_PIPELINE_ROUTE_SHARES_RO` and `_RW` |
| `receipt_digest_env` | name of a variable that holds a digest; `run` refuses with 78 when it is unset or empty and exports the value as `EB_PIPELINE_ROUTE_RECEIPT_DIGEST`; `plan` shows `${NAME}` |
| `phases` | `build` (default) and `sources`: the phases whose CPU-role commands are routed |

Placeholders in `args` and `env`: `{phase_dir}` (`<log_dir>/<phase>`, created
before the command starts), `{log_dir}`, `{work_dir}` (the target's
`filesystems.build_path`), `{target}`, `{phase}`, `{runtime_root}`,
`{shares_ro}`, `{shares_rw}`. Share
entries accept the first three. A route with only `kind` and `command` plans
exactly the 0.1.0 argv.

### Hardware classes, fetch profile, package environment

- `[targets.cpu_hardware]` has the shape of `[targets.hardware]` and names the
  hardware class of CPU-role units. When it is set, every command exports its
  class as `EB_PIPELINE_HW_VENDOR_ID`, `EB_PIPELINE_HW_CPU_FAMILY` and
  `EB_PIPELINE_HW_MODEL`: `cpu_hardware` for CPU-role commands, `hardware` for
  the rest. Without it nothing is exported.
- `[targets.fetch_profile]` is a free-form table passed to source prefetch
  commands as `EB_PIPELINE_FETCH_PROFILE`, serialised as JSON with sorted keys.
- `[targets.package_env.<Name>]` adds to `[modules.package_env.<Name>]` for that
  target; per key the target value wins.

### Per-target paths

`{target}` in `[filesystems] build_path`, in the entries of
`[filesystems] source_paths` and in `[rebuild] state_root` expands to the
target's name. `build_path = "/scratch/<build-user>/build/{target}"` gives each
target an `EASYBUILD_BUILDPATH` of its own, which `{work_dir}` follows.
EasyBuild looks for a source in every `source_paths` entry and downloads a
missing one into the first, writing the file in place, so
`source_paths = ["/scratch/<build-user>/sources/{target}", "/opt/eb/sources"]`
gives each target a download directory of its own and still finds the sources
under `/opt/eb/sources`. `state_root = "/opt/eb/{target}/.eb-staged-rebuild"`
gives each target an eb-staged-rebuild state root, and so a lock, of its own.
`plan` and `explain` show the expanded paths. `check` refuses any other
placeholder in those keys, `{target}` in the other `[filesystems]` paths, the
target names `.` and `..`, and a `{target}` state root when two staged targets
share a software root: eb-staged-rebuild locks a live tree through its state
root, so one tree takes one state root.

### Install paths, cache module, installed module dirs

- `filesystems.export_install_vars = "split"` exports
  `EASYBUILD_INSTALLPATH_SOFTWARE` and `EASYBUILD_INSTALLPATH_MODULES` from the
  target's `software_root` and `modules_root` instead of `EASYBUILD_INSTALLPATH`.
  `read_only_installdir_value` sets the value of `EASYBUILD_READ_ONLY_INSTALLDIR`
  (default `"1"`).
- With `[modules.eessi] site_install = true` and a single install path,
  EESSI-extend replaces `EASYBUILD_INSTALLPATH` when it loads:
  `$EESSI_SITE_SOFTWARE_PREFIX/versions/<release>/software/linux/<cpu_target>`,
  plus `/<accel_target>` for an NVIDIA target when `EESSI_ACCELERATOR_INSTALL`
  is set. `explain` names that path in each target's `install_note`. A target
  that takes staged rebuilds must then set `software_root` and `modules_root`
  to the `software` and `modules` directories under it; `check` refuses the
  site file otherwise.
- A `--rebuild`, `-f` or `--force` line on a staged target whose module file
  is missing under `modules_root` builds with plain `eb`. `plan` notes it on
  the build command and `run` logs a WARN.
- `[caches] init_module` is the default `EB_PIPELINE_CACHE_INIT_MODULE`; an entry
  in `[[caches.targets]]` overrides it.
- `shards.installed_modules_dirs_from_env` names variables whose colon-separated
  values are appended to `installed_modules_dirs` when `run` starts. An unset
  variable refuses with 78, an empty one adds nothing, and `plan` shows
  `${NAME}`.

Every child writes to `<log_root>/<YYYYMMDD_HHMMSS>/<target>/`. The timestamp
is the UTC time, taken once per run. `--log-stamp` overrides it with a stamp of
the same form, a real UTC date and time; anything else is refused with exit 2.
The `logs` phase runs `eb-keep-logs rotate` with `TZ=UTC`, so a run's age
counts from the UTC midnight of its stamp. `--run-id` is a separate label
recorded in the result. The result JSON records the log path of each line.

`plan --shard-plan DIR` reads the `plan.json` that `stack shard` wrote in `DIR`
and expands the per-shard admission and build commands.

`run` builds the units one at a time, each after its own admission: the CPU
prerequisites, then the prelude, then each shard in turn. It has no option to
build one shard alone. The prelude holds the modules that two or more shards
need. A shard list holds lines of the input list only, so a line whose other
dependencies are not installed needs `--robot` to build them.

Limitation: the licence gate checks configuration only. It confirms that every
required package has a group, a threshold or an exemption; it does not verify
file permissions or group membership on the host.

### Staged rebuild ledger

`[rebuild] ledger_config` is the file passed to `eb-staged-rebuild run
--ledger-config`. It names one `target`, which eb-staged-rebuild copies into
every ledger record, so each target that takes staged rebuilds needs a file of
its own: a target's `ledger_config` overrides the one in `[rebuild]`, and
`check` refuses two such targets that resolve to the same file. `explain` shows
the file of each target.

## Exit codes

| code | meaning |
|---|---|
| 0 | all requested work completed |
| 1 | work attempted and failed (a build line or a phase); see the result file |
| 2 | usage error, invalid site file, or a tool below its `min_version` |
| 75 | a resource is temporarily unavailable or a step is pending; rerun |
| 78 | refused by a precondition, by admission or by eb-staged-rebuild; lines built before the refusal stay built, see the result file |
| 130 | interrupted by SIGINT, after the running child ended |
| 143 | interrupted by SIGTERM, after the running child ended |

The process exit code is never a sum of failures; the result file holds the
per-line counts. A signal (130, 143) or a usage error (2) ends the run with
that code. Otherwise, when several conditions apply, the order is 1, 78, 75, 0.

Sibling tool codes are mapped explicitly:

| tool | code | driver reading |
|---|---|---|
| eb-stack `stack admit` | 0 / 78 / 1 / 2 | admitted / refused (retry) / failed / usage, exit 2 |
| eb-stack `stack shard` | `shards.planner_exit_unsharded` (default 3) | unsharded, continue |
| eb-staged-rebuild `run` | 2 | refusal there, such as a lock still held when `--lock-timeout` runs out; the line is recorded as `refused` with `translated_from = 2` and the run ends with 78 |
| eb-staged-rebuild `run` | 75 | pending; the line is recorded as `pending`, exit 75 |
| eb-staged-rebuild `run` | other non-zero | failed |
| eb-keep-logs `rotate` | 75 | another rotate holds `<log_root>/.eb-keep-logs-rotate.lock`, and this one changed nothing; `logs` is recorded as `skipped` and the run's code is unchanged |
| eb-keep-logs `rotate` | other non-zero | `logs` failed, exit 1 |

SIGTERM and SIGINT are forwarded to the running child. The driver starts no
further child, writes the result file, and exits 143 or 130.

## Result file

`phases` holds the phases the run recorded, in pipeline order from `preflight`
to `logs`. A line that ran through eb-staged-rebuild carries a `note` naming
it, whatever its status.

```json
{
  "kind": "result",
  "exit_code": 1,
  "sharded": false,
  "counts": {"failed": 1, "success": 2},
  "not_configured": ["admission", "shard"],
  "admission": [],
  "lines": [{"easyconfig": "Foo-1.0-foss-2025a.eb", "status": "failed", "exit_code": 1, "log": "..."}],
  "phases": []
}
```

`run` writes the result file unless `--dry-run` is set or an input error stops
it first, such as an invalid site file, an unknown target, an unreadable, empty
or malformed list, or a tool below its `min_version`. Input errors exit 2, or 78
when the licence gate alone fails, and leave an existing file at the `--result`
path as it was, so read the exit code before the file.

Line statuses: `success`, `failed`, `refused`, `pending`, `skipped`. Lines that
a target's `[targets.select]` left out appear under `deselected` (`easyconfig`
and `unit`) and in no status count.

Phase statuses: `ok`, `failed`, `refused`, `skipped`, `not configured`,
`interrupted`, and `unsharded` for `shard`. The `build` phase is `skipped`,
with a note, when admission refused before any line started, and `refused`
when every line that ran was refused.

## A thin CI caller

```bash
export EB_PIPELINE_SITE=site.toml EB_PIPELINE_LIST="$LIST" EB_PIPELINE_TARGET="$TARGET"
export EB_PIPELINE_BUILD_TYPE="$BUILD_TYPE" EB_PIPELINE_RUN_ID="$CI_RUN_ID"
eb-pipeline run --result "result-$TARGET.json"
rc=$?
```

The caller names no partition, path, licence group or quota command. It maps
`rc` to its own status and archives the result file and the log directory.

## One job per target

A run builds one target. To build a list on every target, the caller starts
one `run` per name that `targets` prints, in file order, each after the
previous one has finished:

```bash
export EB_PIPELINE_SITE=site.toml EB_PIPELINE_LIST="$LIST" EB_PIPELINE_BUILD_TYPE="$BUILD_TYPE"
names=$(eb-pipeline targets) || exit
for t in $names; do
  EB_PIPELINE_TARGET=$t EB_PIPELINE_RUN_ID="$CI_RUN_ID-$t" \
    eb-pipeline run --result "result-$t.json" || echo "$t: exit $?" >&2
done
```

An invalid site file exits 2 with nothing on stdout, so no run starts. Each
target needs a run id of its own. `stack shard` writes its plan into
`<shard_dir>/<run-id>`, which has no target in it, and eb-staged-rebuild
refuses a run id it already holds, so targets that share
`[rebuild] state_root` would pass it the same `<run-id>-<easyconfig stem>`.
The default run id, the log stamp, is the same for two runs that start in the
same second. Logs go to `<log_root>/<log-stamp>/<target>/`, so targets never
share a log directory.

Without `{target}` in `[filesystems] build_path`, the targets of a site file
share their EasyBuild build path, so run them one at a time. Every target then
gets one `EASYBUILD_BUILDPATH`, and EasyBuild builds an easyconfig in
`<build_path>/<name>/<version>/<toolchain>-<toolchain version><versionsuffix>`,
with no target in the path. A build removes that directory when it starts and
again after it installs, so two targets that build one easyconfig at once
remove each other's build directory, and the build that loses it fails.
EasyBuild downloads a missing source into the first `source_paths` entry and
writes the file in place, so a target that looks for a source that another
target is still downloading there finds the partial file and fails its
checksum. Staged lines of targets that share `[rebuild] state_root` take one
lock, and eb-staged-rebuild holds it for the whole of a staged build. A second
staged build waits up to `[rebuild] lock_timeout` for it, 600 seconds unless
set, and is `refused` when the first still holds it then, so the run ends with
78 instead of waiting with no deadline. eb-staged-rebuild has not measured that
lock between two build nodes. `EASYBUILD_TMPDIR` is safe to share: each `eb`
makes its own `eb-*` directory under it. So is `log_root`: each run writes
under its own `<log-stamp>/<target>`, and with `[logs.rotate]` a run rotates
only when no other run under the log root is live (`logs` under
[Phases](#phases)).

`targets --format json` prints the names as a JSON array on one line, the form
a CI matrix reads. [examples/ci/github-actions.yml](examples/ci/github-actions.yml)
is a GitHub Actions workflow with one job that prints the array and one build
job per target. It sets `max-parallel: 1`, the safe default, and a target that
fails leaves the others running. Raise `max-parallel` only when no two targets
that build at once share a build directory, a download directory or, with
staged rebuilds, a state root:

- `{target}` in `build_path` gives each target a build path of its own. A
  `build_path` without it is one path for every target, so it needs storage
  that no two targets' build nodes share.
- `{target}` in the first `source_paths` entry gives each target a download
  directory of its own.
- `{target}` in `state_root` gives each target a state root, and so a lock, of
  its own, and so does leaving `state_root` unset: eb-staged-rebuild then keeps
  its state in `<parent of software>/.eb-staged-rebuild`, next to each target's
  software tree. The staged lines of targets that share a state root take
  turns, and one that waits longer than `lock_timeout` is refused.

[Per-target paths](#per-target-paths) has examples of all three.
`eessi-site.toml` has `{target}` in `build_path` and `state_root`; give its
targets a first source path each before raising `max-parallel`.

`max-parallel` holds within one run of the workflow, and each dispatch is a
run of its own. The build job's `concurrency` group, `eb-pipeline-<target>`,
lets one dispatch at a time build a target: another dispatch's job for that
target waits until the running one has finished, and
`cancel-in-progress: false` leaves the running build alone. GitHub keeps only
one pending job per group, so a third dispatch replaces the waiting one, which
is cancelled. The group belongs to the repository and has no workflow name in
it, so another workflow in the repository that uses the same group waits as
well. It keeps two dispatches off one target, not off one path: one dispatch
can build a target while another builds a different one. Targets that share a
build directory, a download directory or a state root therefore need one group
for the whole build job, `eb-pipeline` with no target in it, so that one build
job in the repository runs at a time.

Dependabot updates the action pins in `examples/ci/` along with those in
`.github/workflows/`.

## Development

```bash
cargo fmt --all --check
cargo clippy --locked --all-targets -- -D warnings
cargo test --locked
```

Set `EB_PIPELINE_LINT_DENY` to a comma-separated list of your own site's names
or paths before `cargo test --test examples` to refuse them in the templates as
well; the repository names none.

`run` is tested against stub tool binaries written into a temporary directory.
The scenarios: success, a failed line, admission refused after N attempts,
admission admitted on the second attempt, staged-rebuild refusal translation,
and SIGTERM forwarding.

A change adds a news fragment, `changelog.d/+<name>.<type>.md`, with type
`feature`, `bugfix`, `doc`, `removal` or `misc`.
`towncrier build --draft --version X.Y.Z` prints the release notes the
fragments make. A release writes them into `CHANGELOG.md`, as
[RELEASING.md](RELEASING.md) describes.

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
