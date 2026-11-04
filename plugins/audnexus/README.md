# Audnexus

A book metadata provider for Fliks, backed by Audnexus
(`api.audnex.us`), a community mirror of Audible data. It answers the `audnexus` namespace for the
`book` kind: an edition by ASIN, and its chapters (`fetch-chapters`, ASIN only). There is no search.

The `region` of the request comes from the metadata language and region (`fr` gives `region=fr`,
`us` when nothing matches). An ASIN absent from that region answers 404, read as not found.

Requests are anonymous, identified by a `Fliks-Audnexus/<version>` user agent. The public instance
allows 300 requests per minute and sends `x-ratelimit-*` headers; the host paces at one per second.
A 429 maps to `rate-limited` with its `Retry-After`, else the `retryAfterSeconds` of the body.
Chapters never expire from the host cache. The data is Audible's and the instance publishes no terms.

Requires Fliks 6.0.0 or later.

## Build and test

```sh
cargo test -p fk-plugin-audnexus --lib
cargo build -p fk-plugin-audnexus --release --target wasm32-wasip2
cargo test -p fk-plugin-audnexus --test package   # needs the release component
```

`--lib` runs the lookups against the recorded bodies in `plugins/audnexus/tests/fixtures`
through a fake transport. The host harness is not used: a provider holds no database.
