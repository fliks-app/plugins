# TheTVDB

Movie and series metadata from [TheTVDB](https://thetvdb.com): search, details with artwork,
seasons and episodes with translations, people and credits, and id lookups from IMDB.

Fliks ships a default API key, so it works out of the box. To use your own, set **API key** in the
plugin's settings page; clearing it goes back to the Fliks key. The key is stored as a secret.

## Develop

```sh
rustup target add wasm32-wasip2
cargo test -p fk-plugin-tvdb --lib                                  # mappers on recorded bodies
cargo build -p fk-plugin-tvdb --release --target wasm32-wasip2
cargo test -p fk-plugin-tvdb --test package                         # needs the release component
cargo xtask package plugins/tvdb --out dist
```
