# LRCLIB

Synced and plain lyrics from [LRCLIB](https://lrclib.net), as the lyrics provider of the core namespace
`lrclib` for the `album` kind. `fetch-lyrics` calls `GET /api/get` with the title, artist, album and
duration (LRCLIB matches within 2 s) and answers the synced LRC when there is one, else the plain text;
an instrumental record is an empty document, a 404 a miss.

No account or key: the plugin sends the registered client-side application id of Fliks in
`Lrclib-App-Id` and a `Fliks/<version>` user agent. A 429 or 503 is a rate limit that carries its
`Retry-After`.

## Build and test

```sh
cargo test -p fk-plugin-lrclib --lib
cargo build -p fk-plugin-lrclib --release --target wasm32-wasip2
cargo test -p fk-plugin-lrclib --test package   # needs the release component
```
