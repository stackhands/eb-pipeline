#!/bin/bash
# Build one list on every target of site.toml, one target after the other.
# Usage: build-all.sh LIST BUILD_TYPE RUN_ID
# Exits as eb-pipeline does: 1 when a target failed, else 78 when one was
# refused, else 75 when one is pending, else 0; a usage error or a signal
# stops the loop with its own status.
set -u
export EB_PIPELINE_SITE=site.toml EB_PIPELINE_LIST=$1 EB_PIPELINE_BUILD_TYPE=$2
run_id=$3
names=$(eb-pipeline targets) || exit
worst=0
for t in $names; do
    EB_PIPELINE_TARGET=$t EB_PIPELINE_RUN_ID=$run_id-$t \
        eb-pipeline run --result "result-$t.json"
    rc=$?
    echo "$t: exit $rc"
    case $rc in
        0) ;;
        1) worst=1 ;;
        78) [ "$worst" -eq 1 ] || worst=78 ;;
        75) [ "$worst" -ne 0 ] || worst=75 ;;
        *) exit "$rc" ;;
    esac
done
exit "$worst"
