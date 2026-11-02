# Google Books

A book metadata provider for Fliks, backed by the [Google Books API](https://developers.google.com/books)
without a key. It answers the `googlebooks` namespace for the `book` kind: search by ISBN, title and
author, and an edition by ISBN or volume id. Every record is an edition.

Requests are anonymous and identified by a `Fliks-GoogleBooks/<version>` user agent. The host paces
them at one per second. Google documents no keyless quota and has answered every keyless request with
a 429 for a zero daily quota (observed 2026-11-01); that, or a quota error body, maps to
`rate-limited` and the provider is skipped. Covers come from `books.google.com`.

## Build and test

```sh
cargo test -p fk-plugin-googlebooks --lib
cargo build -p fk-plugin-googlebooks --release --target wasm32-wasip2
cargo test -p fk-plugin-googlebooks --test package   # needs the release component
```

`--lib` runs the lookups against the recorded bodies in `plugins/googlebooks/tests/fixtures`
through a fake transport. The host harness is not used: a provider holds no database.
