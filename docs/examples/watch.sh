#!/bin/bash
# What docs/orgmode/reference/watch.org shows: a copy of build-from-ci/site.toml
# with the local scheduler, its paths under $WORK, a [watch] table, and a
# module environment that loads EESSI 2023.06 for x86_64/amd/zen4, which runs
# hiredis and tmux through two runs, then Zip through one whose module
# environment loads no EESSI.
#
# $WORK/cvmfs/software.eessi.io stands in for the EESSI repository, with the
# CVMFS revision 32663 in its user.revision attribute, three modules in its
# module tree, each with the easyconfig and the log that EasyBuild leaves in
# its install, and an Lmod init whose module command knows EESSI/2023.06 only.
# The stand-in for eb installs what it builds as EasyBuild does: the easyconfig
# and a new log in the easybuild directory of the install, and a module file;
# an install already there stays as it is unless the line has --rebuild. The
# stand-in for eb-stack 0.7.4 answers rebuild-set from deps.tsv, one
# "dependent changed class edge_kind" row per line.
# shellcheck disable=SC2016 # commands stay quoted as the pages show them
# shellcheck source-path=SCRIPTDIR source=lib.sh
. "$(dirname -- "$0")/lib.sh"

export WORK=$HOME/watch
mkdir -p "$WORK"
cd "$WORK"

repo=$WORK/cvmfs/software.eessi.io
eessi=$repo/versions/2023.06/software/linux/x86_64/amd/zen4

# Install NAME/VERSION in the EESSI tree, or rebuild it in place, which leaves
# another log in the easybuild directory of its install.
eessi_module() {
    local root=$eessi/software/$1/$2 logs
    mkdir -p "$root/easybuild" "$eessi/modules/all/$1"
    echo "name = '$1'" >"$root/easybuild/$1-$2.eb"
    logs=$(find "$root/easybuild" -name '*.log' | wc -l)
    : >"$root/easybuild/easybuild-$1-$2-$((logs + 1)).log"
    printf 'help([==[%s]==])\nlocal root = "%s"\n' "$1" "$root" >"$eessi/modules/all/$1/$2.lua"
}
eessi_module CMake 3.27.6-GCCcore-13.2.0
eessi_module OpenSSL 1.1
eessi_module libevent 2.1.12-GCCcore-13.2.0
python3 -c 'import os, sys; os.setxattr(sys.argv[1], "user.revision", sys.argv[2].encode())' "$repo" 32663

mkdir -p "$repo/versions/2023.06/init/lmod"
cat >"$repo/versions/2023.06/init/lmod/bash" <<EOF
module() {
    case "\$1 \${2:-}" in
        "purge ") ;;
        "load EESSI/2023.06")
            export EESSI_CVMFS_REPO=$repo EESSI_VERSION=2023.06 EESSI_SOFTWARE_SUBDIR=x86_64/amd/zen4
            export EESSI_MODULEPATH=$eessi/modules/all ;;
        *) echo "module \$*: not in the stand-in" >&2; return 1 ;;
    esac
}
EOF

mkdir -p bin
cat >bin/eb <<'EOF'
#!/bin/sh
echo "eb $*"
stem=${1##*/}
stem=${stem%.eb}
name=${stem%%-*}
version=${stem#*-}
root=$EASYBUILD_INSTALLPATH/software/$name/$version
module=$EASYBUILD_INSTALLPATH/modules/all/$name/$version.lua
case " $* " in
    *" --rebuild "*) ;;
    *) if [ -f "$module" ]; then echo "== $stem is already installed"; exit 0; fi ;;
esac
mkdir -p "$root/easybuild" "${module%/*}"
echo "name = '$name'" >"$root/easybuild/$stem.eb"
logs=$(find "$root/easybuild" -name '*.log' | wc -l)
: >"$root/easybuild/easybuild-$name-$version-$((logs + 1)).log"
printf 'help([==[%s]==])\nlocal root = "%s"\n' "$name" "$root" >"$module"
EOF
cat >bin/eb-stack <<'EOF'
#!/bin/sh
if [ "$1" = --version ]; then
    echo "eb-stack 0.7.4"
    exit 0
