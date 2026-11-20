`eb-pipeline man --out-dir` now writes `eb-pipeline.1` through
`util::write_atomic`, preventing writes through a pre-existing output
symlink.
