`eb-retry-list --easystack` now writes through `util::write_atomic`, so a
pre-existing symlink at the output path is refused instead of being
followed.
