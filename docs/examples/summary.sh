#!/bin/bash
# What docs/orgmode/reference/run-summary.org and
# docs/orgmode/howto/follow-runs.org show: both targets of a copy of
# build-from-ci/site.toml with the local scheduler, no modules and its paths
# under $WORK, run with one log stamp and the stand-in eb of the tutorial,
# which fails the lines whose easyconfig starts with Broken-; the summary the
# two runs wrote together, a run that cannot write one, and the POST of the
# summary that a [notify] table asks for, with a stand-in for curl that
# writes its arguments and its standard input to curl.log; then the last
# runs, the failed lines of one with jq, and a POST through curl to
# follow-runs/listen.py, which takes one on 127.0.0.1:8765.
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

run 'eb-pipeline status --site site.toml'
expect_status 0
expect_line '20261117_090000  failed (exit 1)  $WORK/site/logs/20261117_090000/summary.html'

run 'eb-pipeline status --site site.toml --target zen4 --format json'
expect_status 0
expect_line '  "kind": "status",'

run 'mkdir -p site/logs/20261117_100000/summary.json
eb-pipeline run --site site.toml --list example.list --target zen4 --build-type production --log-stamp 20261117_100000; echo "exit $?"'
expect_line 'exit 1'
expect_line 'ERROR [main] -: cannot write run summary: $WORK/site/logs/20261117_100000/summary.json: Is a directory (os error 21)'

cp site.toml notify.toml
printf '\n[notify]\nwebhook_env = "SUMMARY_WEBHOOK"\nwhen = "problems"\n' >>notify.toml
cat >bin/curl <<'EOF'
#!/bin/sh
echo "curl $*" >>"$WORK/curl.log"
cat >>"$WORK/curl.log"
if [ -n "${CURL_FAIL:-}" ]; then
    echo "curl: (22) The requested URL returned error: $CURL_FAIL" >&2
    exit 22
fi
EOF
chmod +x bin/curl

run 'sed -n "/^\[notify\]/,\$p" notify.toml'
expect_line 'when = "problems"'

run 'eb-pipeline check --site notify.toml'
expect_status 0
expect_line 'INFO [check] example: site file is valid'

run 'export SUMMARY_WEBHOOK=https://hooks.example.org/build/XXXX
eb-pipeline run --site notify.toml --list build.list --target zen2 --build-type production --log-stamp 20261117_110000; echo "exit $?"
eb-pipeline run --site notify.toml --list example.list --target zen4 --build-type production --log-stamp 20261117_110000; echo "exit $?"'
expect_line 'INFO [notify] zen2: summary.json posted to https://hooks.example.org/...'
expect_line 'INFO [notify] zen4: the run exited 0 and notify.when is problems; nothing sent'

run 'cat curl.log'
expect_line 'url = "https://hooks.example.org/build/XXXX"'

run 'CURL_FAIL=500 eb-pipeline run --site notify.toml --list build.list --target zen2 --build-type production --log-stamp 20261117_120000; echo "exit $?"'
expect_line 'WARN [notify] zen2: POST of summary.json to https://hooks.example.org/... failed: curl exited 22: curl: (22) The requested URL returned error: 500'
expect_line 'exit 1'

run 'eb-pipeline status --site site.toml --last 2'
expect_status 0
expect_line '20261117_120000  failed (exit 1)  $WORK/site/logs/20261117_120000/summary.html'
expect_line '20261117_110000  failed (exit 1)  $WORK/site/logs/20261117_110000/summary.html'

run "jq -r '.targets[] | .name as \$t | .packages[] | select(.status != \"success\") | \"\\(\$t)  \\(.easyconfig)  \\(.status)  \\(.log)\"' site/logs/20261117_120000/summary.json"
expect_status 0
expect_line 'zen2  Broken-1.0-GCCcore-15.2.0.eb  failed  $WORK/site/logs/20261117_120000/zen2/Broken-1.0-GCCcore-15.2.0.stdout.log'

rm bin/curl
cp "$pages/howto/follow-runs/listen.py" .
python3 listen.py >listen.log &
listener=$!
for _ in $(seq 100); do
    if [ -s listen.log ] || ! kill -0 "$listener" 2>/dev/null; then
        break
    fi
    sleep 0.1
done
if ! kill -0 "$listener" 2>/dev/null; then
    echo "listen.py did not start, as when 127.0.0.1:8765 is taken" >&2
    exit 1
fi

run 'SUMMARY_WEBHOOK=http://127.0.0.1:8765/hook/XXXX eb-pipeline run --site notify.toml --list build.list --target zen2 --build-type production --log-stamp 20261117_130000; echo "exit $?"'
expect_line 'INFO [notify] zen2: summary.json posted to http://127.0.0.1:8765/...'
expect_line 'exit 1'
wait "$listener"

run 'cat listen.log'
expect_line 'POST /hook/XXXX application/json: summary of run 20261117_130000, failed'
