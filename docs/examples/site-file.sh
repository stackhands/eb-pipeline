#!/bin/bash
# The site file that docs/orgmode/howto/write-a-site-file.org writes: the
# template it starts from, the checks of write-a-site-file/site.toml, its
# resolved configuration and dry run, an override from the environment, and
# the messages of its table of failures. The stand-in for eb-keep-logs
# prints a version, as `check --probe-tools` asks it to.
# shellcheck disable=SC2016 # commands stay quoted as the pages show them
# shellcheck source-path=SCRIPTDIR source=lib.sh
. "$(dirname -- "$0")/lib.sh"

export WORK=$HOME/site
mkdir -p "$WORK/bin" "$WORK/lists"
cd "$WORK"
cat >bin/eb-keep-logs <<'EOF'
#!/bin/sh
echo "eb-keep-logs 0.2.1"
EOF
chmod +x bin/eb-keep-logs
export PATH=$WORK/bin:$PATH

run 'eb-pipeline init --template small-cluster --out site.toml'
expect_status 0

# The edits of step 2.
cp "$pages/howto/write-a-site-file/site.toml" site.toml
printf '%s\n' Example-1.0-GCCcore-15.2.0.eb >lists/zen4.list

run 'eb-pipeline check --site site.toml
eb-pipeline check --site site.toml --lists-dir lists
eb-pipeline check --site site.toml --probe-tools'
expect_status 0
expect_line 'INFO [check] example: site file is valid'

run 'eb-pipeline explain --site site.toml --target zen4 --build-type production'
expect_status 0
expect_line 'EASYBUILD_INSTALLPATH = "/opt/site"'

run 'eb-pipeline run --site site.toml --list lists/zen4.list --target zen4 --build-type production --dry-run'
expect_status 0
expect_line 'INFO [run] zen4: dry run: nothing executed'

run 'EB_PIPELINE__FILESYSTEMS__BUILD_PATH=/local/alice/build eb-pipeline explain --site site.toml --target zen4 | grep BUILDPATH'
expect_status 0
expect_line 'INFO [site] -: filesystems.build_path set from EB_PIPELINE__FILESYSTEMS__BUILD_PATH'
expect_line 'EASYBUILD_BUILDPATH = "/local/alice/build"'

# The failures.
sed 's/^log_root/log_rot/' site.toml >typo.toml
run 'eb-pipeline check --site typo.toml'
expect_status 2
expect_line 'unknown field `log_rot`, expected one of `scratch`, `tmp`, `build_path`, `job_dir`, `source_paths`, `install_root`, `export_install_vars`, `log_root`, `shard_dir`, `read_only_installdir`, `read_only_installdir_value`, `configs_repo`'

sed 's/^kind = "slurm"/kind = "pbs"/' site.toml >pbs.toml
run 'eb-pipeline check --site pbs.toml'
expect_status 2
expect_line 'ERROR [check] example: scheduler.kind: expected slurm or local, found `pbs`'

sed 's/^init = \["EasyBuild"\]/init = ["module load EasyBuild"]/' site.toml >init.toml
run 'eb-pipeline check --site init.toml'
expect_status 2
expect_line 'ERROR [check] example: modules.init: `module load EasyBuild` has white space; an entry is one module name, so give each module its own entry'

sed '/^init = /a shell = "zsh"' site.toml >zsh.toml
run 'eb-pipeline check --site zsh.toml'
expect_status 2
expect_line 'ERROR [check] example: modules.shell: expected a POSIX shell that runs the module wrapper, one of sh, bash, dash, ash, ksh, ksh93, mksh or yash, by name or by path, found `zsh`'

sed 's/^compress_after_days = 30/compress_after_days = 400/' site.toml >rotate.toml
run 'eb-pipeline check --site rotate.toml'
expect_status 2
expect_line 'ERROR [check] example: logs.rotate.delete_after_days: must be greater than compress_after_days (400), found 365'

run 'eb-pipeline check --site site.toml --lists-dir .'
expect_status 2
expect_line 'ERROR [check] example: targets.zen4.build_lists: `zen4.list` not found in .'

rm bin/eb-keep-logs
run 'eb-pipeline check --site site.toml --probe-tools'
expect_status 2
expect_line 'ERROR [main] -: eb_keep_logs: cannot run `eb-keep-logs --version`: No such file or directory (os error 2)'
