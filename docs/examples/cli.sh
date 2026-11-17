#!/bin/bash
# The help texts that docs/orgmode/reference/cli.org shows, and the usage
# line of every command, which its synopsis copies.
# shellcheck source-path=SCRIPTDIR source=lib.sh
. "$(dirname -- "$0")/lib.sh"

run 'eb-pipeline --help'
expect_status 0
expect_line 'Usage: eb-pipeline <COMMAND>'

run 'eb-pipeline --version'
expect_status 0
expect_line "eb-pipeline $(crate_version)"

synopsis=(
    'check [OPTIONS] --site <SITE>'
    'schema'
    'init [OPTIONS]'
    'explain [OPTIONS] --site <SITE>'
    'targets [OPTIONS] --site <SITE>'
    'status [OPTIONS] --site <SITE>'
    'metrics [OPTIONS] --result <RESULT>'
    'plan [OPTIONS] --site <SITE> --list <LIST> --target <TARGET> --build-type <BUILD_TYPE>'
    'run [OPTIONS] --site <SITE> --list <LIST> --target <TARGET> --build-type <BUILD_TYPE>'
    'watch [OPTIONS] --site <SITE> --target <TARGET> --build-type <BUILD_TYPE>'
)
for usage in "${synopsis[@]}"; do
    run "eb-pipeline ${usage%% *} --help"
    expect_status 0
    expect_line "Usage: eb-pipeline $usage"
done
