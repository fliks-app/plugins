# Development

How to build, test and publish this plugin. What it does for a user is in
[README.md](../README.md); the port from Node is specified in [rust-port.md](rust-port.md); how the
plugin system itself works is in Fliks's `server/docs/spec/plugins.md`.

## Build

The plugin is a Rust WebAssembly component (`pluginApi` 2, target `wasm32-wasip2`) over
`fliks-plugin-sdk`.

```sh
rustup target add wasm32-wasip2
cargo build --release --target wasm32-wasip2   # -> target/wasm32-wasip2/release/fk_plugin_notify.wasm
```

`plugin.template.json` is the manifest without `version` and `files`; the release workflow adds
both. A route, a permission or a settings field is declared there and nowhere else.

## Test

```sh
cargo test --lib
```

The wasm exports of the SDK do not link on a native target, so run `--lib` rather than a bare
`cargo test`. The delivery logic runs against an in-memory `Env`, with no database and no host:
the retry ladder (6 attempts, 1/5/15/30/60 s), the conditional writes, the capacity drop at 200,
the lost sweep and the payloads. The SQL itself is not exercised natively.

The SQL and the scheduled retries run in `tests/harness.rs`, which installs the built component
under the real host (SDK `harness` feature) on a scratch database:

```sh
cargo build --release --target wasm32-wasip2
FLIKS_HARNESS_PG_URL=postgres://user:pass@host/postgres cargo test --test harness
```

The URL is a superuser one; the harness creates and drops its own databases
(`FLIKS_HARNESS_DB_PREFIX`, default `fliks_harness_`). Without it the tests skip locally and fail in CI.

## Package

```sh
cargo xtask package --max-mib 12 --out dist   # -> dist/fliks.notify-<version>.fkplugin
```

Builds the component and writes the unsigned archive (`plugin.json` from the template plus
`version` and `files`, `plugin.wasm`, `logo.svg`), then checks the component's world, the host's
archive rules and the size budget.

## Install it on a dev instance

Install `dist/fliks.notify-<version>.fkplugin` with `plugins.allow_unsigned` on, or pass
`--key <hex seed file>` to `cargo xtask package` to sign it with a dev key.

## Publish

Pushing a tag `v<version>` (equal to the crate version) runs `.github/workflows/ci.yml`, whose
`verify` job runs `cargo xtask package` and whose `release` job creates a release with that one
archive. The catalog's `Package plugin` workflow signs it; see "WASM plugins" in the catalog README.
