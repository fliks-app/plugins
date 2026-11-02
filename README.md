# Fliks plugins

The official [Fliks](https://github.com/fliks-app/fliks) plugins, one folder each. Install them
from the plugin catalogue in **Settings → Advanced → Plugins**.

| Folder | Plugin | License |
|---|---|---|
| [`plugins/download`](plugins/download/README.md) | `fliks.download`: indexer search, download clients and the grab pipeline | AGPL-3.0-or-later |
| [`plugins/notify`](plugins/notify/README.md) | `fliks.notify`: notification delivery | MIT |
| [`plugins/openlibrary`](plugins/openlibrary/README.md) | `fliks.openlibrary`: book metadata from OpenLibrary | AGPL-3.0-or-later |
| [`plugins/tmdb`](plugins/tmdb/README.md) | `fliks.tmdb`: movie and series metadata from TMDB | AGPL-3.0-or-later |
| [`plugins/bnf`](plugins/bnf/README.md) | `fliks.bnf`: book metadata from the BnF catalogue | AGPL-3.0-or-later |
| [`plugins/googlebooks`](plugins/googlebooks/README.md) | `fliks.googlebooks`: book metadata from Google Books | AGPL-3.0-or-later |
| [`plugins/tvdb`](plugins/tvdb/README.md) | `fliks.tvdb`: movie and series metadata from TheTVDB | AGPL-3.0-or-later |

[`crates/provider-common`](crates/provider-common) holds what several provider plugins share (language
codes, the HTTP status mapping, the book model and ISBN); it is a library, not a plugin. Each provider
keeps its own parser and golden fixtures (`src/parse.rs`, `tests/fixtures`).

Each folder holds its own `LICENSE`, `plugin.template.json`, `migrations/`, `tests/` and `docs/`.
`xtask/` is the shared packaging CLI over the SDK build tooling, and the root `Cargo.toml` pins one
`fliks-plugin-sdk` rev for every plugin.

## Build and test

```sh
rustup target add wasm32-wasip2
cargo xtask package plugins/download --out dist   # verifies the component, writes dist/fliks.download-<version>.fkplugin
cargo test -p fk-plugin-download
```

Per-plugin details are in `<folder>/docs/development.md`.

## Releases

Every folder has its own version, release PR and tag `<folder>-v<version>` (for example
`download-v1.2.0`; `plugins/openlibrary` tags `openlibrary-v<version>`), managed by release-please from conventional commits; commits touching only
another folder do not bump it. Merging a release PR attaches the unsigned `.fkplugin` to the
release; the catalog's `Package plugin` workflow then signs it.

Tags `v<version>` in the former `fk-plugin-download` and `fk-plugin-notify` repositories are
`download-v<version>` and `notify-v<version>` here, with the same history.
