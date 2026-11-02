# OpenLibrary

A book metadata provider for Fliks, backed by [OpenLibrary](https://openlibrary.org). It answers
the `openlibrary` namespace for the `book` kind: search, a work and its editions, an edition by
ISBN or edition id, and the work an ISBN belongs to.

Requests are anonymous and identified by a `Fliks-OpenLibrary/<version>` user agent. The host paces
them at one per second. Covers come from `covers.openlibrary.org`.

## Build and test

```sh
cargo test -p fk-plugin-openlibrary --lib
cargo build -p fk-plugin-openlibrary --release --target wasm32-wasip2
cargo test -p fk-plugin-openlibrary --test package   # needs the release component
```

`--lib` runs the lookups against the recorded bodies in `plugins/openlibrary/tests/fixtures`
through a fake transport. The host harness is not used: it needs a scratch Postgres, and a provider
holds no database.
