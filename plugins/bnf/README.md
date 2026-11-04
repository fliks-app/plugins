# BnF

A book metadata provider for Fliks, backed by the [BnF general catalogue](https://catalogue.bnf.fr)
(SRU API, UNIMARC records). It answers the `bnf` namespace for the `book` kind: search by ISBN or
title and author, and an edition by ISBN or notice id (`cb...`). Every record is an edition.

Requests are anonymous and identified by a `Fliks-BnF/<version>` user agent. The host paces them at
one per second; the BnF publishes no limit, so lower it on 429 or 503. Records carry no cover.

Data is BnF open data under the Licence Ouverte (Etalab): credit "source: BnF, catalogue general".

Requires Fliks 6.0.0 or later.

## Build and test

```sh
cargo test -p fk-plugin-bnf --lib
cargo build -p fk-plugin-bnf --release --target wasm32-wasip2
cargo test -p fk-plugin-bnf --test package   # needs the release component
```

`--lib` runs the lookups against the recorded bodies in `plugins/bnf/tests/fixtures`
through a fake transport. The host harness is not used: a provider holds no database.
