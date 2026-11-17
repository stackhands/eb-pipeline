#!/bin/bash
# What docs/orgmode/howto/from-eb-stack.org shows: the easystack that
# `eb-stack rebuild-set` wrote and the build list that `eb-stack stack solve
# --jenkins-build-list-out` wrote, the files of tests/fixtures/eb-stack, given
# as they are to the site file and the stand-in eb of the tutorial, and an
# easystack with a typing error.
# shellcheck disable=SC2016 # commands stay quoted as the pages show them
# shellcheck source-path=SCRIPTDIR source=lib.sh
. "$(dirname -- "$0")/lib.sh"

export WORK=$HOME/from-eb-stack
mkdir -p "$WORK/rebuild-set/rebuilds"
cd "$WORK"
# shellcheck source=../orgmode/tutorials/first-build-list/site.sh
. "$pages/tutorials/first-build-list/site.sh"
# shellcheck source=../orgmode/tutorials/first-build-list/eb.sh
. "$pages/tutorials/first-build-list/eb.sh"
export PATH=$WORK/bin:$PATH
fixtures=$root/tests/fixtures/eb-stack
cp "$fixtures/rebuild-set.yml" rebuild-set/rebuilds/20261117-eb-unknown-cve-patches.yml
cp "$fixtures/solve.jenkins.list" stack.list

run 'cat rebuild-set/rebuilds/20261117-eb-unknown-cve-patches.yml'
expect_status 0

run 'eb-pipeline run --site site.toml --list rebuild-set/rebuilds/20261117-eb-unknown-cve-patches.yml --target cpu --build-type production --dry-run | grep "^\[build\]"'
expect_status 0
expect_line '[build] eb pigz-2.8-GCCcore-15.2.0.eb --rebuild'

run 'cat stack.list
eb-pipeline run --site site.toml --list stack.list --target cpu --build-type production --dry-run | grep "^\[build\]"'
expect_status 0
expect_line '[build] eb M4-1.4.20-GCCcore-15.2.0.eb'

run 'eb-pipeline run --site site.toml --list rebuild-set/rebuilds/20261117-eb-unknown-cve-patches.yml --target cpu --build-type production --log-stamp 20261117_090000; echo "exit $?"'
expect_line 'exit 0'

run 'grep "^eb " $WORK/logs/20261117_090000/cpu/pigz-2.8-GCCcore-15.2.0.stdout.log'
expect_line 'eb pigz-2.8-GCCcore-15.2.0.eb --rebuild'

sed 's/^      options:$/      option:/' rebuild-set/rebuilds/20261117-eb-unknown-cve-patches.yml >typo.yml
run 'eb-pipeline run --site site.toml --list typo.yml --target cpu --build-type production --dry-run'
expect_status 2
