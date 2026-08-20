# AGENTS.md

## Versioning

- This project uses CalVer in the zero-padding-free `YYYY.M.#` format.
- `YYYY` is the release year, `M` is the release month, and `#` is the release
  sequence within that month.
- Start each month's release sequence at `1` and increment it for every
  subsequent release in the same month (for example, `2026.8.1`, then
  `2026.8.2`).
- Every release is tagged with its bare version (`2026.8.2`).

## Development builds

- A build is a release build only when `HEAD` carries the tag matching the
  package version and the tree is clean. Everything else is a development build.
- A development build's version is `YYYY.M.#-{short commit hash}`; `build.rs`
  derives it at build time, so never write a hash into `Cargo.toml`.
- `-dirty` is appended when the tree has uncommitted changes — for example,
  `2026.8.2-2879224` or `2026.8.2-2879224-dirty`.
- Sources without a git repository (a published crate, a release tarball) keep
  the bare CalVer version.
