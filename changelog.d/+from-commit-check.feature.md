`eb-pipeline check --list FILE --check-from-commit` is opt-in. It runs
`eb-stack stack from-commit --json` with JavaScript Object Notation (JSON)
output on an easystack before a pipeline run.
Continuous integration (CI) can then refuse stale pins already in the EasyBuild
release tag.
