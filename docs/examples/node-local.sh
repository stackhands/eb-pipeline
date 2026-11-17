#!/bin/bash
# What docs/orgmode/howto/node-local-builds.org shows: node-local-builds/
# site.toml checked and explained, then run on a copy of it with its paths
# under $WORK, no modules, a stand-in for srun that runs each command here as
# job 4242 of alice, and the stand-in eb of the tutorial, which prints its
# EASYBUILD_ variables.
# shellcheck disable=SC2016 # commands stay quoted as the pages show them
# shellcheck source-path=SCRIPTDIR source=lib.sh
. "$(dirname -- "$0")/lib.sh"

export WORK=$HOME/node-local
mkdir -p "$WORK"
cd "$WORK"
cp "$pages/howto/node-local-builds/site.toml" site.toml

run 'eb-pipeline check --site site.toml
eb-pipeline explain --site site.toml | grep -E "^(EASYBUILD_BUILDPATH|EASYBUILD_TMPDIR|EB_PIPELINE_JOB_DIR) ="'
expect_status 0
expect_line 'EASYBUILD_BUILDPATH = "{job_dir}/build"'
expect_line 'EB_PIPELINE_JOB_DIR = "/local/{user}.{job}"'

sed 's#^job_dir = .*#job_dir = "/local/{user}"#' site.toml >shared.toml
run 'eb-pipeline check --site shared.toml'
expect_status 2
expect_line "ERROR [check] example: filesystems.job_dir: must hold {job}, which makes the directory the job's own"

mkdir -p run/bin node/alice.4242
cat >run/bin/srun <<'EOF'
#!/bin/sh
while [ $# -gt 0 ]; do
  case $1 in
    --*) shift ;;
    -?) shift 2 ;;
    *) break ;;
  esac
done
SLURM_JOB_ID=4242 SLURM_JOB_USER=alice exec "$@"
EOF
chmod +x run/bin/srun
sed -e "s#^launch = \"srun\"#launch = \"$WORK/run/bin/srun\"#" \
    -e "s#\"/local/#\"$WORK/node/#" \
    -e "s#/opt/site#$WORK/site#g" \
    -e "s#/scratch/alice#$WORK/scratch#g" \
    -e 's/^init = \["EasyBuild"\]/init = []/' site.toml >run/site.toml
cd run
# shellcheck source=../orgmode/tutorials/first-build-list/eb.sh
. "$pages/tutorials/first-build-list/eb.sh"
export PATH=$WORK/run/bin:$PATH
printf '%s\n' Example-1.0-GCCcore-15.2.0.eb >build.list

run 'eb-pipeline run --site site.toml --list build.list --target zen4 --build-type production --log-stamp 20261117_090000'
expect_status 0
expect_line 'INFO [build] zen4: Example-1.0-GCCcore-15.2.0: ok'

run 'grep -E "^EASYBUILD_(BUILDPATH|TMPDIR)=" $WORK/site/logs/20261117_090000/zen4/Example-1.0-GCCcore-15.2.0.stdout.log'
expect_line 'EASYBUILD_BUILDPATH=$WORK/node/alice.4242/build'
expect_line 'EASYBUILD_TMPDIR=$WORK/scratch/tmp'

rmdir "$WORK/node/alice.4242"
run 'eb-pipeline run --site site.toml --list build.list --target zen4 --build-type production --log-stamp 20261117_100000'
expect_status 1
expect_line 'ERROR [build] zen4: Example-1.0-GCCcore-15.2.0: failed (exit 1)'

run 'cat $WORK/site/logs/20261117_100000/zen4/Example-1.0-GCCcore-15.2.0.stdout.log'
expect_line 'eb-pipeline: job 4242 has no directory $WORK/node/alice.4242 that it can write to, which filesystems.job_dir names'
