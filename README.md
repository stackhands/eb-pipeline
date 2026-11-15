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

Nothing about a site lives in this repository. The examples carry placeholders
only, and a test rejects any example value that looks site-specific.

## Install

Needs Rust 1.90 or newer.

```bash
cargo install --path .
eb-pipeline --version
```

At run time the driver needs `eb` (through the site's modules) and whichever of
the three tools above the site file enables.

## Tutorial: plan and dry-run a list on a small cluster

1. Write the site file. This is `examples/small-cluster.toml`: Slurm, one CPU
   target, no EESSI, no quota, no licences.

   ```bash
   cp examples/small-cluster.toml site.toml
   ```

   The paths in it are placeholders under `/opt` and `/scratch`; edit them for
   your cluster.

2. Check it. `check` exits 0 for a valid file and 2 otherwise.

   ```bash
   eb-pipeline check --site site.toml
   ```

   ```text
   INFO [check] small-lab: site file is valid
   ```

3. Write a build list: one easyconfig per line, EasyBuild options after it.

   ```bash
   printf 'zlib-1.3.1-GCCcore-14.2.0.eb\n' > list.txt
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
| `check --site FILE [--lists-dir DIR] [--probe-tools]` | parse and validate; exit 0 or 2 |
| `schema` | print every key with type, default, required flag and meaning |
| `explain --site FILE [--target T] [--build-type B] [--format toml\|json]` | resolved per-target configuration |
| `plan --site FILE --list LIST --target T --build-type B [--run-id ID] [--log-stamp S] [--shard-plan DIR] [--out FILE.json]` | phases, launch lines, environment, tool calls |
| `run` (arguments of `plan`, plus `--result FILE.json`, `--dry-run`) | execute the plan |

`eb-pipeline schema` is generated from the same declarations the parser uses,
and a test asserts that every field of every section is documented there.

Streams: stdout carries `plan`, `explain`, `schema` and `--dry-run` output only.
Events go to stderr, one per line, as `LEVEL [phase] target: message`. The
result JSON goes to `--result`.

## Phases

`preflight`, `licences`, `shard`, `sources`, `admission`, `build`, `caches`,
`logs`, in that order. An optional section that is absent, or has
`enabled = false`, is skipped and recorded as "not configured" in the plan and
the result.

- `shard`: `eb-stack stack shard`. The exit code `shards.planner_exit_unsharded`
  (default 3) means the planner built the plan unsharded. The list then builds
  as one unit.
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
- `build`: one `eb` call per list line, started through the scheduler launch
  line of the target. A line goes through `eb-staged-rebuild run` only when
  all three hold: `[rebuild] staged = true` (a target can override it with
  `staged`), the line carries `--rebuild`, `-f` or `--force` as a whole option
  (`--force-download` and `--rebuild-from-scratch` do not count), and the module
  is installed, meaning `<modules_root>/all/<Name>/<version>.lua` or a file of
  that name without `.lua` exists. Otherwise it is a normal `eb` call.
  `software_root` and `modules_root` are per-target keys, defaulting to
  `<install_root>/software` and `<install_root>/modules`; they go to
  `eb-staged-rebuild --software` and `--modules` and to the installed check. A failed line triggers
  `eb-keep-logs failed` when `[logs].keep_failed` is set.
- `caches`: runs `[caches].command` once for the selected target, after a clean
  build, with `EB_PIPELINE_CACHE_*` in the environment.
- `logs`: `eb-keep-logs rotate` on `filesystems.log_root`.

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

The option is read as `--name=a,b` or `--name a,b`. A line is planned for the
target when the option is absent and `when_absent = "all"`, or when its value
list contains `value`. `plan` and `run` both apply it, to the whole list or to
each shard unit; a unit left without lines is dropped. With a selector, `run`
writes the kept lines, in list order and with `strip` applied, to
`<log_dir>/selected-<target>.list` and passes that file to `stack shard
--build-list`, so deselected lines do not weigh in shard balancing. `plan` shows
the same path in the shard command, under `selected_list`, and records the kept
count under `selected`. A shard plan given to `plan --shard-plan` must come from
that file. `plan` lists the dropped lines under `deselected`, and the result file
does the same, apart from `lines` and `counts`, so a deselected line never reads
as a failure. Without `[targets.select]` every line is planned.

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
before the command starts), `{log_dir}`, `{work_dir}` (`filesystems.build_path`),
`{target}`, `{phase}`, `{runtime_root}`, `{shares_ro}`, `{shares_rw}`. Share
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

### Install paths, cache module, installed module dirs

- `filesystems.export_install_vars = "split"` exports
  `EASYBUILD_INSTALLPATH_SOFTWARE` and `EASYBUILD_INSTALLPATH_MODULES` from the
  target's `software_root` and `modules_root` instead of `EASYBUILD_INSTALLPATH`.
  `read_only_installdir_value` sets the value of `EASYBUILD_READ_ONLY_INSTALLDIR`
  (default `"1"`).
- `[caches] init_module` is the default `EB_PIPELINE_CACHE_INIT_MODULE`; an entry
  in `[[caches.targets]]` overrides it.
- `shards.installed_modules_dirs_from_env` names variables whose colon-separated
  values are appended to `installed_modules_dirs` when `run` starts. An unset
  variable refuses with 78, an empty one adds nothing, and `plan` shows
  `${NAME}`.

Every child writes to `<log_root>/<YYYYMMDD_HHMMSS>/<target>/`. The timestamp
is taken once per run and `--log-stamp` overrides it. `--run-id` is a separate
label recorded in the result. The result JSON records the log path of each line.

`plan --shard-plan DIR` reads the `plan.json` that `stack shard` wrote in `DIR`
and expands the per-shard admission and build commands.

Limitation: the licence gate checks configuration only. It confirms that every
required package has a group, a threshold or an exemption; it does not verify
file permissions or group membership on the host.

## Exit codes

| code | meaning |
|---|---|
| 0 | all requested work completed |
| 1 | work attempted and failed (a build line or a phase); see the result file |
| 2 | usage error, invalid site file, or a tool below its `min_version` |
| 3 | the planner declined to shard; the plan was built unsharded (informational; `run` itself finishes with 0 or 1) |
| 75 | a resource is temporarily unavailable or a step is pending; rerun |
| 78 | refused by a precondition (admission, configuration); no build step started |
| 130 | interrupted by SIGINT, after the running child ended |
| 143 | interrupted by SIGTERM, after the running child ended |

The process exit code is never a sum of failures; the result file holds the
per-line counts. When several conditions apply the order is 1, 78, 75, 0.

Sibling tool codes are mapped explicitly:

| tool | code | driver reading |
|---|---|---|
| eb-stack `stack admit` | 0 / 78 / 1 / 2 | admitted / refused (retry) / failed / usage, exit 2 |
| eb-stack `stack shard` | `shards.planner_exit_unsharded` | unsharded, continue |
| eb-staged-rebuild `run` | 2 | refusal there; the line is recorded as `refused` with `translated_from = 2` and the run ends with 78 |
| eb-staged-rebuild `run` | 75 | pending; the line is recorded as `pending`, exit 75 |
| eb-staged-rebuild `run` | other non-zero | failed |

SIGTERM and SIGINT are forwarded to the running child. The driver starts no
further child, writes the result file, and exits 143 or 130.

## Result file

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

Line statuses: `success`, `failed`, `refused`, `pending`, `skipped`. Lines that
a target's `[targets.select]` left out appear under `deselected` (`easyconfig`
and `unit`) and in no status count.

## A thin CI caller

```bash
eb-pipeline run --site site.toml --list "$LIST" --target "$TARGET" \
    --build-type "$BUILD_TYPE" --run-id "$CI_RUN_ID" --result "result-$TARGET.json"
rc=$?
```

The caller names no partition, path, licence group or quota command. It maps
`rc` to its own status and archives the result file and the log directory.

## Development

```bash
cargo fmt --all --check
cargo clippy --locked --all-targets -- -D warnings
cargo test --locked
```

`run` is tested against stub tool binaries written into a temporary directory.
The scenarios: success, a failed line, admission refused after N attempts,
admission admitted on the second attempt, staged-rebuild refusal translation,
and SIGTERM forwarding.

## Licence

MIT. See [LICENSE](LICENSE).
