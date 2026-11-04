# ComicVine

Comic metadata from [ComicVine](https://comicvine.gamespot.com). It answers the
`fliks.comicvine:comicvine` namespace for the `comic` kind: search volumes, a volume as a work (title,
publisher, start year, description, cover) and its issues (number as printed, decimal sort key, cover
and store dates, title, cover). A volume whose name contains "Annual" gives `annual` issues.

**Non-commercial use only.** ComicVine's terms limit its API and data to non-commercial use and ask
for a link back to ComicVine wherever its data shows: <https://comicvine.gamespot.com>. The plugin
carries that link in its manifest and its settings page.

Fliks ships no key: set **API key** (from comicvine.gamespot.com/api) in the plugin's settings page; it
is stored as a secret. Until it is set the provider stays inactive.

ComicVine allows 200 requests per resource and hour; the host paces at 0.05 requests a second, which
stays under it for each resource. HTTP 420 maps to `rate-limited`. A volume has no volume number there.

## Build and test

```sh
cargo test -p fk-plugin-comicvine --lib
cargo build -p fk-plugin-comicvine --release --target wasm32-wasip2
cargo test -p fk-plugin-comicvine --test package   # needs the release component
```

`--lib` runs the lookups against the anonymised bodies in `plugins/comicvine/tests/fixtures` through
a fake transport; no network.
