Stop eb-cvmfs, eb-junit, eb-retry-list, eb-shard-weights and eb-test-map from
panicking with exit 101 when a diagnostic cannot be written because stderr is
closed or full. They now report on stderr on a best-effort basis and keep their
documented exit status, including when stdout cannot be written either.
