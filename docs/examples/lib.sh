# Shared by the scripts in this directory, which source it.
#
# A script runs the commands of one page in order, the way a reader types
# them: in a home directory of its own, which mktemp makes and the script
# removes on exit, with the eb-pipeline of this checkout on PATH as
# ~/.cargo/bin/eb-pipeline. Build it first with `cargo build --locked`, or
# name another binary in DOCS_EB_PIPELINE.
#
# Each command prints as `+ COMMAND`, then what it wrote to stdout and
# stderr, as a terminal shows them, with the work directory written as $WORK
# and the home directory as $HOME. The pages paste that output.

set -euo pipefail

root=$(CDPATH='' cd -- "$(dirname -- "${BASH_SOURCE[0]}")/../.." && pwd)
# shellcheck disable=SC2034 # for the scripts that source this file
pages=$root/docs/orgmode
bin=${DOCS_EB_PIPELINE:-$root/target/debug/eb-pipeline}
if [ ! -x "$bin" ]; then
    echo "no eb-pipeline at $bin; run cargo build --locked first" >&2
    exit 2
fi
bin=$(CDPATH='' cd -- "$(dirname -- "$bin")" && pwd)/$(basename -- "$bin")

# eb-pipeline takes flags and site file values from these.
for name in "${!EB_PIPELINE_@}"; do
    unset "$name"
done

HOME=$(mktemp -d /tmp/eb-pipeline-docs.XXXXXX)
export HOME
trap 'rm -rf "$HOME"' EXIT
mkdir -p "$HOME/.cargo/bin"
ln -s "$bin" "$HOME/.cargo/bin/eb-pipeline"
export PATH=$HOME/.cargo/bin:/usr/bin:/bin
export LC_ALL=C
cd "$HOME"

# The exit status and the output of the last command `run` ran.
status=0
output=

# Run one command, or a block of them, as the page shows it.
run() {
    local log
    log=$(mktemp /tmp/eb-pipeline-docs-run.XXXXXX)
    printf '+ %s\n' "$1"
    set +e
    eval "$1" >"$log" 2>&1
    status=$?
    set -e
    output=$(rewrite <"$log")
    rm -f "$log"
    if [ -n "$output" ]; then
        printf '%s\n' "$output"
    fi
}

# Write the work directory as $WORK and the home directory as $HOME.
rewrite() {
    if [ -n "${WORK:-}" ]; then
        sed -e "s#$WORK#\$WORK#g" -e "s#$HOME#\$HOME#g"
    else
        sed -e "s#$HOME#\$HOME#g"
    fi
}

# Stop unless the last command exited with status $1.
expect_status() {
    if [ "$status" -ne "$1" ]; then
        echo "expected exit status $1, got $status" >&2
        exit 1
    fi
}

# Stop unless the output of the last command has the line $1.
expect_line() {
    if ! grep -qxF -- "$1" <<<"$output"; then
        echo "expected the line: $1" >&2
        exit 1
    fi
}

# The version in Cargo.toml, which `eb-pipeline --version` prints.
crate_version() {
    sed -n 's/^version = "\(.*\)"$/\1/p' "$root/Cargo.toml" | head -n 1
}
