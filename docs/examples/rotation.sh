#!/bin/bash
# The rotation of the log tree that docs/orgmode/howto/rotate-logs.org and
# docs/orgmode/explanation/log-rotation.org show: a run that rotates, runs
# that find another run live or rotating, and runs that find eb-keep-logs
# busy, old or failing; then the rotation outside a run, with flock(1).
#
# The stand-in for eb-keep-logs prints its arguments and exits with
# KEEP_LOGS_EXIT, 0 unless set. With KEEP_LOGS_OLD set, its --help leaves
# out --lock-timeout, as the help of eb-keep-logs 0.2.1 does.
# shellcheck disable=SC2016 # commands stay quoted as the pages show them
# shellcheck source-path=SCRIPTDIR source=lib.sh
. "$(dirname -- "$0")/lib.sh"

export WORK=$HOME/rotation
mkdir -p "$WORK/bin"
cd "$WORK"
cat >bin/eb <<'EOF'
#!/bin/sh
echo "eb $*"
EOF
cat >bin/eb-keep-logs <<'EOF'
#!/bin/sh
if [ "$1" = --help ]; then
    if [ -n "$KEEP_LOGS_OLD" ]; then
        echo "usage: eb-keep-logs rotate LOG_ROOT --compress-after DAYS [--delete-after DAYS]"
    else
        echo "usage: eb-keep-logs rotate LOG_ROOT --compress-after DAYS [--delete-after DAYS] [--lock-timeout SECONDS]"
    fi
    exit 0
fi
echo "eb-keep-logs $*"
exit "${KEEP_LOGS_EXIT:-0}"
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

[logs.rotate]
compress_after_days = 30
delete_after_days = 365
EOF
printf '%s\n' Example-1.0-GCCcore-15.2.0.eb >build.list
lock=$WORK/logs/.eb-pipeline-rotate.lock
args='--site site.toml --list build.list --target cpu --build-type production'

# The rotation a run would start.
run "eb-pipeline run $args --dry-run | grep '^\[logs\]'"
expect_status 0
expect_line '[logs] TZ=UTC eb-keep-logs rotate $WORK/logs --compress-after 30 --delete-after 365 --lock-timeout 0'

# A run that ends alone rotates.
run "eb-pipeline run $args --log-stamp 20261117_090000 --result result.json"
expect_status 0
run "grep -B1 -A1 '\"name\": \"logs\"' result.json"
expect_line '      "status": "ok"'
run 'cat logs/20261117_090000/cpu/rotate.log'
expect_line 'eb-keep-logs rotate $WORK/logs --compress-after 30 --delete-after 365 --lock-timeout 0'

# flock(1) holds the lock shared, as another live run does.
run "flock -s logs/.eb-pipeline-rotate.lock eb-pipeline run $args --log-stamp 20261117_100000 --result result.json"
expect_status 0
expect_line 'INFO [logs] cpu: rotation skipped: another run under $WORK/logs holds $WORK/logs/.eb-pipeline-rotate.lock, live or rotating; the rotation is left to a run that ends alone'
run "grep -B1 -A2 '\"name\": \"logs\"' result.json"
expect_line '      "status": "skipped"'

# flock(1) holds the lock alone for two seconds, as a run that rotates does.
flock -x "$lock" sleep 2 &
holder=$!
while flock -n -s "$lock" true; do
    sleep 0.1
done
run "eb-pipeline run $args --log-stamp 20261117_110000"
wait "$holder"
expect_status 0
expect_line 'INFO [logs] cpu: waiting: another run rotates and holds $WORK/logs/.eb-pipeline-rotate.lock'

# Another rotate holds the lock of eb-keep-logs.
run "KEEP_LOGS_EXIT=75 eb-pipeline run $args --log-stamp 20261117_120000"
expect_status 0
expect_line 'INFO [logs] cpu: rotation skipped: eb-keep-logs rotate exited 75: another rotate holds $WORK/logs/.eb-keep-logs-rotate.lock, and this one changed nothing; the rotation is left to it'

# An eb-keep-logs whose help does not list --lock-timeout.
run "KEEP_LOGS_OLD=1 eb-pipeline run $args --log-stamp 20261117_130000"
expect_status 0
expect_line 'INFO [logs] cpu: no --lock-timeout for eb-keep-logs rotate: `eb-keep-logs --help` does not list it, as in eb-keep-logs 0.2.1, which takes no lock of its own'
run 'cat logs/20261117_130000/cpu/rotate.log'
expect_line 'eb-keep-logs rotate $WORK/logs --compress-after 30 --delete-after 365'

# A rotate that fails.
run "KEEP_LOGS_EXIT=1 eb-pipeline run $args --log-stamp 20261117_140000; echo \"exit \$?\""
expect_line 'ERROR [logs] cpu: eb-keep-logs rotate exited 1'
expect_line 'exit 1'

# The rotation outside a run, as cron starts it.
run 'TZ=UTC flock -w 3600 -E 75 logs/.eb-pipeline-rotate.lock eb-keep-logs rotate logs --compress-after 30 --delete-after 365; echo "exit $?"'
expect_line 'exit 0'

# The same while a run is live: flock(1) gives up after its wait.
flock -s "$lock" sleep 3 &
holder=$!
while flock -n -x "$lock" true; do
    sleep 0.1
done
run 'TZ=UTC flock -w 1 -E 75 logs/.eb-pipeline-rotate.lock eb-keep-logs rotate logs --compress-after 30 --delete-after 365; echo "exit $?"'
wait "$holder"
expect_line 'exit 75'

# A rotation lock that cannot be taken: here its path is a directory.
mkdir -p other/.eb-pipeline-rotate.lock
run 'EB_PIPELINE__FILESYSTEMS__LOG_ROOT=$WORK/other eb-pipeline run '"$args"' --log-stamp 20261117_150000'
expect_status 0
expect_line 'WARN [logs] cpu: cannot lock $WORK/other/.eb-pipeline-rotate.lock: Is a directory (os error 21); the rotation does not wait for other runs'
