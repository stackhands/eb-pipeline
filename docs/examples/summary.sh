#!/bin/bash
# What docs/orgmode/reference/run-summary.org shows: both targets of a copy
# of build-from-ci/site.toml with the local scheduler, no modules and its
# paths under $WORK, run with one log stamp and the stand-in eb of the
# tutorial, which fails the lines whose easyconfig starts with Broken-; the
# summary the two runs wrote together, and a run that cannot write one.
# shellcheck disable=SC2016 # commands stay quoted as the pages show them
# shellcheck source-path=SCRIPTDIR source=lib.sh
. "$(dirname -- "$0")/lib.sh"

export WORK=$HOME/summary
mkdir -p "$WORK"
cd "$WORK"
sed -e 's/^kind = "slurm"/kind = "local"/' \
    -e "s#/opt/site#$WORK/site#g" \
    -e "s#/scratch/alice#$WORK/scratch#g" \
    -e 's/^init = \["EasyBuild"\]/init = []/' "$pages/howto/build-from-ci/site.toml" >site.toml
printf '%s\n' Example-1.0-GCCcore-15.2.0.eb Broken-1.0-GCCcore-15.2.0.eb >build.list
printf '%s\n' Example-1.0-GCCcore-15.2.0.eb >example.list
# shellcheck source=../orgmode/tutorials/first-build-list/eb.sh
. "$pages/tutorials/first-build-list/eb.sh"
export PATH=$WORK/bin:$PATH

run 'eb-pipeline run --site site.toml --list build.list --target zen2 --build-type production --log-stamp 20261117_090000; echo "exit $?"
eb-pipeline run --site site.toml --list example.list --target zen4 --build-type production --log-stamp 20261117_090000; echo "exit $?"'
expect_line 'exit 1'
expect_line 'exit 0'
expect_line 'INFO [summary] zen2: summary.json and summary.html written in $WORK/site/logs/20261117_090000'
expect_line 'INFO [summary] zen4: summary.json and summary.html written in $WORK/site/logs/20261117_090000'

run 'ls -A site/logs/20261117_090000'
expect_line '.summary.lock'
expect_line 'summary.html'
expect_line 'summary.json'

run 'cat site/logs/20261117_090000/summary.json'
expect_line '  "exit_code": 1,'
expect_line '  "status": "failed",'

run 'grep -o "<title>.*</title>" site/logs/20261117_090000/summary.html
grep -o "href=\"[^\"]*\"" site/logs/20261117_090000/summary.html | sort -u'
expect_line '<title>eb-pipeline run 20261117_090000: failed</title>'
expect_line 'href="zen2/Broken-1.0-GCCcore-15.2.0.stdout.log"'

run 'mkdir -p site/logs/20261117_100000/summary.json
eb-pipeline run --site site.toml --list example.list --target zen4 --build-type production --log-stamp 20261117_100000; echo "exit $?"'
expect_line 'exit 0'
expect_line 'WARN [summary] zen4: no summary written: $WORK/site/logs/20261117_100000/summary.json: Is a directory (os error 21)'
