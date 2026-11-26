`eb-pipeline --help` and `--version`, eb-cvmfs, eb-junit, eb-retry-list,
eb-shard-weights and eb-test-map no longer panic with exit 101 when a
diagnostic cannot be written because stderr is closed or full. They keep their
documented exit status, including when stdout cannot be written either.
