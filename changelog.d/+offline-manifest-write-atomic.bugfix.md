`offline::write` publishes `manifest.json` through `util::write_atomic`
so offline manifests share stale temporary cleanup, symlink checks, and
parent directory sync.
