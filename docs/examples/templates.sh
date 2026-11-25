#!/bin/bash
# The templates that docs/orgmode/reference/templates.org lists: their names,
# a site file written from each, and what `check` says about it.
# shellcheck source-path=SCRIPTDIR source=lib.sh
. "$(dirname -- "$0")/lib.sh"

run 'eb-pipeline init --list'
expect_status 0
expect_line 'workstation      One machine, no scheduler: every phase runs on this host; no EESSI, quota or licences'
expect_line 'small-cluster    Slurm, one CPU target; no EESSI, inode quota or licences'

for name in workstation small-cluster eessi-site; do
    run "eb-pipeline init --template $name --out $name.toml"
    expect_status 0
    expect_line "INFO [init] -: wrote template $name to $name.toml"
    if ! cmp -s "$name.toml" "$root/templates/$name.toml"; then
        echo "$name.toml differs from templates/$name.toml" >&2
        exit 1
    fi

    run "eb-pipeline check --site $name.toml"
    expect_status 0
done

run 'eb-pipeline init --template small-cluster --out small-cluster.toml'
expect_status 2
expect_line 'ERROR [main] -: cannot create small-cluster.toml: File exists (os error 17)'

run 'eb-pipeline targets --site eessi-site.toml'
expect_status 0
