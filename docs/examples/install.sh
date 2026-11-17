#!/bin/bash
# The installs that docs/orgmode/howto/install.org shows: release 0.3.0 from
# GitHub, checked against its checksum, and a build from a clone of this
# checkout with cargo install, then the failures its table lists.
#
# It needs the network and a Rust toolchain of 1.90 or newer, so the docs
# job of the CI workflow does not run it. Pick the toolchain with
# RUSTUP_TOOLCHAIN, as in RUSTUP_TOOLCHAIN=1.90.0 bash docs/examples/install.sh;
# OLD_TOOLCHAIN names one older than 1.90 for the failure, 1.83.0 unless set.
# shellcheck disable=SC2016 # commands stay quoted as the pages show them

# lib.sh moves HOME and PATH; keep the toolchain of the user who runs this.
cargo=$(command -v cargo)
export RUSTUP_HOME=${RUSTUP_HOME:-$HOME/.rustup}
export CARGO_HOME=${CARGO_HOME:-$HOME/.cargo}
old_toolchain=${OLD_TOOLCHAIN:-1.83.0}
# shellcheck source-path=SCRIPTDIR source=lib.sh
. "$(dirname -- "$0")/lib.sh"

export WORK=$HOME/install
mkdir -p "$WORK" "$HOME/.local/bin"
cd "$WORK"
# The pages run the eb-pipeline they install, not the one lib.sh links.
rm "$HOME/.cargo/bin/eb-pipeline"
PATH=$HOME/.local/bin:$PATH:$(dirname -- "$cargo")
export PATH

release=https://github.com/stackhands/eb-pipeline/releases/download/v0.3.0
archive=eb-pipeline-x86_64-unknown-linux-musl.tar.xz

run "curl -fsSLO $release/$archive
curl -fsSLO $release/$archive.sha256
sha256sum -c $archive.sha256"
expect_status 0
expect_line "$archive: OK"

run "tar -xJf $archive
ls ${archive%.tar.xz}"
expect_status 0
for cmd in eb-pipeline eb-retry-list eb-shard-weights eb-test-map eb-junit eb-cvmfs; do
    expect_line "$cmd"
done

run "for cmd in eb-pipeline eb-retry-list eb-shard-weights eb-test-map eb-junit eb-cvmfs; do
install -D -m 755 ${archive%.tar.xz}/\$cmd ~/.local/bin/\$cmd
done
for cmd in eb-pipeline eb-retry-list eb-shard-weights eb-test-map eb-junit eb-cvmfs; do
command -v \$cmd
\$cmd --help >/dev/null
done
eb-pipeline --version"
expect_status 0
expect_line 'eb-pipeline 0.3.0'

run 'eb-pipeline targets --site site.toml'
expect_status 2
expect_line "error: unrecognized subcommand 'targets'"

git clone --quiet "$root" eb-pipeline
cd eb-pipeline

run 'cargo install --locked --path . --root ~/.local'
expect_status 101
expect_line 'error: binary `eb-pipeline` already exists in destination'
rm ~/.local/bin/eb-pipeline

run "cargo +$old_toolchain install --locked --path . --root ~/.local"
expect_status 101

run 'cargo install --locked --quiet --path . --root ~/.local
command -v eb-pipeline
eb-pipeline --version'
expect_status 0
expect_line "eb-pipeline $(crate_version)"

run 'eb-pipeline targets --help | head -n 1'
expect_status 0
expect_line 'Print the target names in file order, for a CI matrix or a shell loop'

# An archive that is not the one the checksum names.
cd "$WORK"
printf x >>"$archive"
run "sha256sum -c $archive.sha256"
expect_status 1
expect_line "$archive: FAILED"
