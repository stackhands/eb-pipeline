# Releasing

A release is a signed tag `vX.Y.Z` on `main`. The `release` workflow runs
on the tag and:

1. refuses the tag unless it equals `v` plus the `version` in `Cargo.toml`
   and `CHANGELOG.md` has a section for that version;
2. builds static `x86_64-unknown-linux-musl` and `aarch64-unknown-linux-musl`
   `tar.xz` archives with dist, each on its own architecture;
   each archive must contain every binary named in
   `.github/release-binaries.txt`, and that list must match the binaries this
   workspace builds;
3. checks each `<archive>.sha256` from dist and writes `sha256.sum` over
   both archives and `source.tar.gz`;
4. attaches a build provenance attestation to every archive;
5. opens a draft release with the archives, the checksums and the
   changelog section as notes.

Publishing the draft stays manual.

## Checklist

1. Set `version` in `Cargo.toml` to the new version and refresh the lock
   file (`cargo update -p eb-pipeline`, on the build host).
2. Run `towncrier build --version X.Y.Z`. It writes the fragments in
   `changelog.d/` into `CHANGELOG.md` under `## X.Y.Z (YYYY-MM-DD)` and
   removes them. The text under that heading becomes the release notes.
3. CI on `main` is green.
4. Commit: `git commit -am "chore: release X.Y.Z"`.
5. Tag and push: `git tag -s vX.Y.Z -m vX.Y.Z && git push origin main vX.Y.Z`.
6. Check the draft release: two archives, their `.sha256` files,
   `sha256.sum`, the notes, and the attestations
   (`gh attestation verify FILE --repo stackhands/eb-pipeline`).
7. Publish the draft. Assets cannot change after that, so nothing is added
   to a published release.

A failed guard leaves no release. Fix the version or changelog, delete the
tag locally and on the remote, and tag again.
