_eb__pipeline() {
    local i cur prev opts cmd
    COMPREPLY=()
    if [[ "${BASH_VERSINFO[0]}" -ge 4 ]]; then
        cur="$2"
    else
        cur="${COMP_WORDS[COMP_CWORD]}"
    fi
    prev="$3"
    cmd=""
    opts=""

    for i in "${COMP_WORDS[@]:0:COMP_CWORD}"
    do
        case "${cmd},${i}" in
            ",$1")
                cmd="eb__pipeline"
                ;;
            eb__pipeline,audit)
                cmd="eb__pipeline__subcmd__audit"
                ;;
            eb__pipeline,check)
                cmd="eb__pipeline__subcmd__check"
                ;;
            eb__pipeline,completions)
                cmd="eb__pipeline__subcmd__completions"
                ;;
            eb__pipeline,explain)
                cmd="eb__pipeline__subcmd__explain"
                ;;
            eb__pipeline,help)
                cmd="eb__pipeline__subcmd__help"
                ;;
            eb__pipeline,init)
                cmd="eb__pipeline__subcmd__init"
                ;;
            eb__pipeline,man)
                cmd="eb__pipeline__subcmd__man"
                ;;
            eb__pipeline,metrics)
                cmd="eb__pipeline__subcmd__metrics"
                ;;
            eb__pipeline,plan)
                cmd="eb__pipeline__subcmd__plan"
                ;;
            eb__pipeline,run)
                cmd="eb__pipeline__subcmd__run"
                ;;
            eb__pipeline,schema)
                cmd="eb__pipeline__subcmd__schema"
                ;;
            eb__pipeline,status)
                cmd="eb__pipeline__subcmd__status"
                ;;
            eb__pipeline,targets)
                cmd="eb__pipeline__subcmd__targets"
                ;;
            eb__pipeline,verify)
                cmd="eb__pipeline__subcmd__verify"
                ;;
            eb__pipeline,watch)
                cmd="eb__pipeline__subcmd__watch"
                ;;
            eb__pipeline__subcmd__help,audit)
                cmd="eb__pipeline__subcmd__help__subcmd__audit"
                ;;
            eb__pipeline__subcmd__help,check)
                cmd="eb__pipeline__subcmd__help__subcmd__check"
                ;;
            eb__pipeline__subcmd__help,completions)
                cmd="eb__pipeline__subcmd__help__subcmd__completions"
                ;;
            eb__pipeline__subcmd__help,explain)
                cmd="eb__pipeline__subcmd__help__subcmd__explain"
                ;;
            eb__pipeline__subcmd__help,help)
                cmd="eb__pipeline__subcmd__help__subcmd__help"
                ;;
            eb__pipeline__subcmd__help,init)
                cmd="eb__pipeline__subcmd__help__subcmd__init"
                ;;
            eb__pipeline__subcmd__help,man)
                cmd="eb__pipeline__subcmd__help__subcmd__man"
                ;;
            eb__pipeline__subcmd__help,metrics)
                cmd="eb__pipeline__subcmd__help__subcmd__metrics"
                ;;
            eb__pipeline__subcmd__help,plan)
                cmd="eb__pipeline__subcmd__help__subcmd__plan"
                ;;
            eb__pipeline__subcmd__help,run)
                cmd="eb__pipeline__subcmd__help__subcmd__run"
                ;;
            eb__pipeline__subcmd__help,schema)
                cmd="eb__pipeline__subcmd__help__subcmd__schema"
                ;;
            eb__pipeline__subcmd__help,status)
                cmd="eb__pipeline__subcmd__help__subcmd__status"
                ;;
            eb__pipeline__subcmd__help,targets)
                cmd="eb__pipeline__subcmd__help__subcmd__targets"
                ;;
            eb__pipeline__subcmd__help,verify)
                cmd="eb__pipeline__subcmd__help__subcmd__verify"
                ;;
            eb__pipeline__subcmd__help,watch)
                cmd="eb__pipeline__subcmd__help__subcmd__watch"
                ;;
            *)
                ;;
        esac
    done

    case "${cmd}" in
        eb__pipeline)
            opts="-h -V --help --version check schema init explain targets status metrics plan run verify watch audit completions man help"
            if [[ ${cur} == -* || ${COMP_CWORD} -eq 1 ]] ; then
                COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
                return 0
            fi
            case "${prev}" in
                *)
                    COMPREPLY=()
                    ;;
            esac
            COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
            return 0
            ;;
        eb__subcmd__pipeline__subcmd__audit)
            opts="-h --site --target --findings --cve --strict --out-dir --help"
            if [[ ${cur} == -* || ${COMP_CWORD} -eq 2 ]] ; then
                COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
                return 0
            fi
            case "${prev}" in
                --site)
                    COMPREPLY=($(compgen -f "${cur}"))
                    return 0
                    ;;
                --target)
                    COMPREPLY=($(compgen -f "${cur}"))
                    return 0
                    ;;
                --findings)
                    COMPREPLY=($(compgen -f "${cur}"))
                    return 0
                    ;;
                --cve)
                    COMPREPLY=($(compgen -f "${cur}"))
                    return 0
                    ;;
                --out-dir)
                    COMPREPLY=($(compgen -f "${cur}"))
                    return 0
                    ;;
                *)
                    COMPREPLY=()
                    ;;
            esac
            COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
            return 0
            ;;
        eb__subcmd__pipeline__subcmd__check)
            opts="-h --site --lists-dir --probe-tools --list --target --stack-checks --check-from-commit --easyconfigs-repo --easyblocks-repo --easybuild-version --gap-out --generation --shadow --help"
            if [[ ${cur} == -* || ${COMP_CWORD} -eq 2 ]] ; then
                COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
                return 0
            fi
            case "${prev}" in
                --site)
                    COMPREPLY=($(compgen -f "${cur}"))
                    return 0
                    ;;
                --lists-dir)
                    COMPREPLY=($(compgen -f "${cur}"))
                    return 0
                    ;;
                --list)
                    COMPREPLY=($(compgen -f "${cur}"))
                    return 0
                    ;;
                --target)
                    COMPREPLY=($(compgen -f "${cur}"))
                    return 0
                    ;;
                --stack-checks)
                    COMPREPLY=($(compgen -W "files arch cuda-cc generation provided known-failures" -- "${cur}"))
                    return 0
                    ;;
                --easyconfigs-repo)
                    COMPREPLY=($(compgen -f "${cur}"))
                    return 0
                    ;;
                --easyblocks-repo)
                    COMPREPLY=($(compgen -f "${cur}"))
                    return 0
                    ;;
                --easybuild-version)
                    COMPREPLY=($(compgen -f "${cur}"))
                    return 0
                    ;;
                --gap-out)
                    COMPREPLY=($(compgen -f "${cur}"))
                    return 0
                    ;;
                --generation)
                    COMPREPLY=($(compgen -f "${cur}"))
                    return 0
                    ;;
                *)
                    COMPREPLY=()
                    ;;
            esac
            COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
            return 0
            ;;
        eb__subcmd__pipeline__subcmd__completions)
            opts="-h --shell --out-dir --help"
            if [[ ${cur} == -* || ${COMP_CWORD} -eq 2 ]] ; then
                COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
                return 0
            fi
            case "${prev}" in
                --shell)
                    COMPREPLY=($(compgen -W "bash zsh fish" -- "${cur}"))
                    return 0
                    ;;
                --out-dir)
                    COMPREPLY=($(compgen -f "${cur}"))
                    return 0
                    ;;
                *)
                    COMPREPLY=()
                    ;;
            esac
            COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
            return 0
            ;;
        eb__subcmd__pipeline__subcmd__explain)
            opts="-h --site --target --build-type --format --help"
            if [[ ${cur} == -* || ${COMP_CWORD} -eq 2 ]] ; then
                COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
                return 0
            fi
            case "${prev}" in
                --site)
                    COMPREPLY=($(compgen -f "${cur}"))
                    return 0
                    ;;
                --target)
                    COMPREPLY=($(compgen -f "${cur}"))
                    return 0
                    ;;
                --build-type)
                    COMPREPLY=($(compgen -f "${cur}"))
                    return 0
                    ;;
                --format)
                    COMPREPLY=($(compgen -W "toml json" -- "${cur}"))
                    return 0
                    ;;
                *)
                    COMPREPLY=()
                    ;;
            esac
            COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
            return 0
            ;;
        eb__subcmd__pipeline__subcmd__help)
            opts="check schema init explain targets status metrics plan run verify watch audit completions man help"
            if [[ ${cur} == -* || ${COMP_CWORD} -eq 2 ]] ; then
                COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
                return 0
            fi
            case "${prev}" in
                *)
                    COMPREPLY=()
                    ;;
            esac
            COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
            return 0
            ;;
        eb__subcmd__pipeline__subcmd__help__subcmd__audit)
            opts=""
            if [[ ${cur} == -* || ${COMP_CWORD} -eq 3 ]] ; then
                COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
                return 0
            fi
            case "${prev}" in
                *)
                    COMPREPLY=()
                    ;;
            esac
            COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
            return 0
            ;;
        eb__subcmd__pipeline__subcmd__help__subcmd__check)
            opts=""
            if [[ ${cur} == -* || ${COMP_CWORD} -eq 3 ]] ; then
                COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
                return 0
            fi
            case "${prev}" in
                *)
                    COMPREPLY=()
                    ;;
            esac
            COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
            return 0
            ;;
        eb__subcmd__pipeline__subcmd__help__subcmd__completions)
            opts=""
            if [[ ${cur} == -* || ${COMP_CWORD} -eq 3 ]] ; then
                COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
                return 0
            fi
            case "${prev}" in
                *)
                    COMPREPLY=()
                    ;;
            esac
            COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
            return 0
            ;;
        eb__subcmd__pipeline__subcmd__help__subcmd__explain)
            opts=""
            if [[ ${cur} == -* || ${COMP_CWORD} -eq 3 ]] ; then
                COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
                return 0
            fi
            case "${prev}" in
                *)
                    COMPREPLY=()
                    ;;
            esac
            COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
            return 0
            ;;
        eb__subcmd__pipeline__subcmd__help__subcmd__help)
            opts=""
            if [[ ${cur} == -* || ${COMP_CWORD} -eq 3 ]] ; then
                COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
                return 0
            fi
            case "${prev}" in
                *)
                    COMPREPLY=()
                    ;;
            esac
            COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
            return 0
            ;;
        eb__subcmd__pipeline__subcmd__help__subcmd__init)
            opts=""
            if [[ ${cur} == -* || ${COMP_CWORD} -eq 3 ]] ; then
                COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
                return 0
            fi
            case "${prev}" in
                *)
                    COMPREPLY=()
                    ;;
            esac
            COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
            return 0
            ;;
        eb__subcmd__pipeline__subcmd__help__subcmd__man)
            opts=""
            if [[ ${cur} == -* || ${COMP_CWORD} -eq 3 ]] ; then
                COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
                return 0
            fi
            case "${prev}" in
                *)
                    COMPREPLY=()
                    ;;
            esac
            COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
            return 0
            ;;
        eb__subcmd__pipeline__subcmd__help__subcmd__metrics)
            opts=""
            if [[ ${cur} == -* || ${COMP_CWORD} -eq 3 ]] ; then
                COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
                return 0
            fi
            case "${prev}" in
                *)
                    COMPREPLY=()
                    ;;
            esac
            COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
            return 0
            ;;
        eb__subcmd__pipeline__subcmd__help__subcmd__plan)
            opts=""
            if [[ ${cur} == -* || ${COMP_CWORD} -eq 3 ]] ; then
                COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
                return 0
            fi
            case "${prev}" in
                *)
                    COMPREPLY=()
                    ;;
            esac
            COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
            return 0
            ;;
        eb__subcmd__pipeline__subcmd__help__subcmd__run)
            opts=""
            if [[ ${cur} == -* || ${COMP_CWORD} -eq 3 ]] ; then
                COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
                return 0
            fi
            case "${prev}" in
                *)
                    COMPREPLY=()
                    ;;
            esac
            COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
            return 0
            ;;
        eb__subcmd__pipeline__subcmd__help__subcmd__schema)
            opts=""
            if [[ ${cur} == -* || ${COMP_CWORD} -eq 3 ]] ; then
                COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
                return 0
            fi
            case "${prev}" in
                *)
                    COMPREPLY=()
                    ;;
            esac
            COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
            return 0
            ;;
        eb__subcmd__pipeline__subcmd__help__subcmd__status)
            opts=""
            if [[ ${cur} == -* || ${COMP_CWORD} -eq 3 ]] ; then
                COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
                return 0
            fi
            case "${prev}" in
                *)
                    COMPREPLY=()
                    ;;
            esac
            COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
            return 0
            ;;
        eb__subcmd__pipeline__subcmd__help__subcmd__targets)
            opts=""
            if [[ ${cur} == -* || ${COMP_CWORD} -eq 3 ]] ; then
                COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
                return 0
            fi
            case "${prev}" in
                *)
                    COMPREPLY=()
                    ;;
            esac
            COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
            return 0
            ;;
        eb__subcmd__pipeline__subcmd__help__subcmd__verify)
            opts=""
            if [[ ${cur} == -* || ${COMP_CWORD} -eq 3 ]] ; then
                COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
                return 0
            fi
            case "${prev}" in
                *)
                    COMPREPLY=()
                    ;;
            esac
            COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
            return 0
            ;;
        eb__subcmd__pipeline__subcmd__help__subcmd__watch)
            opts=""
            if [[ ${cur} == -* || ${COMP_CWORD} -eq 3 ]] ; then
                COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
                return 0
            fi
            case "${prev}" in
                *)
                    COMPREPLY=()
                    ;;
            esac
            COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
            return 0
            ;;
        eb__subcmd__pipeline__subcmd__init)
            opts="-h --template --list --out --hints --help"
            if [[ ${cur} == -* || ${COMP_CWORD} -eq 2 ]] ; then
                COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
                return 0
            fi
            case "${prev}" in
                --template)
                    COMPREPLY=($(compgen -f "${cur}"))
                    return 0
                    ;;
                --out)
                    COMPREPLY=($(compgen -f "${cur}"))
                    return 0
                    ;;
                *)
                    COMPREPLY=()
                    ;;
            esac
            COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
            return 0
            ;;
        eb__subcmd__pipeline__subcmd__man)
            opts="-h --out-dir --help"
            if [[ ${cur} == -* || ${COMP_CWORD} -eq 2 ]] ; then
                COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
                return 0
            fi
            case "${prev}" in
                --out-dir)
                    COMPREPLY=($(compgen -f "${cur}"))
                    return 0
                    ;;
                *)
                    COMPREPLY=()
                    ;;
            esac
            COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
            return 0
            ;;
        eb__subcmd__pipeline__subcmd__metrics)
            opts="-h --result --format --help"
            if [[ ${cur} == -* || ${COMP_CWORD} -eq 2 ]] ; then
                COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
                return 0
            fi
            case "${prev}" in
                --result)
                    COMPREPLY=($(compgen -f "${cur}"))
                    return 0
                    ;;
                --format)
                    COMPREPLY=($(compgen -W "prometheus json" -- "${cur}"))
                    return 0
                    ;;
                *)
                    COMPREPLY=()
                    ;;
            esac
            COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
            return 0
            ;;
        eb__subcmd__pipeline__subcmd__plan)
            opts="-h --site --list --package-plan --verify-package-plan --target --build-type --run-id --log-stamp --offline-bundle --require-offline --out --shard-plan --help"
            if [[ ${cur} == -* || ${COMP_CWORD} -eq 2 ]] ; then
                COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
                return 0
            fi
            case "${prev}" in
                --site)
                    COMPREPLY=($(compgen -f "${cur}"))
                    return 0
                    ;;
                --list)
                    COMPREPLY=($(compgen -f "${cur}"))
                    return 0
                    ;;
                --package-plan)
                    COMPREPLY=($(compgen -f "${cur}"))
                    return 0
                    ;;
                --target)
                    COMPREPLY=($(compgen -f "${cur}"))
                    return 0
                    ;;
                --build-type)
                    COMPREPLY=($(compgen -f "${cur}"))
                    return 0
                    ;;
                --run-id)
                    COMPREPLY=($(compgen -f "${cur}"))
                    return 0
                    ;;
                --log-stamp)
                    COMPREPLY=($(compgen -f "${cur}"))
                    return 0
                    ;;
                --offline-bundle)
                    COMPREPLY=($(compgen -f "${cur}"))
                    return 0
                    ;;
                --out)
                    COMPREPLY=($(compgen -f "${cur}"))
                    return 0
                    ;;
                --shard-plan)
                    COMPREPLY=($(compgen -f "${cur}"))
                    return 0
                    ;;
                *)
                    COMPREPLY=()
                    ;;
            esac
            COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
            return 0
            ;;
        eb__subcmd__pipeline__subcmd__run)
            opts="-h --site --list --package-plan --verify-package-plan --target --build-type --run-id --log-stamp --offline-bundle --require-offline --result --plan-out --metrics-file --shard-plan --dry-run --oom-preflight --node --resume-result --help"
            if [[ ${cur} == -* || ${COMP_CWORD} -eq 2 ]] ; then
                COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
                return 0
            fi
            case "${prev}" in
                --site)
                    COMPREPLY=($(compgen -f "${cur}"))
                    return 0
                    ;;
                --list)
                    COMPREPLY=($(compgen -f "${cur}"))
                    return 0
                    ;;
                --package-plan)
                    COMPREPLY=($(compgen -f "${cur}"))
                    return 0
                    ;;
                --target)
                    COMPREPLY=($(compgen -f "${cur}"))
                    return 0
                    ;;
                --build-type)
                    COMPREPLY=($(compgen -f "${cur}"))
                    return 0
                    ;;
                --run-id)
                    COMPREPLY=($(compgen -f "${cur}"))
                    return 0
                    ;;
                --log-stamp)
                    COMPREPLY=($(compgen -f "${cur}"))
                    return 0
                    ;;
                --offline-bundle)
                    COMPREPLY=($(compgen -f "${cur}"))
                    return 0
                    ;;
                --result)
                    COMPREPLY=($(compgen -f "${cur}"))
                    return 0
                    ;;
                --plan-out)
                    COMPREPLY=($(compgen -f "${cur}"))
                    return 0
                    ;;
                --metrics-file)
                    COMPREPLY=($(compgen -f "${cur}"))
                    return 0
                    ;;
                --shard-plan)
                    COMPREPLY=($(compgen -f "${cur}"))
                    return 0
                    ;;
                --node)
                    COMPREPLY=($(compgen -f "${cur}"))
                    return 0
                    ;;
                --resume-result)
                    COMPREPLY=($(compgen -f "${cur}"))
                    return 0
                    ;;
                *)
                    COMPREPLY=()
                    ;;
            esac
            COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
            return 0
            ;;
        eb__subcmd__pipeline__subcmd__schema)
            opts="-h --help"
            if [[ ${cur} == -* || ${COMP_CWORD} -eq 2 ]] ; then
                COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
                return 0
            fi
            case "${prev}" in
                *)
                    COMPREPLY=()
                    ;;
            esac
            COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
            return 0
            ;;
        eb__subcmd__pipeline__subcmd__status)
            opts="-h --site --target --last --problems-only --format --help"
            if [[ ${cur} == -* || ${COMP_CWORD} -eq 2 ]] ; then
                COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
                return 0
            fi
            case "${prev}" in
                --site)
                    COMPREPLY=($(compgen -f "${cur}"))
                    return 0
                    ;;
                --target)
                    COMPREPLY=($(compgen -f "${cur}"))
                    return 0
                    ;;
                --last)
                    COMPREPLY=($(compgen -f "${cur}"))
                    return 0
                    ;;
                --format)
                    COMPREPLY=($(compgen -W "text json" -- "${cur}"))
                    return 0
                    ;;
                *)
                    COMPREPLY=()
                    ;;
            esac
            COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
            return 0
            ;;
        eb__subcmd__pipeline__subcmd__targets)
            opts="-h --site --format --help"
            if [[ ${cur} == -* || ${COMP_CWORD} -eq 2 ]] ; then
                COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
                return 0
            fi
            case "${prev}" in
                --site)
                    COMPREPLY=($(compgen -f "${cur}"))
                    return 0
                    ;;
                --format)
                    COMPREPLY=($(compgen -W "text json" -- "${cur}"))
                    return 0
                    ;;
                *)
                    COMPREPLY=()
                    ;;
            esac
            COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
            return 0
            ;;
        eb__subcmd__pipeline__subcmd__verify)
            opts="-h --site --list --recorded-list --target --build-type --from --help"
            if [[ ${cur} == -* || ${COMP_CWORD} -eq 2 ]] ; then
                COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
                return 0
            fi
            case "${prev}" in
                --site)
                    COMPREPLY=($(compgen -f "${cur}"))
                    return 0
                    ;;
                --list)
                    COMPREPLY=($(compgen -f "${cur}"))
                    return 0
                    ;;
                --target)
                    COMPREPLY=($(compgen -f "${cur}"))
                    return 0
                    ;;
                --build-type)
                    COMPREPLY=($(compgen -f "${cur}"))
                    return 0
                    ;;
                --from)
                    COMPREPLY=($(compgen -f "${cur}"))
                    return 0
                    ;;
                *)
                    COMPREPLY=()
                    ;;
            esac
            COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
            return 0
            ;;
        eb__subcmd__pipeline__subcmd__watch)
            opts="-h --site --target --build-type --run-id --log-stamp --dry-run --adopt --plan-out --result --help"
            if [[ ${cur} == -* || ${COMP_CWORD} -eq 2 ]] ; then
                COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
                return 0
            fi
            case "${prev}" in
                --site)
                    COMPREPLY=($(compgen -f "${cur}"))
                    return 0
                    ;;
                --target)
                    COMPREPLY=($(compgen -f "${cur}"))
                    return 0
                    ;;
                --build-type)
                    COMPREPLY=($(compgen -f "${cur}"))
                    return 0
                    ;;
                --run-id)
                    COMPREPLY=($(compgen -f "${cur}"))
                    return 0
                    ;;
                --log-stamp)
                    COMPREPLY=($(compgen -f "${cur}"))
                    return 0
                    ;;
                --plan-out)
                    COMPREPLY=($(compgen -f "${cur}"))
                    return 0
                    ;;
                --result)
                    COMPREPLY=($(compgen -f "${cur}"))
                    return 0
                    ;;
                *)
                    COMPREPLY=()
                    ;;
            esac
            COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
            return 0
            ;;
    esac
}

if [[ "${BASH_VERSINFO[0]}" -eq 4 && "${BASH_VERSINFO[1]}" -ge 4 || "${BASH_VERSINFO[0]}" -gt 4 ]]; then
    complete -F _eb__pipeline -o nosort -o bashdefault -o default eb-pipeline
else
    complete -F _eb__pipeline -o bashdefault -o default eb-pipeline
fi
