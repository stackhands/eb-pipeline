`util::write_atomic` tags temporary files with kernel host, machine,
boot, process ID namespace, process ID, and process start time identity.
It reaps a stale temporary file once the writer exits on the same boot.
A changed start time counts as an exit. The file must come from this
host and namespace. After a reboot, it reaps entries only when host and
machine ID match. It skips malformed names, non-UTF-8 entries,
non-positive process IDs, and files whose identity it cannot read.