fi
cmd=$1
shift
while [ $# -gt 0 ]; do
    case $1 in
        --changed) changed=$2; shift ;;
        --out-dir) out=$2; shift ;;
    esac
    shift
done
mkdir -p "$out"
case $cmd in
    rebuild-set)
        awk -F'\t' 'NR == FNR { c[$1] = 1; next }
            ($2 in c) { printf "%s{\"changed\":\"%s\",\"dependent\":\"%s\",\"class\":\"%s\",\"edge_kind\":\"%s\",\"reason\":\"stand-in\"}", (n++ ? "," : ""), $2, $1, $3, $4 }
            BEGIN { printf "{\"schema_version\":5,\"rows\":[" } END { print "]}" }' "$changed" "$WORK/deps.tsv" >"$out/rebuild-set.json" ;;
esac
EOF
chmod +x bin/eb bin/eb-stack
printf '%s\t%s\t%s\t%s\n' \
    hiredis/1.2.0-GCCcore-13.2.0 CMake-3.27.6-GCCcore-13.2.0.eb review builddependency \
    hiredis/1.2.0-GCCcore-13.2.0 OpenSSL-1.1.eb no-rebuild runtime \
    tmux/3.4-GCCcore-13.2.0 libevent-2.1.12-GCCcore-13.2.0.eb no-rebuild runtime >deps.tsv
export PATH=$WORK/bin:$PATH

sed -e 's/^kind = "slurm"/kind = "local"/' \
    -e "s#/opt/site#$WORK/site#g" \
    -e "s#/scratch/alice#$WORK/scratch#g" \
    -e "s#^init = \[\"EasyBuild\"\]#shell_init = \"$repo/versions/2023.06/init/lmod/bash\"\ninit = [\"EESSI/2023.06\"]#" \
    "$pages/howto/build-from-ci/site.toml" >site.toml
printf '\n[watch]\n' >>site.toml
printf '%s\n' hiredis-1.2.0-GCCcore-13.2.0.eb tmux-3.4-GCCcore-13.2.0.eb >zen4.list

run 'sed -n "/^\[modules\]/,\$p" site.toml'
expect_line '[watch]'

run 'eb-pipeline run --site site.toml --list zen4.list --target zen4 --build-type production --log-stamp 20261117_090000'
expect_status 0
expect_line 'INFO [watch] zen4: recorded 2 against EESSI 2023.06 x86_64/amd/zen4 at revision 32663: hiredis/1.2.0-GCCcore-13.2.0, tmux/3.4-GCCcore-13.2.0'

run 'cat site/zen4/.eb-pipeline-watch/zen4.json'
expect_line '  "kind": "watch-state",'
expect_line '          "class": "review",'

run 'eb-pipeline run --site site.toml --list zen4.list --target zen4 --build-type production --log-stamp 20261117_100000'
expect_status 0
expect_line 'INFO [watch] zen4: 2 line(s) left their install as it was and are not recorded again'

sed -e '/^shell_init = /d' -e 's#^init = \["EESSI/2023.06"\]#init = []#' site.toml >no-eessi.toml
echo Zip-3.0-GCCcore-13.2.0.eb >zip.list
run 'eb-pipeline run --site no-eessi.toml --list zip.list --target zen4 --build-type production --log-stamp 20261117_110000'
expect_status 0
expect_line 'WARN [watch] zen4: Zip/3.0-GCCcore-13.2.0 not recorded: no EESSI in the module environment of target zen4: EESSI_VERSION is unset; no EESSI change under them is seen until a record of them is written'

run 'find scratch/eb-pipeline-watch -type f'
expect_line 'scratch/eb-pipeline-watch/zen4/20261117_110000/record/eessi-probe.log'
