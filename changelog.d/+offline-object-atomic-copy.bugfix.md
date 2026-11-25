`offline::write` publishes content-addressed objects through a shared
`util::copy_atomic_checked` helper. Objects share temporary naming,
stale local cleanup, file sync, atomic rename, and parent directory
sync with `util::write_atomic`.
