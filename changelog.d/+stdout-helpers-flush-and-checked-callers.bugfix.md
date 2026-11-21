Flush standard output inside `util::write_stdout`, so a buffered write that the
consumer cannot take is reported instead of lost, and route the help and
version output, `from-commit` and `gap` through `util::write_stdout_checked`
so each reports the same `cannot write stdout: ...` diagnostic.
