#!/bin/bash
# What docs/orgmode/howto/attest-build-inputs.org shows: a site with
# [attest] enabled, a dry run that leaves the log untouched, a plan that
# appends a record, a verify that accepts it, and a verify that refuses
# after the patch changes.
# shellcheck disable=SC2016 # commands stay quoted as the pages show them
page=$(dirname -- "$0")/../orgmode/howto/attest-build-inputs.org
for line in \
    'eb-pipeline plan --site site.toml --list lists/build.list --target cpu --build-type production --run-id nightly --log-stamp 20261117_090000 --out plan.json' \
    'eb-pipeline verify --site site.toml --list lists/build.list --target cpu --build-type production --from plan.json' \
    'exit 78' \
    'verified input record'
do
    grep -F -- "$line" "$page" >/dev/null || {
        echo "attest-build-inputs.org is missing: $line" >&2
        exit 1
    }
done
# shellcheck source-path=SCRIPTDIR source=lib.sh
. "$(dirname -- "$0")/lib.sh"

export WORK=$HOME/attest
mkdir -p "$WORK/bin" "$WORK/easyconfigs" "$WORK/easyblocks" "$WORK/patches" "$WORK/lists" "$WORK/keys"
cd "$WORK" || exit

cat >bin/eb-stack <<'EOF'
#!/bin/sh
case "$1" in
  --version) echo "eb-stack 0.7.4"; exit 0 ;;
esac
[ "$1" = stack ] && [ "$2" = order ] || exit 0
out=""
while [ $# -gt 0 ]; do
  case "$1" in
    --out) out=$2; shift 2 ;;
    *) shift ;;
  esac
done
printf '%s\n' "$STUB_EASYCONFIG" >"$out"
EOF
cat >bin/rfc9162-sidecar <<'EOF'
#!/bin/sh
cmd=$1
shift
dir="" path="" proof="" blob=""
while [ $# -gt 0 ]; do
  case "$1" in
    --dir) dir=$2; shift 2 ;;
    --path) path=$2; shift 2 ;;
    --proof) proof=$2; shift 2 ;;
    --blob-file) blob=$2; shift 2 ;;
    --key|--origin|--trusted) shift 2 ;;
    *) shift ;;
  esac
done
case "$cmd" in
  init|sync|snapshot) exit 0 ;;
  prove)
    hash=$(sha256sum "$dir/$path" | awk '{print $1}')
    entry=$(printf '{"blob":"%s","op":"put","path":"%s"}' "$hash" "$path")
    root=$(printf '\000%s' "$entry" | sha256sum | awk '{print $1}')
    printf '{"blob":"%s","checkpoint":"note","entry":"%s","inclusion":[],"index":0,"op":"put","origin":"example.org/inputs","path":"%s","root":"%s","size":1}\n' "$hash" "$(printf '%s' "$entry" | sed 's/"/\\"/g')" "$path" "$root"
    ;;
  verify-proof)
    want=$(sed -n 's/.*"blob": *"\([0-9a-f]*\)".*/\1/p' "$proof" | head -1)
    got=$(sha256sum "$blob" | awk '{print $1}')
    [ "$want" = "$got" ] || { echo "blob mismatch" >&2; exit 1; }
    echo "verified proof $path index 0 size 1 root $want by 1 trusted key(s)"
    ;;
  *) exit 2 ;;
esac
EOF
cat >bin/eb <<'EOF'
#!/bin/sh
exit 0
EOF
chmod +x bin/eb-stack bin/rfc9162-sidecar bin/eb
export PATH=$WORK/bin:$PATH
export STUB_EASYCONFIG=$WORK/easyconfigs/Foo-1.0.eb
export EB_IMAGE_DIGEST=sha256:0123456789abcdef
export EB_ROUTE_RECEIPT=receipt-1

printf '%s\n' "easyblock = 'EB_Foo'" "patches = ['foo.patch']" >easyconfigs/Foo-1.0.eb
printf '%s\n' 'class EB_Foo(EasyBlock):' '    pass' >easyblocks/eb_foo.py
printf '%s\n' 'diff' >patches/foo.patch
printf '%s\n' 'not-a-real-key' >keys/attest.pem
printf '%s\n' 'Foo-1.0.eb' >lists/build.list
cat >site.toml <<EOF
schema_version = 1
kind = "site"
[site]
name = "t"
[scheduler]
kind = "local"
[[targets]]
name = "cpu"
[targets.route]
kind = "command"
command = "true"
receipt_digest_env = "EB_ROUTE_RECEIPT"
[filesystems]
scratch = "$WORK/scratch"
tmp = "$WORK/tmp"
build_path = "$WORK/build"
install_root = "$WORK/install"
log_root = "$WORK/logs"
[modules]
init = []
[attest]
enabled = true
log_dir = "$WORK/attest-log"
key = "$WORK/keys/attest.pem"
origin = "example.org/inputs"
trusted = ["example.org/inputs+00000000+AQAAAA=="]
bin = "$WORK/bin/rfc9162-sidecar"
easyconfigs = ["$WORK/easyconfigs"]
easyblocks = ["$WORK/easyblocks"]
patches = ["$WORK/patches"]
image_digest_env = "EB_IMAGE_DIGEST"
EOF

run 'eb-pipeline check --site site.toml'
expect_status 0
expect_line 'INFO [check] t: site file is valid'

run 'eb-pipeline run --site site.toml --list lists/build.list --target cpu --build-type production --run-id nightly --log-stamp 20261117_090000 --dry-run'
expect_status 0
expect_line 'INFO [run] cpu: dry run: nothing executed'
[ ! -e "$WORK/attest-log" ]

run 'eb-pipeline plan --site site.toml --list lists/build.list --target cpu --build-type production --run-id nightly --log-stamp 20261117_090000 --out plan.json'
expect_status 0
grep -q '"record_sha256"' plan.json
grep -q 'eb-pipeline-input-attestation-v1' "$WORK/attest-log"/inputs/*.json

run 'eb-pipeline verify --site site.toml --list lists/build.list --target cpu --build-type production --from plan.json'
expect_status 0
grep -q '^verified input record ' <<<"$output"

printf '%s\n' 'changed' >>patches/foo.patch
run 'eb-pipeline verify --site site.toml --list lists/build.list --target cpu --build-type production --from plan.json'
expect_status 78
