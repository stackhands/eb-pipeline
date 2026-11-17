#!/bin/bash
# The commands of the tutorial, docs/orgmode/tutorials/first-build-list.org,
# in order, from "Install eb-pipeline" on; lib.sh puts the eb-pipeline of this
# checkout where `cargo install` puts it. Checks the exit statuses and the
# lines the page explains.
# shellcheck disable=SC2016 # commands stay quoted as the page shows them
# shellcheck source-path=SCRIPTDIR source=lib.sh
. "$(dirname -- "$0")/lib.sh"
snippets=$pages/tutorials/first-build-list

run 'eb-pipeline --version'
expect_status 0
expect_line "eb-pipeline $(crate_version)"

run 'export WORK=$HOME/eb-tutorial
mkdir -p "$WORK"
cd "$WORK"'
expect_status 0

run "$(cat "$snippets/eb.sh")"
expect_status 0

run 'export PATH="$WORK/bin:$PATH"
command -v eb'
expect_line '$WORK/bin/eb'

run "$(cat "$snippets/site.sh")"
expect_status 0

run 'eb-pipeline check --site site.toml'
expect_status 0
expect_line 'INFO [check] example: site file is valid'

run "cat > build.list <<'EOF'
Example-1.0-GCCcore-15.2.0.eb
zlib-1.3.1-GCCcore-15.2.0.eb --rebuild
EOF"
expect_status 0

run 'eb-pipeline run --site site.toml --list build.list --target cpu --build-type production --dry-run'
expect_status 0
expect_line '[build] eb Example-1.0-GCCcore-15.2.0.eb'
expect_line '[build] eb zlib-1.3.1-GCCcore-15.2.0.eb --rebuild'
expect_line 'INFO [run] cpu: dry run: nothing executed'

run 'eb-pipeline run --site site.toml --list build.list --target cpu --build-type production --result result.json
echo $?'
expect_line 'INFO [build] cpu: Example-1.0-GCCcore-15.2.0: ok'
expect_line 'INFO [build] cpu: zlib-1.3.1-GCCcore-15.2.0: ok'
expect_line 0

run 'cat result.json'
expect_line '  "exit_code": 0,'
expect_line '    "success": 2'

run 'cat logs/*/cpu/Example-1.0-GCCcore-15.2.0.stdout.log'
expect_status 0
expect_line 'eb Example-1.0-GCCcore-15.2.0.eb'
expect_line 'EASYBUILD_BUILDPATH=$WORK/scratch/build'

# The log directory is named after the second the run starts in, and the next
# run needs a directory of its own.
sleep 1

run "cat > build.list <<'EOF'
Example-1.0-GCCcore-15.2.0.eb
Broken-1.0-GCCcore-15.2.0.eb
zlib-1.3.1-GCCcore-15.2.0.eb --rebuild
EOF"
expect_status 0

run 'eb-pipeline run --site site.toml --list build.list --target cpu --build-type production --result result.json
echo $?'
expect_line 'ERROR [build] cpu: Broken-1.0-GCCcore-15.2.0: failed (exit 1)'
expect_line 'INFO [build] cpu: zlib-1.3.1-GCCcore-15.2.0: ok'
expect_line 1

run "grep -A3 '\"counts\"' result.json"
expect_line '    "failed": 1,'
expect_line '    "success": 2'

run 'cat logs/*/cpu/Broken-1.0-GCCcore-15.2.0.stdout.log'
expect_status 0
expect_line 'ERROR: cannot build Broken-1.0-GCCcore-15.2.0.eb'

run 'cd
rm -rf "$WORK"'
expect_status 0
