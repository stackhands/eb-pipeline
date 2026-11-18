#!/usr/bin/env bash
# A site administrator's path through the published stackhands tools.
#
# The script downloads the latest release of each tool and of eb-stack,
# then, on a machine where EESSI is already mounted:
#   eessi-arch-guard chooses the CPU target,
#   eb-local builds one tiny easyconfig through EESSI-extend,
#   eb-keep-logs keeps a failed build's log and prunes an old run,
#   eb-staged-rebuild builds that install again in a staged tree and swaps it in,
#   eb-pipeline check validates a site file and the plan's eb-stack shard step runs,
#   eb-cvmfs publishes a stub synchronization.
#
# GitHub Actions mounts EESSI before this script, with
# eessi/github-action-eessi. On a host with no mount, and outside GitHub
# Actions, the script runs itself inside the EESSI client image with
# apptainer when that program exists. Set SMOKE_EESSI_VIA=host to refuse
# that, or SMOKE_EESSI_VIA=apptainer to require it.
#
# SMOKE_WORK is the directory for downloads, the prefix and the logs.
# SMOKE_EESSI_VERSION defaults to 2026.06, the stack eb-local 3.x extends.

set -euo pipefail

STEP=start
on_err() {
    local status=$1
    printf 'integration smoke failed during: %s (exit %s)\n' "$STEP" "$status" >&2
    exit "$status"
}
trap 'on_err $?' ERR

step() {
    STEP=$1
    printf '\n== %s ==\n' "$1"
}

say() {
    printf '%s\n' "$*"
}

# EESSI's init script reads variables it does not set. nounset stays off
# while it, and the arch guard's shell text, run.
source_eessi() {
    local init=$1
    set +u
    # shellcheck disable=SC1090
    source "$init"
    set -u
}

apply_guard() {
    local line
    while IFS= read -r line; do
        case $line in
            export\ * | unset\ *)
                set +u
                # The guard's contract is eval of these lines.
                # shellcheck disable=SC2086
                eval "$line"
                set -u
                ;;
            '') ;;
            *) printf 'arch-guard: %s\n' "$line" ;;
        esac
    done
}

machine() {
    local m
    m=$(uname -m)
    case $m in
        x86_64 | aarch64) printf '%s\n' "$m" ;;
        arm64) printf '%s\n' aarch64 ;;
        *)
            printf 'unsupported machine %s\n' "$m" >&2
            exit 2
            ;;
    esac
}

# Print tag, asset name, asset URL, checksum name, checksum URL.
release_asset() {
    local repo=$1 arch=$2
    python3 - "$repo" "$arch" <<'PY'
import json, sys, urllib.request

repo, arch = sys.argv[1], sys.argv[2]
musl = f"{arch}-unknown-linux-musl"
gnu = f"{arch}-unknown-linux-gnu"
zig = f"{arch}-linux-musl"
url = f"https://api.github.com/repos/stackhands/{repo}/releases/latest"
with urllib.request.urlopen(url) as response:
    release = json.load(response)
assets = {item["name"]: item["browser_download_url"] for item in release["assets"]}
names = set(assets)

def pick_stack():
    for name in (f"eb_stack-{musl}.tar.xz", f"eb_stack-{gnu}.tar.xz"):
        if name in names:
            return name
    raise SystemExit(f"no eb-stack archive for {arch} in {sorted(names)}")

def pick_pipeline():
    name = f"eb-pipeline-{musl}.tar.xz"
    if name not in names:
        raise SystemExit(f"no eb-pipeline archive for {arch} in {sorted(names)}")
    return name

def pick_zig():
    found = [
        name for name in names
        if name.startswith(repo + "-") and name.endswith("-" + zig) and ".cdx" not in name
    ]
    if len(found) != 1:
        raise SystemExit(f"expected one {repo} binary for {zig}, found {found}")
    return found[0]

if repo == "eb-stack":
    asset = pick_stack()
elif repo == "eb-pipeline":
    asset = pick_pipeline()
else:
    asset = pick_zig()

checksum = None
for candidate in (f"{asset}.sha256", "SHA256SUMS", "sha256.sum"):
    if candidate in names:
        checksum = candidate
        break
if checksum is None:
    raise SystemExit(f"no checksum asset next to {asset}")

print(release["tag_name"])
print(asset)
print(assets[asset])
print(checksum)
print(assets[checksum])
PY
}

