`util::write_atomic` syncs the parent directory after renaming the
temporary file into place. It ignores only `EINVAL`, `ENOTSUP`, and
`EBADF` from directory sync and returns all other errors. Bare output
file names resolve against the current directory through the shared
parent helper.
