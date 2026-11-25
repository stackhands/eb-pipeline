`eb-pipeline watch` now writes `<target>.json` through `util::write_atomic`
instead of a predictable `.<target>.json.tmp` path, avoiding writes through
pre-existing temporary symlinks.
