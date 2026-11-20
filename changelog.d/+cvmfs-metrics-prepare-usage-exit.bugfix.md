`eb-cvmfs` exits with status `2`, not `1`, when the metrics file cannot be
prepared before any repository command starts. Status `1` now means only that
the repository operation succeeded and the metrics write failed.
