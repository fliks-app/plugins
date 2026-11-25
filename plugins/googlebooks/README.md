# Google Books

A book metadata provider for Fliks, backed by the [Google Books API](https://developers.google.com/books).
It answers the `googlebooks` namespace for the `book` kind: search by ISBN, title and
author, and an edition by ISBN or volume id. Every record is an edition.

It needs a Google Cloud API key with the Books API enabled, set in the plugin settings: Google gives
keyless requests a daily quota of zero. Until the key is set the host skips the provider. A key Google
refuses (invalid, blocked, API disabled, zero quota) maps to `unauthorized`, which sets the provider
aside until its settings change; a spent daily quota maps to `rate-limited`. Requests carry a
`Fliks-GoogleBooks/<version>` user agent and the host paces them at one per second. Covers come from
`books.google.com`.

Requires Fliks 7.0.0 or later.

## Build and test

```sh
cargo test -p fk-plugin-googlebooks --lib
cargo build -p fk-plugin-googlebooks --release --target wasm32-wasip2
cargo test -p fk-plugin-googlebooks --test package   # needs the release component
```

`--lib` runs the lookups against the recorded bodies in `plugins/googlebooks/tests/fixtures`
through a fake transport. The host harness is not used: a provider holds no database.

## Compatibility

Built against the provider interface of the Fliks core range declared in its manifest.
