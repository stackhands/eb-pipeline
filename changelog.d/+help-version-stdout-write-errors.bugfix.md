`eb-pipeline --help` and `--version` now report stdout write failures (for
example `/dev/full`) as runtime errors instead of exiting successfully without
diagnostic output.
