# Contributing

Human-written and AI-assisted contributions are both welcome, under the same
rules. Open an issue or a pull request at
<https://github.com/porthole-dev/obscura>.

## Commits

- **Sign off every commit** (`git commit -s`). `Signed-off-by:` is your
  [Developer Certificate of Origin](https://developercertificate.org/): it
  certifies that you have the right to submit the change. Only a person can
  give it, never an AI tool. CI rejects a pull request with a commit whose
  sign-off does not match its author.
- **Disclose AI assistance.** If an AI tool helped write a change, add an
  `Assisted-by:` trailer naming the tool (for example `Assisted-by: Claude`)
  before your sign-off. A change written without one needs no trailer. See
  [AI.md](AI.md). Never list an AI tool as `Co-authored-by:` or in
  `Signed-off-by:`; CI rejects both, and tool boilerplate such as
  "Generated with ..." lines.
- One logical change per commit, with a subject that says what changes and a
  body that says why.

## Checks

```sh
cargo fmt --check
cargo clippy --release --locked -- -D warnings
cargo test --release --locked --features pipewire-controls
build-aux/preview/check.sh          # UI checks against a fake camera, headless
```

CI runs these and a meson build (`.github/workflows/ci.yml`), plus the
trailer and sign-off check in `.github/scripts/commit-check.sh`.

## Releases

The version is in `Cargo.toml` (with `Cargo.lock`), `meson.build` and the
`<releases>` list of the metainfo file. To release, update them in one
commit, then tag it `v<version>` on `main` and push the tag. GitHub serves the release tarball at
`https://github.com/porthole-dev/obscura/archive/refs/tags/v<version>.tar.gz`,
which is the source the postmarketOS package fetches.
