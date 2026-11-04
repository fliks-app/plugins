# TheXEM scene numbering

Scene to aired numbering of series, from [TheXEM](https://thexem.info). The plugin serves no work: it
answers the two scene calls of the `fliks.thexem:xem` provider for the `series` kind and the `tvdb`
origin. `scene-mapped-ids` lists the TVDB ids TheXEM maps (`/map/havemap?origin=tvdb`) and `scene-mapping`
gives one id's per-episode mapping (`/map/all?origin=tvdb&id=<id>`). An id TheXEM does not map answers
none; an entry missing its scene or TVDB side is skipped. The core stores the mappings and translates
the numbers of release and file names (`server/docs/spec/scene-numbering.md`).
Without a provider declaring `scene`, Fliks reads every release and file number as aired numbering.

The base URL is the plugin's own `baseUrl` setting, TheXEM when empty. The host paces the provider at one
request a second.

## Build and test

```sh
cargo test -p fk-plugin-thexem --lib
cargo build -p fk-plugin-thexem --release --target wasm32-wasip2
cargo test -p fk-plugin-thexem --test package   # needs the release component
```

`--lib` runs the parsing against the recorded TheXEM bodies in `plugins/thexem/tests/fixtures`; no network.
