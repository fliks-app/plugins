# Development

How to build, test and publish this plugin. What it does for a user is in
[README.md](../README.md); the port from Node is specified in [rust-port.md](rust-port.md); how the
plugin system itself works is in Fliks's `server/docs/spec/plugins.md`.

## Build

The plugin is a Rust WebAssembly component (`pluginApi` 3, target `wasm32-wasip2`) over
`fliks-plugin-sdk`.

```sh
rustup target add wasm32-wasip2
cargo build -p fk-plugin-download --release --target wasm32-wasip2   # -> target/wasm32-wasip2/release/fk_plugin_download.wasm
cargo xtask package plugins/download --out dist   # verifies the component, writes dist/fliks.download-<version>.fkplugin
```

`plugin.template.json` is the manifest without `version` and `files`; packaging adds both. A
route, a permission, a job or a settings field is declared there and nowhere else
(`tests/vectors.rs` pins the routes and jobs against the code).

## Kinds, units and history

Video kinds (`movie`, `series`) are acquired; a target of any other kind is refused by the routes
and skipped with a log by the sweeps (`grab::Video`). A `book` is searched only (`torznab::search_book`):
`t=book` with `author`/`title` when the caps advertise them, else `t=search`, under 7020 (ebook, 7000
if only the parent is advertised) for the `text` family and 3030 for `audio`, read from the caps'
category tree (`indexers.capsCategories`). Grabbing and importing a book wait for the host to serve
those families. Each language a book is wished in is its own target: the host calls, the pending-grab
check and the sweep's resume position work per (family, language), and a video call carries no language.
A target's `scope` is empty for a movie, one
season for a season pack and one episode for an episode.

`download_history` keys a grab by `mediaId`, `itemId` (a row of core `items`) and `seasonId` (the
group of a season pack or of an episode). The routes keep answering `episodeId`, which is the
`itemId` when the grab has a group and null otherwise, so a movie's own item is never an episode.

## Outbound requests and budgets

Every export that sends sets its guest budget on entry (`rt::budgets`, `src/lib.rs`), and every
request in `src/net.rs` ends by its own timeout or that budget, whichever comes first. The search
runs its indexers concurrently inside one `rt::block_on` with the `*_async` functions; the
blocking `net::send`/`get`/`get_following` must never be called inside it (`block_on` does not
nest). Log through the SDK's `info!`/`warn!`/`error!` (credentials are scrubbed) and pass URLs
through `fliks_plugin_sdk::log::redact_url`.

## Test

```sh
cargo build -p fk-plugin-download --release --target wasm32-wasip2
FLIKS_HARNESS_PG_URL=postgresql://user:pass@host:5432/postgres cargo test -p fk-plugin-download
```

`tests/vectors.rs` replays golden vectors recorded from the TypeScript plugin at `v0.12.1`
(`tests/vectors/gen/gen.ts`). `tests/harness.rs` installs the built component into the real
Fliks host on a scratch database it creates through `FLIKS_HARNESS_PG_URL` (a superuser URL;
`FLIKS_HARNESS_DB_PREFIX` names the database) and drops afterwards, with stub Torznab,
qBittorrent and Transmission servers. Without the URL those tests skip with a warning locally and fail under CI.

## Install it on a dev instance

Upload `dist/fliks.download-<version>.fkplugin` from **Settings → Advanced → Plugins** on a server
with `plugins.allow_unsigned`, or sign it with `cargo xtask package plugins/download --key <dev-key-file>`.

## Publish

Release-please opens a `download` release PR from the conventional commits that touch this folder.
Merging it tags `download-v<version>` and attaches the verified, unsigned
`fliks.download-<version>.fkplugin` (12 MiB budget) to the release. The catalog's `Package plugin`
workflow signs it and replaces the asset; see "WASM plugins" in the catalog README.
