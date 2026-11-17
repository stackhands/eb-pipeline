#!/bin/bash
# What docs/orgmode/howto/site-installs.org shows: the prefix and CPU target
# that each target of site-installs/site.toml exports, and the tree that
# EESSI-extend installs its builds into. The other steps of the page need a
# cluster with EESSI.
# shellcheck disable=SC2016 # commands stay quoted as the pages show them
# shellcheck source-path=SCRIPTDIR source=lib.sh
. "$(dirname -- "$0")/lib.sh"

export WORK=$HOME/site-installs
mkdir -p "$WORK"
cd "$WORK"
cp "$pages/howto/site-installs/site.toml" site.toml

run 'eb-pipeline check --site site.toml'
expect_status 0

run 'for t in $(eb-pipeline targets --site site.toml); do
    eb-pipeline explain --site site.toml --target "$t" | grep -E "install_note|EESSI_SITE_SOFTWARE_PREFIX|EESSI_SOFTWARE_SUBDIR_OVERRIDE"
done'
expect_status 0
expect_line 'EESSI_SITE_SOFTWARE_PREFIX = "/opt/site/zen2"'
expect_line 'EESSI_SOFTWARE_SUBDIR_OVERRIDE = "x86_64/amd/zen2"'
expect_line 'install_note = "EESSI-extend replaces EASYBUILD_INSTALLPATH when it loads: /opt/site/zen4/versions/<release>/software/linux/x86_64/amd/zen4"'
expect_line 'EESSI_SITE_SOFTWARE_PREFIX = "/opt/site/zen4"'
expect_line 'EESSI_SOFTWARE_SUBDIR_OVERRIDE = "x86_64/amd/zen4"'
