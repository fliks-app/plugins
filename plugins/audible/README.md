# Audible

A book metadata provider for Fliks, backed by the unofficial Audible catalog
(`api.audible.<tld>/1.0/catalog/products`). It answers the `audible` namespace for the `book`
kind: search by title, author or keywords, and an edition by ASIN. Every record is one audio edition.

The storefront comes from the metadata language and region (`fr` gives `api.audible.fr`, `en-GB`
`api.audible.co.uk`, `us` when nothing matches). A book exists under the same ASIN in several
storefronts, but a storefront does not return another's books: an unknown ASIN answers a skeleton,
read as not found.

Requests are anonymous, identified by a `Fliks-Audible/<version>` user agent. The API is
undocumented and has no published limit: the host paces them at one per second, lowered on 429.
This is not an official API and is outside any published terms; covers and text are Audible's.

Requires Fliks 6.0.0 or later.

## Build and test

```sh
cargo test -p fk-plugin-audible --lib
cargo build -p fk-plugin-audible --release --target wasm32-wasip2
cargo test -p fk-plugin-audible --test package   # needs the release component
```

`--lib` runs the lookups against the recorded bodies in `plugins/audible/tests/fixtures`
through a fake transport. The host harness is not used: a provider holds no database.
