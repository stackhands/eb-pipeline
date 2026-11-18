# Print an optspec for argparse to handle cmd's options that are independent of any subcommand.
function __fish_eb_pipeline_global_optspecs
    string join \n h/help V/version
end

function __fish_eb_pipeline_needs_command
    # Figure out if the current invocation already has a command.
    set -l cmd (commandline -opc)
    set -e cmd[1]
    argparse -s (__fish_eb_pipeline_global_optspecs) -- $cmd 2>/dev/null
    or return
    if set -q argv[1]
        # Also print the command, so this can be used to figure out what it is.
        echo $argv[1]
        return 1
    end
    return 0
end

function __fish_eb_pipeline_using_subcommand
    set -l cmd (__fish_eb_pipeline_needs_command)
    test -z "$cmd"
    and return 1
    contains -- $cmd[1] $argv
end

complete -c eb-pipeline -n "__fish_eb_pipeline_needs_command" -s h -l help -d 'Print help (see more with \'--help\')'
complete -c eb-pipeline -n "__fish_eb_pipeline_needs_command" -s V -l version -d 'Print version'
complete -c eb-pipeline -n "__fish_eb_pipeline_needs_command" -f -a "check" -d 'Parse and validate a site file'
complete -c eb-pipeline -n "__fish_eb_pipeline_needs_command" -f -a "schema" -d 'Print the site file reference: every key, type, default and meaning'
complete -c eb-pipeline -n "__fish_eb_pipeline_needs_command" -f -a "init" -d 'Write a starter site file from a built-in template'
complete -c eb-pipeline -n "__fish_eb_pipeline_needs_command" -f -a "explain" -d 'Print the resolved configuration of one or all targets'
complete -c eb-pipeline -n "__fish_eb_pipeline_needs_command" -f -a "targets" -d 'Print the target names in file order, for a CI matrix or a shell loop'
complete -c eb-pipeline -n "__fish_eb_pipeline_needs_command" -f -a "status" -d 'Print the last runs under the log root, newest first, from their summaries'
complete -c eb-pipeline -n "__fish_eb_pipeline_needs_command" -f -a "metrics" -d 'Print Prometheus text, or a short JSON status, for one result file'
complete -c eb-pipeline -n "__fish_eb_pipeline_needs_command" -f -a "plan" -d 'Compute phases, launch lines, environments and tool calls without executing'
complete -c eb-pipeline -n "__fish_eb_pipeline_needs_command" -f -a "run" -d 'Execute the plan\'s phases as subprocesses'
complete -c eb-pipeline -n "__fish_eb_pipeline_needs_command" -f -a "verify" -d 'Recompute the input record and check its inclusion proof'
complete -c eb-pipeline -n "__fish_eb_pipeline_needs_command" -f -a "watch" -d 'Rebuild the recorded installs an EESSI change reaches, in dependency order'
complete -c eb-pipeline -n "__fish_eb_pipeline_needs_command" -f -a "audit" -d 'Run eb-stack vulnerability checks and collect rebuild lists'
complete -c eb-pipeline -n "__fish_eb_pipeline_needs_command" -f -a "completions" -d 'Generate shell completion scripts from the clap command definition'
complete -c eb-pipeline -n "__fish_eb_pipeline_needs_command" -f -a "man" -d 'Generate a section 1 man page from the clap command definition'
complete -c eb-pipeline -n "__fish_eb_pipeline_needs_command" -f -a "help" -d 'Print this message or the help of the given subcommand(s)'
complete -c eb-pipeline -n "__fish_eb_pipeline_using_subcommand check" -l site -d 'Site file' -r -F
complete -c eb-pipeline -n "__fish_eb_pipeline_using_subcommand check" -l lists-dir -d 'Directory that must hold every build list a target names' -r -F
complete -c eb-pipeline -n "__fish_eb_pipeline_using_subcommand check" -l list -d 'Build list or easystack to pass to requested eb-stack checks' -r -F
complete -c eb-pipeline -n "__fish_eb_pipeline_using_subcommand check" -l target -d 'Limit target-dependent settings to this target' -r
complete -c eb-pipeline -n "__fish_eb_pipeline_using_subcommand check" -l stack-checks -d 'Comma-separated eb-stack checks to run before any allocation' -r -f -a "files\t'`stack files`, with robot trees, the easyconfig repository and source paths'
arch\t'`stack arch`, with each selected target\'s `cpu_target` as `--arch`'
cuda-cc\t'`stack cuda-cc`, with the robot trees. `eb-stack` checks its accelerator targets'
generation\t'`stack generation` on the easystack'
provided\t'`stack provided` on a plain build list, with `[sbom].lock` as `--baseline-lock`'
known-failures\t'`stack known-failures`, with robot trees and each selected `cpu_target`'"
complete -c eb-pipeline -n "__fish_eb_pipeline_using_subcommand check" -l probe-tools -d 'Run each needed tool\'s `--version` and enforce min_version'
complete -c eb-pipeline -n "__fish_eb_pipeline_using_subcommand check" -s h -l help -d 'Print help (see more with \'--help\')'
complete -c eb-pipeline -n "__fish_eb_pipeline_using_subcommand schema" -s h -l help -d 'Print help'
complete -c eb-pipeline -n "__fish_eb_pipeline_using_subcommand init" -l template -d 'Template name; `--list` prints the names' -r
complete -c eb-pipeline -n "__fish_eb_pipeline_using_subcommand init" -l out -d 'Write here; absent or `-` prints on stdout. An existing file is refused' -r -F
complete -c eb-pipeline -n "__fish_eb_pipeline_using_subcommand init" -l list -d 'Print the template names and summaries'
complete -c eb-pipeline -n "__fish_eb_pipeline_using_subcommand init" -l hints -d 'After writing a file, print next steps on stderr (also when `EB_PIPELINE_INIT_HINTS=1`)'
complete -c eb-pipeline -n "__fish_eb_pipeline_using_subcommand init" -s h -l help -d 'Print help'
complete -c eb-pipeline -n "__fish_eb_pipeline_using_subcommand explain" -l site -d 'Site file' -r -F
complete -c eb-pipeline -n "__fish_eb_pipeline_using_subcommand explain" -l target -d 'Target name; all targets when absent' -r
complete -c eb-pipeline -n "__fish_eb_pipeline_using_subcommand explain" -l build-type -d 'Apply this build type\'s overrides' -r
complete -c eb-pipeline -n "__fish_eb_pipeline_using_subcommand explain" -l format -d 'Output format' -r -f -a "toml\t''
json\t''"
complete -c eb-pipeline -n "__fish_eb_pipeline_using_subcommand explain" -s h -l help -d 'Print help'
complete -c eb-pipeline -n "__fish_eb_pipeline_using_subcommand targets" -l site -d 'Site file' -r -F
complete -c eb-pipeline -n "__fish_eb_pipeline_using_subcommand targets" -l format -d '`text`: one name per line; `json`: a JSON array on one line' -r -f -a "text\t''
json\t''"
complete -c eb-pipeline -n "__fish_eb_pipeline_using_subcommand targets" -s h -l help -d 'Print help'
complete -c eb-pipeline -n "__fish_eb_pipeline_using_subcommand status" -l site -d 'Site file' -r -F
complete -c eb-pipeline -n "__fish_eb_pipeline_using_subcommand status" -l target -d 'Only the runs of this target, with only its entry' -r
complete -c eb-pipeline -n "__fish_eb_pipeline_using_subcommand status" -l last -d 'How many runs' -r
complete -c eb-pipeline -n "__fish_eb_pipeline_using_subcommand status" -l format -d '`text`: a line per run and per target; `json`: one JSON document' -r -f -a "text\t''
json\t''"
complete -c eb-pipeline -n "__fish_eb_pipeline_using_subcommand status" -s h -l help -d 'Print help'
complete -c eb-pipeline -n "__fish_eb_pipeline_using_subcommand metrics" -l result -d 'Result file written by `run --result`' -r -F
complete -c eb-pipeline -n "__fish_eb_pipeline_using_subcommand metrics" -l format -d '`prometheus`, the default, or `json`' -r -f -a "prometheus\t''
json\t''"
complete -c eb-pipeline -n "__fish_eb_pipeline_using_subcommand metrics" -s h -l help -d 'Print help'
complete -c eb-pipeline -n "__fish_eb_pipeline_using_subcommand plan" -l site -d 'Site file' -r -F
complete -c eb-pipeline -n "__fish_eb_pipeline_using_subcommand plan" -l list -d 'Build list file: one `Name-version-toolchain.eb [eb options]` per line, or an EasyBuild easystack, a `.yml` or `.yaml` file' -r -F
complete -c eb-pipeline -n "__fish_eb_pipeline_using_subcommand plan" -l package-plan -d 'Directory written by `eb-stack package plan`; builds its emitted recipes' -r -F
complete -c eb-pipeline -n "__fish_eb_pipeline_using_subcommand plan" -l target -d 'Target name' -r
complete -c eb-pipeline -n "__fish_eb_pipeline_using_subcommand plan" -l build-type -d 'Build type name (selects per-build-type overrides)' -r
complete -c eb-pipeline -n "__fish_eb_pipeline_using_subcommand plan" -l run-id -d 'Run label recorded in the result and passed to tools; a staged rebuild line passes `<run-id>-<easyconfig stem>`. Default: the log stamp for `run`, `<run-id>` for `plan`' -r
complete -c eb-pipeline -n "__fish_eb_pipeline_using_subcommand plan" -l log-stamp -d 'Timestamp directory under the log root: a UTC time as `YYYYMMDD_HHMMSS`, the name `eb-keep-logs rotate` ages. Default: the current UTC time, taken once per run; `<log-stamp>` for `plan`' -r
complete -c eb-pipeline -n "__fish_eb_pipeline_using_subcommand plan" -l offline-bundle -d 'Build a checksum-indexed bundle of prefetched and configured assets' -r -F
complete -c eb-pipeline -n "__fish_eb_pipeline_using_subcommand plan" -l out -d 'Write the plan JSON here; `-` or absent prints it on stdout' -r -F
complete -c eb-pipeline -n "__fish_eb_pipeline_using_subcommand plan" -l shard-plan -d 'Directory written by `stack shard`; expands its units and build lines' -r -F
complete -c eb-pipeline -n "__fish_eb_pipeline_using_subcommand plan" -l verify-package-plan -d 'Run ecosystem-native checks after a package-plan build'
complete -c eb-pipeline -n "__fish_eb_pipeline_using_subcommand plan" -l require-offline -d 'Run phases after source prefetch through sources.offline_wrapper'
complete -c eb-pipeline -n "__fish_eb_pipeline_using_subcommand plan" -s h -l help -d 'Print help'
complete -c eb-pipeline -n "__fish_eb_pipeline_using_subcommand run" -l site -d 'Site file' -r -F
complete -c eb-pipeline -n "__fish_eb_pipeline_using_subcommand run" -l list -d 'Build list file: one `Name-version-toolchain.eb [eb options]` per line, or an EasyBuild easystack, a `.yml` or `.yaml` file' -r -F
complete -c eb-pipeline -n "__fish_eb_pipeline_using_subcommand run" -l package-plan -d 'Directory written by `eb-stack package plan`; builds its emitted recipes' -r -F
complete -c eb-pipeline -n "__fish_eb_pipeline_using_subcommand run" -l target -d 'Target name' -r
complete -c eb-pipeline -n "__fish_eb_pipeline_using_subcommand run" -l build-type -d 'Build type name (selects per-build-type overrides)' -r
complete -c eb-pipeline -n "__fish_eb_pipeline_using_subcommand run" -l run-id -d 'Run label recorded in the result and passed to tools; a staged rebuild line passes `<run-id>-<easyconfig stem>`. Default: the log stamp for `run`, `<run-id>` for `plan`' -r
complete -c eb-pipeline -n "__fish_eb_pipeline_using_subcommand run" -l log-stamp -d 'Timestamp directory under the log root: a UTC time as `YYYYMMDD_HHMMSS`, the name `eb-keep-logs rotate` ages. Default: the current UTC time, taken once per run; `<log-stamp>` for `plan`' -r
complete -c eb-pipeline -n "__fish_eb_pipeline_using_subcommand run" -l offline-bundle -d 'Build a checksum-indexed bundle of prefetched and configured assets' -r -F
complete -c eb-pipeline -n "__fish_eb_pipeline_using_subcommand run" -l result -d 'Write the result JSON here' -r -F
complete -c eb-pipeline -n "__fish_eb_pipeline_using_subcommand run" -l metrics-file -d 'Atomically write Prometheus metrics here when the run ends' -r -F
complete -c eb-pipeline -n "__fish_eb_pipeline_using_subcommand run" -l shard-plan -d 'Directory written by `stack shard`; reuse its units without replanning' -r -F
complete -c eb-pipeline -n "__fish_eb_pipeline_using_subcommand run" -l node -d 'Node size for `--oom-preflight`, as `CORES:MEMORY_MIB[:TARGET]`' -r
complete -c eb-pipeline -n "__fish_eb_pipeline_using_subcommand run" -l resume-result -d 'Continue from this result when its input digest still matches. Needs `[resume] enabled`' -r -F
complete -c eb-pipeline -n "__fish_eb_pipeline_using_subcommand run" -l verify-package-plan -d 'Run ecosystem-native checks after a package-plan build'
complete -c eb-pipeline -n "__fish_eb_pipeline_using_subcommand run" -l require-offline -d 'Run phases after source prefetch through sources.offline_wrapper'
complete -c eb-pipeline -n "__fish_eb_pipeline_using_subcommand run" -l dry-run -d 'Print the commands that would run and start nothing'
complete -c eb-pipeline -n "__fish_eb_pipeline_using_subcommand run" -l oom-preflight -d 'Ask `eb-stack stack oom` whether any planned package exceeds this node'
complete -c eb-pipeline -n "__fish_eb_pipeline_using_subcommand run" -s h -l help -d 'Print help (see more with \'--help\')'
complete -c eb-pipeline -n "__fish_eb_pipeline_using_subcommand verify" -l site -d 'Site file' -r -F
complete -c eb-pipeline -n "__fish_eb_pipeline_using_subcommand verify" -l list -d 'Build list file: one `Name-version-toolchain.eb [eb options]` per line, or an EasyBuild easystack, a `.yml` or `.yaml` file' -r -F
complete -c eb-pipeline -n "__fish_eb_pipeline_using_subcommand verify" -l package-plan -d 'Directory written by `eb-stack package plan`; builds its emitted recipes' -r -F
complete -c eb-pipeline -n "__fish_eb_pipeline_using_subcommand verify" -l target -d 'Target name' -r
complete -c eb-pipeline -n "__fish_eb_pipeline_using_subcommand verify" -l build-type -d 'Build type name (selects per-build-type overrides)' -r
complete -c eb-pipeline -n "__fish_eb_pipeline_using_subcommand verify" -l run-id -d 'Run label recorded in the result and passed to tools; a staged rebuild line passes `<run-id>-<easyconfig stem>`. Default: the log stamp for `run`, `<run-id>` for `plan`' -r
complete -c eb-pipeline -n "__fish_eb_pipeline_using_subcommand verify" -l log-stamp -d 'Timestamp directory under the log root: a UTC time as `YYYYMMDD_HHMMSS`, the name `eb-keep-logs rotate` ages. Default: the current UTC time, taken once per run; `<log-stamp>` for `plan`' -r
complete -c eb-pipeline -n "__fish_eb_pipeline_using_subcommand verify" -l offline-bundle -d 'Build a checksum-indexed bundle of prefetched and configured assets' -r -F
complete -c eb-pipeline -n "__fish_eb_pipeline_using_subcommand verify" -l from -d 'Plan or result JSON that holds `input_attestation`' -r -F
complete -c eb-pipeline -n "__fish_eb_pipeline_using_subcommand verify" -l verify-package-plan -d 'Run ecosystem-native checks after a package-plan build'
complete -c eb-pipeline -n "__fish_eb_pipeline_using_subcommand verify" -l require-offline -d 'Run phases after source prefetch through sources.offline_wrapper'
complete -c eb-pipeline -n "__fish_eb_pipeline_using_subcommand verify" -s h -l help -d 'Print help'
complete -c eb-pipeline -n "__fish_eb_pipeline_using_subcommand watch" -l site -d 'Site file' -r -F
complete -c eb-pipeline -n "__fish_eb_pipeline_using_subcommand watch" -l target -d 'Target name' -r
complete -c eb-pipeline -n "__fish_eb_pipeline_using_subcommand watch" -l build-type -d 'Build type of the rebuild (selects per-build-type overrides)' -r
complete -c eb-pipeline -n "__fish_eb_pipeline_using_subcommand watch" -l run-id -d 'Run label of the rebuild. Default: the log stamp' -r
complete -c eb-pipeline -n "__fish_eb_pipeline_using_subcommand watch" -l log-stamp -d 'Timestamp directory of the rebuild under the log root, as for `run`. Default: the current UTC time' -r
complete -c eb-pipeline -n "__fish_eb_pipeline_using_subcommand watch" -l plan-out -d 'Write the plan JSON here' -r -F
complete -c eb-pipeline -n "__fish_eb_pipeline_using_subcommand watch" -l result -d 'Write the rebuild\'s result JSON here' -r -F
complete -c eb-pipeline -n "__fish_eb_pipeline_using_subcommand watch" -l dry-run -d 'Print the plan and rebuild nothing'
complete -c eb-pipeline -n "__fish_eb_pipeline_using_subcommand watch" -l adopt -d 'Record every install in the target\'s module tree against the EESSI loaded now, in place of the record, and rebuild nothing'
complete -c eb-pipeline -n "__fish_eb_pipeline_using_subcommand watch" -s h -l help -d 'Print help (see more with \'--help\')'
complete -c eb-pipeline -n "__fish_eb_pipeline_using_subcommand audit" -l site -d 'Site file' -r -F
complete -c eb-pipeline -n "__fish_eb_pipeline_using_subcommand audit" -l target -d 'Target name' -r
complete -c eb-pipeline -n "__fish_eb_pipeline_using_subcommand audit" -l findings -d 'Vulnerability scanner CSV for `eb-stack rebuild-set --findings`' -r -F
complete -c eb-pipeline -n "__fish_eb_pipeline_using_subcommand audit" -l cve -d 'CVE id to audit. Repeat for multiple advisories' -r
complete -c eb-pipeline -n "__fish_eb_pipeline_using_subcommand audit" -l out-dir -d 'Output directory for audit artifacts and copied rebuild lists' -r -F
complete -c eb-pipeline -n "__fish_eb_pipeline_using_subcommand audit" -l strict -d 'Exit non-zero when eb-stack reports backport items'
complete -c eb-pipeline -n "__fish_eb_pipeline_using_subcommand audit" -s h -l help -d 'Print help'
complete -c eb-pipeline -n "__fish_eb_pipeline_using_subcommand completions" -l shell -d 'Target shell' -r -f -a "bash\t''
zsh\t''
fish\t''"
complete -c eb-pipeline -n "__fish_eb_pipeline_using_subcommand completions" -l out-dir -d 'Output directory' -r -F
complete -c eb-pipeline -n "__fish_eb_pipeline_using_subcommand completions" -s h -l help -d 'Print help'
complete -c eb-pipeline -n "__fish_eb_pipeline_using_subcommand man" -l out-dir -d 'Output directory' -r -F
complete -c eb-pipeline -n "__fish_eb_pipeline_using_subcommand man" -s h -l help -d 'Print help'
complete -c eb-pipeline -n "__fish_eb_pipeline_using_subcommand help; and not __fish_seen_subcommand_from check schema init explain targets status metrics plan run verify watch audit completions man help" -f -a "check" -d 'Parse and validate a site file'
complete -c eb-pipeline -n "__fish_eb_pipeline_using_subcommand help; and not __fish_seen_subcommand_from check schema init explain targets status metrics plan run verify watch audit completions man help" -f -a "schema" -d 'Print the site file reference: every key, type, default and meaning'
complete -c eb-pipeline -n "__fish_eb_pipeline_using_subcommand help; and not __fish_seen_subcommand_from check schema init explain targets status metrics plan run verify watch audit completions man help" -f -a "init" -d 'Write a starter site file from a built-in template'
complete -c eb-pipeline -n "__fish_eb_pipeline_using_subcommand help; and not __fish_seen_subcommand_from check schema init explain targets status metrics plan run verify watch audit completions man help" -f -a "explain" -d 'Print the resolved configuration of one or all targets'
complete -c eb-pipeline -n "__fish_eb_pipeline_using_subcommand help; and not __fish_seen_subcommand_from check schema init explain targets status metrics plan run verify watch audit completions man help" -f -a "targets" -d 'Print the target names in file order, for a CI matrix or a shell loop'
complete -c eb-pipeline -n "__fish_eb_pipeline_using_subcommand help; and not __fish_seen_subcommand_from check schema init explain targets status metrics plan run verify watch audit completions man help" -f -a "status" -d 'Print the last runs under the log root, newest first, from their summaries'
complete -c eb-pipeline -n "__fish_eb_pipeline_using_subcommand help; and not __fish_seen_subcommand_from check schema init explain targets status metrics plan run verify watch audit completions man help" -f -a "metrics" -d 'Print Prometheus text, or a short JSON status, for one result file'
complete -c eb-pipeline -n "__fish_eb_pipeline_using_subcommand help; and not __fish_seen_subcommand_from check schema init explain targets status metrics plan run verify watch audit completions man help" -f -a "plan" -d 'Compute phases, launch lines, environments and tool calls without executing'
complete -c eb-pipeline -n "__fish_eb_pipeline_using_subcommand help; and not __fish_seen_subcommand_from check schema init explain targets status metrics plan run verify watch audit completions man help" -f -a "run" -d 'Execute the plan\'s phases as subprocesses'
complete -c eb-pipeline -n "__fish_eb_pipeline_using_subcommand help; and not __fish_seen_subcommand_from check schema init explain targets status metrics plan run verify watch audit completions man help" -f -a "verify" -d 'Recompute the input record and check its inclusion proof'
complete -c eb-pipeline -n "__fish_eb_pipeline_using_subcommand help; and not __fish_seen_subcommand_from check schema init explain targets status metrics plan run verify watch audit completions man help" -f -a "watch" -d 'Rebuild the recorded installs an EESSI change reaches, in dependency order'
complete -c eb-pipeline -n "__fish_eb_pipeline_using_subcommand help; and not __fish_seen_subcommand_from check schema init explain targets status metrics plan run verify watch audit completions man help" -f -a "audit" -d 'Run eb-stack vulnerability checks and collect rebuild lists'
complete -c eb-pipeline -n "__fish_eb_pipeline_using_subcommand help; and not __fish_seen_subcommand_from check schema init explain targets status metrics plan run verify watch audit completions man help" -f -a "completions" -d 'Generate shell completion scripts from the clap command definition'
complete -c eb-pipeline -n "__fish_eb_pipeline_using_subcommand help; and not __fish_seen_subcommand_from check schema init explain targets status metrics plan run verify watch audit completions man help" -f -a "man" -d 'Generate a section 1 man page from the clap command definition'
complete -c eb-pipeline -n "__fish_eb_pipeline_using_subcommand help; and not __fish_seen_subcommand_from check schema init explain targets status metrics plan run verify watch audit completions man help" -f -a "help" -d 'Print this message or the help of the given subcommand(s)'
