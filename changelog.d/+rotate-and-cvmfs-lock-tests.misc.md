The rotate-overlap test's keep-logs stub waits only until the sibling target
enters rotation or finishes, with a marker file per target instead of a shared
temporary, and the eb-cvmfs shared-metrics lock test releases its first run
before asserting, so a failed wait no longer leaves the child blocked.
