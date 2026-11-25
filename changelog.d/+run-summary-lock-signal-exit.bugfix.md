`eb-pipeline run` now preserves `SIGINT`/`SIGTERM` exit status in
`result.json` when a signal lands while the process is waiting on
`.summary.lock`.
