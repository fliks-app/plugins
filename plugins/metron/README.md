# Metron

Comic metadata from [Metron](https://metron.cloud), a community comic database. It answers the
`fliks.metron:metron` namespace for the `comic` kind: search series, a series as a work (title,
publisher, start year, volume, description, cover of its first issue) and its issues (number as
printed, decimal sort key, `issue` or `annual` kind, cover and store dates, cover).

Fliks ships no account: set **Username** and **Password** of your own Metron account in the plugin's
settings page (the password is stored as a secret). Until both are set the provider stays inactive.
Metron authenticates the API with HTTP Basic.

Limits are per account: 20 requests a minute and 5,000 a day. The host paces at 0.33 requests a second
with a 5,000 daily quota. A 429 maps to `rate-limited` with its `Retry-After`, else `X-RateLimit-Reset`
(seconds or Unix time); a wait of an hour or more marks the day's quota spent.

A volume fetch costs 2 requests, an issue list one per 30 issues plus one.

## Build and test

```sh
cargo test -p fk-plugin-metron --lib
cargo build -p fk-plugin-metron --release --target wasm32-wasip2
cargo test -p fk-plugin-metron --test package   # needs the release component
```

`--lib` runs the lookups against the anonymised bodies in `plugins/metron/tests/fixtures` through a
fake transport; no network.
