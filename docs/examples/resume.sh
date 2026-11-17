#!/bin/bash
# What docs/orgmode/howto/resume-a-run.org shows: a site with [resume]
# enabled, a dry run that prints `eb-stack stack order --hashes-out`, a run
# that records the digest and the module file, a second run that skips that
# unit, and a third whose hash file differs and is refused.
# shellcheck disable=SC2016 # commands stay quoted as the pages show them
page=$(dirname -- "$0")/../orgmode/howto/resume-a-run.org
for line in \
    '[shard] $WORK/bin/eb-stack stack order --out $WORK/logs/20261117_090000/cpu/input-order.txt --hashes-out $WORK/logs/20261117_090000/cpu/input-hashes.txt --easyconfigs $WORK/easyconfigs --root Foo==1.0' \
    'INFO [build] cpu: Foo-1.0-foss-2025a: resumed; outputs still present' \
    'exit 78'
do
    grep -F -- "$line" "$page" >/dev/null || {
        echo "resume-a-run.org is missing: $line" >&2
        exit 1
    }
done
# shellcheck source-path=SCRIPTDIR source=lib.sh
. "$(dirname -- "$0")/lib.sh"

export WORK=$HOME/resume
mkdir -p "$WORK/bin" "$WORK/easyconfigs" "$WORK/lists"
cd "$WORK" || exit

cat >bin/eb-stack <<'EOF'
#!/bin/sh
case "$1" in
  --version) echo "eb-stack 0.7.4"; exit 0 ;;
esac
[ "$1" = stack ] || exit 64
sub=$2
shift 2
case "$sub" in
  order)
    hashes=""
    while [ $# -gt 0 ]; do
      case "$1" in
        --hashes-out) hashes=$2; shift 2 ;;
        *) shift ;;
      esac
    done
    mkdir -p "$(dirname "$hashes")"
    body='aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa  Foo/1.0-foss-2025a'
    if [ -n "${STUB_ORDER_DIFFERENT:-}" ]; then
      body='bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb  Foo/1.0-foss-2025a'
    fi
    printf '%s\n' "$body" >"$hashes"
    echo "hashes=$hashes modules=1 incomplete=0"
    echo "order=$hashes builds=1"
    exit 0 ;;
esac
exit 64
EOF

cat >bin/eb <<'EOF'
#!/bin/sh
case " $* " in
  *" --fetch "*) exit 0 ;;
esac
stem=$(basename "$1" .eb)
name=${stem%%-[0-9]*}
ver=${stem#"$name"-}
dir="${EASYBUILD_INSTALLPATH:?}/modules/all/$name"
mkdir -p "$dir"
printf 'help([[%s]])\n' "$stem" >"$dir/$ver.lua"
exit 0
EOF
chmod +x bin/eb-stack bin/eb
export PATH=$WORK/bin:$PATH

cat >site.toml <<EOF
schema_version = 1
kind = "site"
[site]
name = "example"
[site.tools.eb_stack]
bin = "$WORK/bin/eb-stack"
[scheduler]
kind = "local"
[[targets]]
name = "cpu"
[filesystems]
scratch = "$WORK/scratch"
tmp = "$WORK/tmp"
build_path = "$WORK/build"
install_root = "$WORK/install"
log_root = "$WORK/logs"
[modules]
init = []
[resume]
enabled = true
easyconfigs = ["$WORK/easyconfigs"]
EOF
printf '%s\n' 'Foo-1.0-foss-2025a.eb' >lists/build.list

run 'eb-pipeline check --site site.toml'
expect_status 0
expect_line 'INFO [check] example: site file is valid'

run 'eb-pipeline run --site site.toml --list lists/build.list --target cpu --build-type production --run-id nightly --log-stamp 20261117_090000 --dry-run | grep hashes-out'
expect_status 0
expect_line '[shard] $WORK/bin/eb-stack stack order --out $WORK/logs/20261117_090000/cpu/input-order.txt --hashes-out $WORK/logs/20261117_090000/cpu/input-hashes.txt --easyconfigs $WORK/easyconfigs --root Foo==1.0'

run 'eb-pipeline run --site site.toml --list lists/build.list --target cpu --build-type production --run-id nightly --log-stamp 20261117_090000 --result result.json'
expect_status 0
expect_line 'INFO [shard] cpu: recorded the input identity'
expect_line 'INFO [build] cpu: Foo-1.0-foss-2025a: ok'

run 'grep input_digest result.json'
expect_status 0
expect_line '  "input_digest": "eb79de43485649f152122db34e595fabf467f7ad689bd8743548d71f8163f65d",'

run 'eb-pipeline run --site site.toml --list lists/build.list --target cpu --build-type production --run-id nightly-2 --log-stamp 20261117_100000 --result resumed.json --resume-result result.json'
expect_status 0
expect_line 'INFO [build] cpu: Foo-1.0-foss-2025a: resumed; outputs still present'

export STUB_ORDER_DIFFERENT=1
run 'eb-pipeline run --site site.toml --list lists/build.list --target cpu --build-type production --run-id nightly-3 --log-stamp 20261117_110000 --result refused.json --resume-result result.json; echo "exit $?"'
expect_status 0
expect_line 'ERROR [shard] cpu: input digest mismatch: prior result has eb79de43485649f152122db34e595fabf467f7ad689bd8743548d71f8163f65d, this plan has 17f012a2d6778fb28a20769cb1ef8998d7341f564a5f77e4767b24e0587ca150'
expect_line 'exit 78'
