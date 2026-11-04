# Metron

Comic metadata from [Metron](https://metron.cloud), a community comic database. It answers the
`fliks.metron:metron` namespace for the `comic` kind: search series, a series as a work (title,
publisher, start year, volume, description, cover of its first issue) and its issues (number as
printed, decimal sort key, `issue` or `annual` kind, cover and store dates, cover).

Fliks ships no account: set **API token** in the plugin's settings page (create one in the API Tokens
section of your own Metron account page; it is stored as a secret and sent as `Authorization: Bearer`).
Until it is set the provider stays inactive.

Metron's data is licensed CC BY-SA 4.0; the settings page carries the attribution.

Limits are per account: 20 requests a minute and 5,000 a day, read by the host from `ratePerSec` 0.33
and a 5,000 daily quota. A 429 maps to `rate-limited` with its `Retry-After`; Metron's day is a
sliding window, so a long wait is returned as is and the host decides when to retry.

A volume fetch costs 2 requests, an issue list one per 100 issues plus one.

## Build and test

```sh
cargo test -p fk-plugin-metron --lib
cargo build -p fk-plugin-metron --release --target wasm32-wasip2
cargo test -p fk-plugin-metron --test package   # needs the release component
```

`--lib` runs the lookups against the anonymised bodies in `plugins/metron/tests/fixtures` through a
fake transport; no network.