fetch() {
    local url=$1 dest=$2
    curl --retry 5 --retry-delay 2 --fail --silent --show-error --location \
        --output "$dest" "$url"
}

verify_checksum() {
    local file=$1 sums=$2
    local base hash got
    base=$(basename "$file")
    hash=$(awk -v name="$base" '
        {
            h = $1
            n = $2
            sub(/^\*/, "", n)
            if (n == name) { print h; exit }
        }
    ' "$sums")
    if [[ -z $hash ]]; then
        printf 'checksum file %s has no entry for %s\n' "$sums" "$base" >&2
        exit 1
    fi
    got=$(sha256sum "$file" | awk '{print $1}')
    if [[ $got != "$hash" ]]; then
        printf 'checksum mismatch for %s\n  expected %s\n  got      %s\n' "$base" "$hash" "$got" >&2
        exit 1
    fi
    say "checksum ok: $base"
}

install_tool() {
    local repo=$1 arch=$2 bindir=$3 cache=$4
    local tag asset asset_url sum sum_url
    local -a fields
    mapfile -t fields < <(release_asset "$repo" "$arch")
    if [[ ${#fields[@]} -ne 5 ]]; then
        printf 'could not resolve a release of %s\n' "$repo" >&2
        exit 1
    fi
    tag=${fields[0]}
    asset=${fields[1]}
    asset_url=${fields[2]}
    sum=${fields[3]}
    sum_url=${fields[4]}
    say "$repo $tag ($asset)"
    printf '%s %s\n' "$repo" "$tag" >>"$cache/versions.txt"
    fetch "$asset_url" "$cache/$asset"
    fetch "$sum_url" "$cache/$sum"
    verify_checksum "$cache/$asset" "$cache/$sum"
    case $asset in
        *.tar.xz)
            tar -xJf "$cache/$asset" -C "$cache"
            ;;
        *)
            install -m 0755 "$cache/$asset" "$bindir/$(basename "$repo")"
            ;;
    esac
}

link_unpacked() {
    local cache=$1 bindir=$2
    local path name
    while IFS= read -r path; do
        name=$(basename "$path")
        case $name in
            eb-pipeline | eb-cvmfs | eb-stack | eb-test-map | eb-junit | eb-retry-list | eb-shard-weights)
                install -m 0755 "$path" "$bindir/$name"
                ;;
        esac
    done < <(find "$cache" -type f -perm -0100)
}

dotted_version() {
    local bin=$1
    "$bin" --version | head -n 1 | grep -oE '[0-9]+\.[0-9]+\.[0-9]+' | head -n 1
}

write_sources() {
    local src=$1 root=$2
    local payload tar_stamp tar_fail sum
    mkdir -p "$src" "$root/s/smokestamp" "$root/s/smokefail"
    payload=$src/smoke-stamp
    printf '%s\n' '#!/bin/sh' 'printf "stackhands-smoke\n"' >"$payload"
    chmod 0755 "$payload"
    tar -C "$src" -czf "$src/SmokeStamp-1.0.tar.gz" smoke-stamp
    cp "$src/SmokeStamp-1.0.tar.gz" "$src/SmokeFail-1.0.tar.gz"
    sum=$(sha256sum "$src/SmokeStamp-1.0.tar.gz" | awk '{print $1}')
    tar_stamp=$src/SmokeStamp-1.0.tar.gz
    tar_fail=$src/SmokeFail-1.0.tar.gz
    cat >"$root/s/smokestamp/SmokeStamp-1.0.eb" <<EOF
easyblock = 'Binary'
name = 'SmokeStamp'
version = '1.0'
homepage = 'https://example.org'
description = "One-file install for the integration smoke test"
toolchain = SYSTEM
sources = ['SmokeStamp-1.0.tar.gz']
checksums = ['$sum']
# The Binary easyblock leaves an archive packed unless this is set.
extract_sources = True
install_cmds = [
    'mkdir -p %(installdir)s/bin',
    'cp smoke-stamp %(installdir)s/bin/smoke-stamp',
    'chmod 755 %(installdir)s/bin/smoke-stamp',
]
sanity_check_paths = {
    'files': ['bin/smoke-stamp'],
    'dirs': [],
}
moduleclass = 'tools'
EOF
    cat >"$root/s/smokefail/SmokeFail-1.0.eb" <<EOF
easyblock = 'Binary'
name = 'SmokeFail'
version = '1.0'
homepage = 'https://example.org'
description = "A build that fails, so its log can be kept"
toolchain = SYSTEM
sources = ['SmokeFail-1.0.tar.gz']
checksums = ['$sum']
extract_sources = True
install_cmds = ['false']
moduleclass = 'tools'
EOF
    # The copies above are the sources EasyBuild reads. Record them.
    say "source $tar_stamp"
    say "source $tar_fail"
}

write_site() {
    local site=$1 lists=$2 target=$3 cpu=$4 prefix=$5 software=$6 modules=$7
    local stack=$8 keep=$9 staged=${10} easyconfigs=${11}
    local scratch=${12} tmp=${13} build=${14} sources=${15} logs=${16} state=${17}
    local stack_version keep_version staged_version
    stack_version=$(dotted_version "$stack")
    keep_version=$(dotted_version "$keep")
    staged_version=$(dotted_version "$staged")
    cat >"$site" <<EOF
schema_version = 1
kind = "site"

[site]
name = "smoke"
build_account = "smoke"
stack_generation = "${EESSI_VERSION}"

[site.tools.eb_stack]
bin = "$stack"
min_version = "$stack_version"

[site.tools.eb_keep_logs]
bin = "$keep"
min_version = "$keep_version"

[site.tools.eb_staged_rebuild]
bin = "$staged"
min_version = "$staged_version"

[scheduler]
kind = "local"

[[targets]]
name = "$target"
build_lists = ["smoke.list"]
cpu_target = "$cpu"
site_prefix = "$prefix"
software_root = "$software"
modules_root = "$modules"
default = true

[filesystems]
scratch = "$scratch"
tmp = "$tmp"
build_path = "$build"
source_paths = ["$sources"]
install_root = "$prefix"
log_root = "$logs"

[modules]
init = ["EESSI/${EESSI_VERSION}"]

[logs]
keep_failed = true

[logs.rotate]
compress_after_days = 1
delete_after_days = 7

[rebuild]
staged = true
state_root = "$state"
backend = "installpath"

[shards]
count = 2
easyconfigs = ["$easyconfigs"]

[admission]
enabled = false

[licences]
enabled = false

[sources]
enabled = false
EOF
    printf '%s\n' 'SmokeStamp-1.0.eb' >"$lists/smoke.list"
}

run_plan_step() {
    local plan=$1
    python3 - "$plan" <<'PY'
import json, shlex, sys
plan = json.load(open(sys.argv[1]))
found = None
for phase in plan["phases"]:
    if phase.get("name") == "shard" and phase.get("status") == "planned":
        found = phase["commands"][0]["argv"]
        break
if not found:
    raise SystemExit("the plan has no shard command")
print(" ".join(shlex.quote(part) for part in found))
PY
}

maybe_reexec_apptainer() {
    local init=$1
    if [[ -r $init ]]; then
        return 0
    fi
    if [[ ${SMOKE_EESSI_VIA:-} == host ]]; then
        printf 'EESSI init %s is not readable.\n' "$init" >&2
        exit 1
    fi
    if [[ -n ${SMOKE_INSIDE_APPTAINER:-} || -n ${GITHUB_ACTIONS:-} ]]; then
        printf 'EESSI init %s is not readable.\n' "$init" >&2
        exit 1
    fi
    if [[ ${SMOKE_EESSI_VIA:-apptainer} != apptainer ]]; then
        printf 'EESSI init %s is not readable.\n' "$init" >&2
        exit 1
    fi
    if ! command -v apptainer >/dev/null 2>&1; then
        printf 'EESSI init %s is not readable, and apptainer is not installed.\n' "$init" >&2
        exit 1
    fi
    say "no host mount; running the same script in the EESSI client image"
    exec apptainer exec --cleanenv \
        --bind "$WORK:$WORK" \
        --env SMOKE_INSIDE_APPTAINER=1 \
        --env SMOKE_WORK="$WORK" \
        --env SMOKE_EESSI_VERSION="$EESSI_VERSION" \
        --env HOME="${HOME:-/tmp}" \
        docker://ghcr.io/eessi/client:debian12 \
        bash "$0"
}

main() {
    local arch init bindir cache prefix src easyconfigs lists
    local scratch tmp build logs state kept rotate_root
    local target cpu software modules site plan shard_cmd
    local stamp fail_status mod_file shard_status
    local -a eb_common

    EESSI_VERSION=${SMOKE_EESSI_VERSION:-2026.06}
    export EESSI_VERSION
    WORK=${SMOKE_WORK:-$(mktemp -d "${TMPDIR:-/tmp}/integration-smoke.XXXXXX")}
    export WORK
    mkdir -p "$WORK"
    say "work directory: $WORK"
    init=/cvmfs/software.eessi.io/versions/${EESSI_VERSION}/init/lmod/bash
    maybe_reexec_apptainer "$init"

    step "download published releases"
    arch=$(machine)
    bindir=$WORK/bin
    cache=$WORK/cache
    mkdir -p "$bindir" "$cache"
    : >"$cache/versions.txt"
    install_tool eessi-arch-guard "$arch" "$bindir" "$cache"
    install_tool eb-local "$arch" "$bindir" "$cache"
    install_tool eb-keep-logs "$arch" "$bindir" "$cache"
    install_tool eb-staged-rebuild "$arch" "$bindir" "$cache"
    install_tool eb-pipeline "$arch" "$bindir" "$cache"
    install_tool eb-stack "$arch" "$bindir" "$cache"
    link_unpacked "$cache" "$bindir"
    export PATH="$bindir:$PATH"
    say "tools:"
    command -v eessi-arch-guard
    command -v eb-local
    command -v eb-keep-logs
    command -v eb-staged-rebuild
    command -v eb-pipeline
    command -v eb-stack
    eessi-arch-guard --version
    eb-local --version
    eb-keep-logs --version
    eb-staged-rebuild --version
    eb-pipeline --version
    eb-stack --version

    step "load EESSI ${EESSI_VERSION}"
    # A previous shell may point EasyBuild outside the prefix this run owns.
    unset EASYBUILD_INSTALLPATH EASYBUILD_INSTALLPATH_SOFTWARE \
        EASYBUILD_INSTALLPATH_MODULES EASYBUILD_PREFIX || true
    unset EESSI_PROJECT_INSTALL EESSI_SITE_INSTALL EESSI_CVMFS_INSTALL || true
    source_eessi "$init"
    say "module subdir before the guard: ${EESSI_SOFTWARE_SUBDIR:-unset}"

    step "eessi-arch-guard decides the target"
    local guard_text
    guard_text=$(eessi-arch-guard --shell 2>"$WORK/arch-guard.err") || {
        printf 'eessi-arch-guard failed\n' >&2
        cat "$WORK/arch-guard.err" >&2
        exit 1
    }
    printf '%s\n' "$guard_text" >"$WORK/arch-guard.shell"
    apply_guard <"$WORK/arch-guard.shell"
    export PATH="$bindir:${PATH:-}"
    cpu=${EESSI_SOFTWARE_SUBDIR:?eessi-arch-guard left EESSI_SOFTWARE_SUBDIR unset}
    target=${cpu//\//-}
    say "target $target (cpu_target $cpu)"
    printf '%s\n' "$cpu" >"$WORK/cpu-target.txt"

    step "prepare a one-file easyconfig"
    prefix=$WORK/prefix
    src=$WORK/sources
    easyconfigs=$WORK/easyconfigs
    scratch=$WORK/scratch
    tmp=$WORK/tmp
    build=$WORK/build
    logs=$WORK/logs
    state=$WORK/staged-state
    kept=$WORK/kept
    lists=$WORK/lists
    mkdir -p "$prefix" "$scratch" "$tmp" "$build" "$logs" "$state" "$kept" "$lists" "$WORK/working"
    write_sources "$src" "$easyconfigs"
    export EESSI_USER_INSTALL=$prefix
    export WORKING_DIR=$WORK/working
    export EASYBUILD_SOURCEPATH=$src
    eb_common=(
        --sourcepath="$src"
        --tmpdir="$tmp"
        --tmp-logdir="$logs/easybuild"
    )
    mkdir -p "$logs/easybuild"

    step "eb-local builds SmokeStamp through EESSI-extend"
    # eb-local <easyconfig> refuses to build unless SLURM_JOB_ID is set.
    # eb-local eb is the released command that loads EESSI-extend and runs
    # eb with no Slurm job, which is this host and a GitHub-hosted runner.
    eb-local eb -- "${eb_common[@]}" "$easyconfigs/s/smokestamp/SmokeStamp-1.0.eb"
    mod_file=$(find "$prefix" -path '*/all/SmokeStamp/1.0.lua' -print -quit)
    if [[ -z $mod_file ]]; then
        printf 'SmokeStamp was not installed under %s\n' "$prefix" >&2
        find "$prefix" -maxdepth 6 -type d -print >&2
        exit 1
    fi
    say "module $mod_file"
    case $mod_file in
        */"$cpu"/*) say "install is on the target the arch guard chose" ;;
        *)
            printf 'SmokeStamp landed at %s, not under the target %s\n' "$mod_file" "$cpu" >&2
            exit 1
            ;;
    esac
    modules=$(dirname "$(dirname "$(dirname "$mod_file")")")
    software=$(dirname "$modules")/software
    if [[ ! -d $software ]]; then
        printf 'software directory %s is missing\n' "$software" >&2
        exit 1
    fi
    say "software $software"
    say "modules $modules"

    step "eb-local records a failed build for eb-keep-logs"
    unset EASYBUILD_INSTALLPATH EASYBUILD_INSTALLPATH_SOFTWARE \
        EASYBUILD_INSTALLPATH_MODULES EASYBUILD_PREFIX || true
    set +e
    eb-local eb -- "${eb_common[@]}" "$easyconfigs/s/smokefail/SmokeFail-1.0.eb"
    fail_status=$?
    set -e
    if [[ $fail_status -eq 0 ]]; then
        printf 'SmokeFail was supposed to fail\n' >&2
        exit 1
    fi
    say "SmokeFail exit $fail_status"
    # eb-keep-logs and eb-staged-rebuild call the eb binary. EESSI-extend,
    # which `eb-local eb` loads in its own process, loads the newest
    # EasyBuild. Load that module here. Loading EESSI-extend in this shell
    # would also set the install path, which the staged build then has to
    # undo.
    if ! command -v eb >/dev/null 2>&1; then
        set +u
        module load EasyBuild
        set -u
    fi
    command -v eb
    export EASYBUILD_TMPDIR=$tmp
    eb-keep-logs failed "$fail_status" SmokeFail-1.0 "$kept" "$logs/easybuild"
    if [[ ! -f $kept/SmokeFail-1.0.failed.ebout.log ]]; then
        printf 'eb-keep-logs did not keep the EasyBuild log in %s\n' "$kept" >&2
        ls -la "$kept" >&2
        exit 1
    fi
    say "kept $(find "$kept" -type f -printf '%f\n' | sort | paste -sd' ' -)"

    step "eb-keep-logs prunes old runs"
    rotate_root=$WORK/rotate
    mkdir -p "$rotate_root"
    local old mid
    old=$(date -d '10 days ago' +%Y%m%d)_120000
    mid=$(date -d '3 days ago' +%Y%m%d)_120000
    mkdir -p "$rotate_root/$old" "$rotate_root/$mid"
    printf '%s\n' 'old run' >"$rotate_root/$old/build.log"
    printf '%s\n' 'middle run' >"$rotate_root/$mid/build.log"
    unset EB_PIPELINE_LOG_DIR || true
    eb-keep-logs rotate "$rotate_root" --compress-after 1 --delete-after 7
    if [[ -e $rotate_root/$old ]]; then
        printf 'run %s was not deleted\n' "$old" >&2
        exit 1
    fi
    if [[ ! -f $rotate_root/$mid/build.log.gz || -f $rotate_root/$mid/build.log ]]; then
        printf 'run %s was not compressed\n' "$mid" >&2
        find "$rotate_root" -type f -print >&2
        exit 1
    fi
    say "pruned $old and compressed $mid"

    step "eb-staged-rebuild stages and swaps the install"
    eb-staged-rebuild probe --json --path "$software" --eb "$(command -v eb)" \
        >"$WORK/probe.json" || true
    say "probe written to $WORK/probe.json"
    stamp=$(date -u +%Y%m%d%H%M%S)
    eb-staged-rebuild run \
        --software "$software" \
        --modules "$modules" \
        --state-root "$state" \
        --eb "$(command -v eb)" \
        --backend installpath \
        --run-id "smoke-$stamp" \
        --lock-timeout 120 \
        --module SmokeStamp/1.0 \
        -- "$easyconfigs/s/smokestamp/SmokeStamp-1.0.eb" --rebuild "${eb_common[@]}"
    eb-staged-rebuild status --state-root "$state" "smoke-$stamp"
    say "staged run smoke-$stamp published"

    step "eb-stack plans the list and eb-pipeline check accepts the site"
    write_site "$lists/site.toml" "$lists" "$target" "$cpu" "$prefix" \
        "$software" "$modules" "$(command -v eb-stack)" "$(command -v eb-keep-logs)" \
        "$(command -v eb-staged-rebuild)" "$easyconfigs" "$scratch" "$tmp" \
        "$build" "$src" "$logs" "$state"
    eb-pipeline check --site "$lists/site.toml" --lists-dir "$lists" --probe-tools
    eb-stack stack order \
        --easyconfigs "$easyconfigs" \
        --root 'SmokeStamp==1.0' \
        --out "$WORK/build-order.txt" \
        --easystack-out "$WORK/smoke.yml"
    say "order:"
    cat "$WORK/build-order.txt"
    eb-pipeline plan \
        --site "$lists/site.toml" \
        --list "$lists/smoke.list" \
        --target "$target" \
        --build-type production \
        --run-id "smoke-$stamp" \
        --log-stamp "${stamp:0:8}_120000" \
        --out "$WORK/plan.json"
    shard_cmd=$(run_plan_step "$WORK/plan.json")
    say "plan step: $shard_cmd"
    # stack shard exits 3 when it leaves the list unsharded. eb-pipeline
    # treats that as a plan and builds the list as one unit.
    set +e
    bash -c "$shard_cmd"
    shard_status=$?
    set -e
    if [[ $shard_status -ne 0 && $shard_status -ne 3 ]]; then
        printf 'stack shard exited %s\n' "$shard_status" >&2
        exit "$shard_status"
    fi
    say "stack shard exit $shard_status"
    say "eb-pipeline check and the shard plan step finished"

    step "eb-cvmfs against a stub cvmfs_server"
    if [[ ! -x $bindir/eb-cvmfs ]]; then
        printf 'the published eb-pipeline release does not ship eb-cvmfs\n' >&2
        printf 'unpacked tools:\n' >&2
        find "$cache" -type f -perm -0100 -printf '%p\n' >&2
        exit 1
    fi
    local stub calls sync
    stub=$WORK/cvmfs-stub
    calls=$WORK/cvmfs-calls
    mkdir -p "$stub"
    cat >"$stub/cvmfs_server" <<'EOF'
#!/bin/sh
echo "$*" >> "$CALLS"
case "$1" in
    list)
        if [ -n "${STUB_IN_TRANSACTION:-}" ]; then
            echo "$REPOSITORY (stratum0 / local) - in transaction"
        else
            echo "$REPOSITORY (stratum0 / local) - healthy"
        fi
        exit 0
        ;;
    transaction) exit 0 ;;
    publish) exit 0 ;;
    abort) exit 0 ;;
esac
exit 99
EOF
    sync=$stub/sync-cache
    cat >"$sync" <<'EOF'
#!/bin/sh
echo "sync $*" >> "$CALLS"
exit 0
EOF
    chmod 0755 "$stub/cvmfs_server" "$sync"
    : >"$calls"
    PATH="$stub:$PATH" CALLS="$calls" REPOSITORY=repo.example \
        eb-cvmfs repo.example -- "$sync" --marker
    say "cvmfs calls:"
    cat "$calls"
    python3 - "$calls" <<'PY'
import pathlib, sys
got = pathlib.Path(sys.argv[1]).read_text()
want = "list\ntransaction repo.example\nsync --marker\npublish repo.example\n"
if got != want:
    raise SystemExit(f"unexpected cvmfs_server calls:\n{got}")
PY
    say "eb-cvmfs published the stub transaction"

    step "done"
    say "integration smoke passed"
    say "target $target"
    cat "$cache/versions.txt"
}

main "$@"
