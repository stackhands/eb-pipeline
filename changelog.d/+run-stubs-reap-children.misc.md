On a failed timeout or check, the `run_stubs` tests kill and reap the run they started. Their waiting stubs end once the test directory is gone.
