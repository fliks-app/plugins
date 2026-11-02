# Fliks plugins

The official [Fliks](https://github.com/fliks-app/fliks) plugins, one folder each. Install them
from the plugin catalogue in **Settings → Advanced → Plugins**.

| Folder | Plugin | License |
|---|---|---|
| [`download`](download/README.md) | `fliks.download`: indexer search, download clients and the grab pipeline | AGPL-3.0-or-later |
| [`notify`](notify/README.md) | `fliks.notify`: notification delivery | MIT |

[`crates/book-providers`](crates/book-providers) holds the book provider request builders and
response parsers shared by the book provider plugins; it is a library, not a plugin.

Each folder holds its own `LICENSE`, `plugin.template.json`, `migrations/`, `tests/` and `docs/`.
`xtask/` is the shared packaging CLI over the SDK build tooling, and the root `Cargo.toml` pins one
`fliks-plugin-sdk` rev for every plugin.

## Build and test

```sh
rustup target add wasm32-wasip2
cargo xtask package download --out dist   # verifies the component, writes dist/fliks.download-<version>.fkplugin
cargo test -p fk-plugin-download
```

Per-plugin details are in `<folder>/docs/development.md`.

## Releases

Every folder has its own version, release PR and tag `<folder>-v<version>` (for example
`download-v1.2.0`), managed by release-please from conventional commits; commits touching only
another folder do not bump it. Merging a release PR attaches the unsigned `.fkplugin` to the
release; the catalog's `Package plugin` workflow then signs it.

Tags `v<version>` in the former `fk-plugin-download` and `fk-plugin-notify` repositories are
`download-v<version>` and `notify-v<version>` here, with the same history.
