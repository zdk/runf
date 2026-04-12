# Contributing

Thanks for your interest in lowfat.

## Build & test

```sh
cargo build
cargo test --workspace
```

The workspace has four crates: `lowfat-core`, `lowfat-plugin`, `lowfat-runner`, and the `lowfat` CLI.

## Pull requests

- Keep PRs small and focused on one change.
- Add a test for new behaviour; unit tests live alongside the code.

## Writing a plugin

See [docs/PLUGINS.md](docs/PLUGINS.md). Bundled plugins live under `plugins/`.

## Releases (maintainers)

Bump `version` in `Cargo.toml`, tag `vX.Y.Z`, push — the release workflow handles the rest.
