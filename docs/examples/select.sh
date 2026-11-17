#!/bin/bash
# What docs/orgmode/howto/select-lines.org shows: the site file of
# build-from-ci/site.toml with a [targets.select] table in each target, a
# build list with a line for zen4 only, and what each target plans and runs
# of it, the last with a copy of the site file that has the local scheduler,
# no modules and its paths under $WORK, and the stand-in eb of the tutorial.
# shellcheck disable=SC2016 # commands stay quoted as the pages show them
# shellcheck source-path=SCRIPTDIR source=lib.sh
. "$(dirname -- "$0")/lib.sh"

export WORK=$HOME/select
mkdir -p "$WORK"
cd "$WORK"
# The select table of each target goes after its [targets.phases] table.
select_table() {
    printf '\n[targets.select]\noption = "--only-targets"\nvalue = "%s"\nstrip = true\n' "$1"
}
while IFS= read -r line; do
    printf '%s\n' "$line"
    case $line in
        'build = "-p cpu -C zen2 '*) select_table zen2 ;;
        'build = "-p cpu -C zen4 '*) select_table zen4 ;;
    esac
done <"$pages/howto/build-from-ci/site.toml" >site.toml
printf '%s\n' Example-1.0-GCCcore-15.2.0.eb 'Example-2.0-GCCcore-15.2.0.eb --only-targets=zen4' >build.list

run 'eb-pipeline check --site site.toml'
expect_status 0

run 'cat build.list'

run 'eb-pipeline plan --site site.toml --list build.list --target zen2 --build-type production | grep -A5 "\"deselected\""'
expect_status 0
expect_line '      "easyconfig": "Example-2.0-GCCcore-15.2.0.eb",'

sed 's/^option = "--only-targets"$/option = "only-targets"/' site.toml >no-dashes.toml
run 'eb-pipeline check --site no-dashes.toml'
expect_status 2

mkdir local
sed -e 's/^kind = "slurm"/kind = "local"/' \
    -e "s#/opt/site#$WORK/site#g" \
    -e "s#/scratch/alice#$WORK/scratch#g" \
    -e 's/^init = \["EasyBuild"\]/init = []/' site.toml >local/site.toml
cp build.list local/
cd local
# shellcheck source=../orgmode/tutorials/first-build-list/eb.sh
. "$pages/tutorials/first-build-list/eb.sh"
export PATH=$WORK/local/bin:$PATH

run 'eb-pipeline run --site site.toml --list build.list --target zen4 --build-type production --dry-run | grep "^\[build\]"
eb-pipeline run --site site.toml --list build.list --target zen2 --build-type production --dry-run | grep "^\[build\]"'
expect_status 0

run 'eb-pipeline run --site site.toml --list build.list --target zen2 --build-type production --log-stamp 20261117_090000 --result result.json; echo "exit $?"'
expect_line 'exit 0'

run 'cat $WORK/site/logs/20261117_090000/zen2/selected-zen2.list
grep -A4 "\"deselected\"" result.json'
expect_line 'Example-1.0-GCCcore-15.2.0.eb'
