#!/bin/bash
# What docs/orgmode/howto/build-from-ci.org shows: the targets of
# build-from-ci/site.toml and the paths each one gets, then
# build-from-ci/build-all.sh run on a copy of that site file with the local
# scheduler, no modules and its paths under $WORK, with the stand-in eb of
# the tutorial, which fails the lines whose easyconfig starts with Broken-.
# shellcheck disable=SC2016 # commands stay quoted as the pages show them
# shellcheck source-path=SCRIPTDIR source=lib.sh
. "$(dirname -- "$0")/lib.sh"

export WORK=$HOME/ci
mkdir -p "$WORK"
cd "$WORK"
cp "$pages/howto/build-from-ci/site.toml" site.toml

run 'eb-pipeline targets --site site.toml
eb-pipeline targets --site site.toml --format json'
expect_status 0
expect_line '["zen2","zen4"]'

run 'for t in $(eb-pipeline targets --site site.toml); do
    eb-pipeline explain --site site.toml --target "$t" | grep -E "BUILDPATH|SOURCEPATH"
done'
expect_status 0
expect_line 'EASYBUILD_BUILDPATH = "/scratch/alice/build/zen2"'
expect_line 'EASYBUILD_SOURCEPATH = "/scratch/alice/sources/zen4:/opt/site/sources"'

sed 's/^kind = "slurm"/kind = "local"/' site.toml >broken.toml
sed -i 's/^name = "zen4"/name = "zen2"/' broken.toml
run 'eb-pipeline targets --site broken.toml'
expect_status 2

mkdir local
sed -e 's/^kind = "slurm"/kind = "local"/' \
    -e "s#/opt/site#$WORK/site#g" \
    -e "s#/scratch/alice#$WORK/scratch#g" \
    -e 's/^init = \["EasyBuild"\]/init = []/' site.toml >local/site.toml
cp "$pages/howto/build-from-ci/build-all.sh" local/build-all.sh
cd local
# shellcheck source=../orgmode/tutorials/first-build-list/eb.sh
. "$pages/tutorials/first-build-list/eb.sh"
export PATH=$WORK/local/bin:$PATH
printf '%s\n' Example-1.0-GCCcore-15.2.0.eb Broken-1.0-GCCcore-15.2.0.eb >build.list

run 'bash build-all.sh build.list production 1234; echo "exit $?"'
expect_line 'zen2: exit 1'
expect_line 'exit 1'

run "grep -H '^  \"exit_code\"' result-*.json"
expect_line 'result-zen4.json:  "exit_code": 1,'

run 'bash build-all.sh missing.list production 1235; echo "exit $?"'
expect_line 'ERROR [main] -: cannot read list missing.list: No such file or directory (os error 2)'
expect_line 'exit 2'
