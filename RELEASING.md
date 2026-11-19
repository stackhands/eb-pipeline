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

`cog bump` does the mechanical part. `cog.toml` runs `tools/release-prepare`
before the bump commit; the script's header lists what it sets and what it
leaves to a person.

## Steps

1. Branch from `main`: `git switch -c chore/release-X.Y.Z origin/main`.
2. Edit, in a commit of its own, the paragraph of
   `docs/orgmode/howto/install.org` that says what the previous release
   lacks.
3. `cog bump --minor` (or `--patch`, `--major`, `--version X.Y.Z`). It runs
   the hook, commits `chore(version): X.Y.Z` and creates the tag on that
   commit. `cog bump --minor --dry-run` prints the version only.
4. Push the branch, open the pull request, and merge it with a merge commit
   once its checks pass, so the tagged commit is on `main`.
5. Sign the tag and push it:
   `git tag -s -f vX.Y.Z -m "eb-pipeline X.Y.Z" 'vX.Y.Z^{}' && git push origin vX.Y.Z`.
6. Check the draft release: two archives, their `.sha256` files,
   `sha256.sum`, the notes, and the attestations
   (`gh attestation verify FILE --repo stackhands/eb-pipeline`).
7. Publish the draft. Assets cannot change after that, so nothing is added
   to a published release.

A failed guard leaves no release. Fix the version or changelog, delete the
tag locally and on the remote, and tag again.
