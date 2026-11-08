# Fanart.tv

Artist images and album covers from [Fanart.tv](https://fanart.tv), as the provider of the core
namespace `fanarttv` for the `album` kind (`server/docs/spec/music.md` rules 25 and 26). It is keyed by
MusicBrainz ids: `findBy` maps a `musicbrainz` id to the same id, `person` answers the thumb of an
artist as `avatarUrl`, `work` the most liked album cover of a release group as `posterUrl`.

The project key comes from the host's defaults (`fliks:apiKey`); the admin may type their own in
`apiKey`, and an optional personal key in `clientKey`. Without any key the provider is unavailable.

## Build and test

```sh
cargo test -p fk-plugin-fanarttv --lib
cargo build -p fk-plugin-fanarttv --release --target wasm32-wasip2
cargo test -p fk-plugin-fanarttv --test package   # needs the release component
```
