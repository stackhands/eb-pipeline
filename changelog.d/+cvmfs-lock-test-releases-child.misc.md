Release the first eb-cvmfs invocation in the shared-metrics lock test before its
assertions run, so a failed wait no longer leaves the child blocked.
