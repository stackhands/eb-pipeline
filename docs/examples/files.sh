#!/bin/bash
# The plan, the result file and the log tree that
# docs/orgmode/reference/plan-and-result.org and log-tree.org show: a site
# with the local scheduler, kept logs and rotation, run with stand-ins for
# eb and eb-keep-logs that print their arguments.
# shellcheck disable=SC2016 # commands stay quoted as the pages show them
# shellcheck source-path=SCRIPTDIR source=lib.sh
. "$(dirname -- "$0")/lib.sh"

export WORK=$HOME/files
mkdir -p "$WORK/bin"
cd "$WORK"
cat >bin/eb <<'EOF'
#!/bin/sh
echo "eb $*"
case "$1" in Broken-*) exit 1 ;; esac
EOF
cat >bin/eb-keep-logs <<'EOF'
#!/bin/sh
if [ "$1" = --help ]; then
    echo "usage: eb-keep-logs rotate LOG_ROOT --compress-after DAYS [--delete-after DAYS] [--lock-timeout SECONDS]"
    exit 0
fi
echo "eb-keep-logs $*"
EOF
chmod +x bin/eb bin/eb-keep-logs
export PATH=$WORK/bin:$PATH
cat >site.toml <<EOF
schema_version = 1
kind = "site"

[site]
name = "example"

[scheduler]
kind = "local"

[[targets]]
name = "cpu"

[filesystems]
scratch = "$WORK/scratch"
tmp = "$WORK/scratch/tmp"
build_path = "$WORK/scratch/build"
install_root = "$WORK/software"
log_root = "$WORK/logs"

[modules]
init = []

[logs]
keep_failed = true

[logs.rotate]
compress_after_days = 30
delete_after_days = 365
EOF
printf '%s\n' Example-1.0-GCCcore-15.2.0.eb Broken-1.0-GCCcore-15.2.0.eb >build.list

args='--site site.toml --list build.list --target cpu --build-type production --run-id nightly --log-stamp 20261117_090000'

run "eb-pipeline plan $args --out plan.json"
expect_status 0

run 'cat plan.json'
expect_line '  "kind": "plan",'

run "eb-pipeline run $args --result result.json"
expect_status 1
expect_line 'ERROR [build] cpu: Broken-1.0-GCCcore-15.2.0: failed (exit 1)'

run 'cat result.json'
expect_line '  "exit_code": 1,'

run 'find logs | sort'
expect_line 'logs/.eb-pipeline-rotate.lock'
expect_line 'logs/20261117_090000/cpu/Broken-1.0-GCCcore-15.2.0.keep-logs.log'

run 'cat logs/20261117_090000/cpu/Broken-1.0-GCCcore-15.2.0.keep-logs.log logs/20261117_090000/cpu/rotate.log'
expect_line 'eb-keep-logs rotate $WORK/logs --compress-after 30 --delete-after 365 --lock-timeout 0'
