#!/bin/bash
# `init --hints` prints next steps on stderr after writing a site file.
set -euo pipefail

# shellcheck source-path=SCRIPTDIR source=lib.sh
. "$(dirname -- "$0")/lib.sh"

run 'eb-pipeline init --template eessi-site --out site.toml --hints'
expect_status 0
expect_line 'INFO [init] hint: list target names with `eb-pipeline targets --site site.toml` (needs a build from the main branch; release 0.3.0 has no targets command)'
expect_line 'INFO [init] hint: check the site file with `eb-pipeline check --site site.toml`'

run 'EB_PIPELINE_INIT_HINTS=1 eb-pipeline init --template small-cluster --out other.toml'
expect_status 0
expect_line 'INFO [init] hint: list target names with `eb-pipeline targets --site other.toml` (needs a build from the main branch; release 0.3.0 has no targets command)'