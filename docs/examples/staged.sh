#!/bin/bash
# What docs/orgmode/howto/staged-rebuilds.org shows: the site file of
# build-from-ci/site.toml with a [rebuild] table, checked, and the site files
# that check refuses; then a copy of it with the local scheduler, no modules
# and its paths under $WORK, which rebuilds an installed package through a
# stand-in for eb-staged-rebuild.
#
# The stand-in prints its arguments and exits with STAGED_EXIT, 0 unless set.
# With 2 it first prints, on stderr, the refusal of eb-staged-rebuild that
# finds the lock of its state root held.
# shellcheck disable=SC2016 # commands stay quoted as the pages show them
# shellcheck source-path=SCRIPTDIR source=lib.sh
. "$(dirname -- "$0")/lib.sh"

export WORK=$HOME/staged
mkdir -p "$WORK"
cd "$WORK"
cp "$pages/howto/build-from-ci/site.toml" site.toml
cat >>site.toml <<'EOF'

[rebuild]
state_root = "/opt/site/{target}/.eb-staged-rebuild"
lock_timeout = 600
EOF

run 'eb-pipeline check --site site.toml'
expect_status 0

# One ledger configuration for both targets.
sed 's#^lock_timeout = 600$#&\nledger_config = "/opt/site/ledger.json"#' site.toml >shared-ledger.toml
run 'eb-pipeline check --site shared-ledger.toml'
expect_status 2

# One software tree for both targets.
sed 's#^site_prefix = "/opt/site/zen[24]"$#site_prefix = "/opt/site"#' site.toml >shared-tree.toml
run 'eb-pipeline check --site shared-tree.toml'
expect_status 2

mkdir local
sed -e 's/^kind = "slurm"/kind = "local"/' \
    -e "s#/opt/site#$WORK/site#g" \
    -e "s#/scratch/alice#$WORK/scratch#g" \
    -e 's/^init = \["EasyBuild"\]/init = []/' site.toml >local/site.toml
cd local
# shellcheck source=../orgmode/tutorials/first-build-list/eb.sh
. "$pages/tutorials/first-build-list/eb.sh"
cat >bin/eb-staged-rebuild <<'EOF'
#!/bin/sh
echo "eb-staged-rebuild $*"
state=
while [ $# -gt 0 ]; do
    case $1 in
        --state-root) state=$2; shift ;;
        --) break ;;
    esac
    shift
done
if [ "${STAGED_EXIT:-0}" -eq 2 ]; then
    echo "eb-staged-rebuild: another eb-staged-rebuild holds the lock at $state/.lock" >&2
fi
exit "${STAGED_EXIT:-0}"
EOF
chmod +x bin/eb-staged-rebuild
export PATH=$WORK/local/bin:$PATH
mkdir -p "$WORK/site/zen4/modules/all/Example"
: >"$WORK/site/zen4/modules/all/Example/1.0-GCCcore-15.2.0.lua"
echo 'Example-1.0-GCCcore-15.2.0.eb --rebuild' >zen4.list
cp zen4.list zen2.list
args='--site site.toml --list zen4.list --target zen4 --build-type production'

run 'ls $WORK/site/zen4/modules/all/Example'
expect_line '1.0-GCCcore-15.2.0.lua'

run 'eb-pipeline run '"$args"' --run-id 1234 --dry-run | grep "^\[build\]"'
expect_status 0

run 'eb-pipeline run '"$args"' --run-id 1234 --log-stamp 20261117_090000 --result result.json; echo "exit $?"'
expect_line 'exit 0'

run "grep -A3 '\"easyconfig\": \"Example' result.json"
expect_line '      "note": "ran through eb-staged-rebuild",'

run 'cat $WORK/site/logs/20261117_090000/zen4/Example-1.0-GCCcore-15.2.0.stdout.log'

run 'STAGED_EXIT=2 eb-pipeline run '"$args"' --run-id 1235; echo "exit $?"'
expect_line 'exit 78'

run 'STAGED_EXIT=75 eb-pipeline run '"$args"' --run-id 1236; echo "exit $?"'
expect_line 'exit 75'

run 'eb-pipeline run --site site.toml --list zen2.list --target zen2 --build-type production --run-id 1237; echo "exit $?"'
expect_line 'exit 0'
