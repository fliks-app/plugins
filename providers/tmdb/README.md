# TMDB

Movie and series metadata from [The Movie Database](https://www.themoviedb.org): search, details,
seasons and episodes, people and credits, id lookups from IMDB and TVDB, and the trending,
popular, upcoming, genre and discover lists.

Fliks ships a default API key, so it works out of the box. To use your own, set **API key** in the
plugin's settings page; clearing it goes back to the Fliks key. The key is stored as a secret.

## Develop

```sh
rustup target add wasm32-wasip2
cargo test -p fk-plugin-tmdb --lib                                  # mappers on recorded bodies
cargo build -p fk-plugin-tmdb --release --target wasm32-wasip2
cargo test -p fk-plugin-tmdb --test package                         # needs the release component
cargo xtask package providers/tmdb --out dist
```
