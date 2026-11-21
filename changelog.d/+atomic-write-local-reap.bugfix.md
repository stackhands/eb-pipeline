`util::write_atomic` now tags temp files with host and boot identity and
reaps only stale local temp siblings, preventing cross-host deletion on
shared filesystems while cleaning up crash leftovers on rerun. Sibling
names that merely resemble a temp file, such as `.cpu.json.tmp`, no longer
panic the scan and are left alone, as are names with a non-positive pid or
non-UTF-8 bytes. A bare file name is scanned in the current directory, an
empty `HOSTNAME` falls back to the kernel host name, a boot id that needs
rewriting disables reaping, and the parent directory is synced after the
rename.
