#!/bin/bash
# What docs/orgmode/howto/publish-a-junit-report.org checks: a result file
# with one successful line, the JUnit case eb-junit prints for it, and a
# plan file, which is not a result.
# shellcheck shell=bash
# shellcheck disable=SC2016 # commands stay quoted as the pages show them
# shellcheck source-path=SCRIPTDIR source=lib.sh
. "$(dirname -- "$0")/lib.sh"

junit=$(dirname -- "$bin")/eb-junit
if [ ! -x "$junit" ]; then
    echo "no eb-junit at $junit; run cargo build --locked first" >&2
    exit 2
fi
ln -s "$junit" "$HOME/.cargo/bin/eb-junit"

export WORK=$HOME/junit
mkdir -p "$WORK"
cd "$WORK"

cat >result-cpu.json <<'EOF'
{
  "schema_version": 1,
  "kind": "result",
  "site": "example",
  "target": "cpu",
  "build_type": "production",
  "run_id": "nightly",
  "exit_code": 0,
  "lines": [
    {
      "easyconfig": "Ok-1.0.eb",
      "unit": "all",
      "phase": "build",
      "status": "success",
      "log": "logs/ok.log",
      "duration_seconds": 0.5
    }
  ]
}
EOF

run 'eb-junit --result result-cpu.json'
expect_status 0
expect_line '    <testcase name="cpu/Ok-1.0.eb" classname="example/cpu" file="Ok-1.0.eb" time="0.500"/>'
if grep -q 'logs/ok.log' <<<"$output"; then
    echo "the success case repeated its log path" >&2
    exit 1
fi

printf '%s\n' '{"kind":"plan","schema_version":1}' >plan.json
run 'eb-junit --result plan.json'
expect_status 2
expect_line 'eb-junit: plan.json: expected kind `result`, found `plan`'
