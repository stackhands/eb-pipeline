#!/bin/bash
# The install steps of the tutorial and install how-to name Rust 1.90 and
# `cargo +1.90 install` when the host default toolchain may be older.
# Fails until those pages carry the rustup and +1.90 lines.
set -euo pipefail

root=$(CDPATH='' cd -- "$(dirname -- "${BASH_SOURCE[0]}")/../.." && pwd)
pages=$root/docs/orgmode

need() {
    local file=$1
    shift
    local text
    text=$(cat "$file")
    for pat in "$@"; do
        if ! grep -Fq "$pat" <<<"$text"; then
            echo "missing in $file: $pat" >&2
            exit 1
        fi
    done
}

need "$pages/tutorials/first-build-list.org" \
    'rustup toolchain install 1.90' \
    'cargo +1.90 install --locked --path .'

need "$pages/howto/install.org" \
    'rustup toolchain install 1.90' \
    'cargo +1.90 install --locked --path . --root ~/.local'