`util::write_atomic` now tags temp files with host and boot identity and
reaps only stale local temp siblings, preventing cross-host deletion on
shared filesystems while cleaning up crash leftovers on rerun.
