# LibriVox

A source of free audiobooks for Fliks, backed by the [LibriVox API](https://librivox.org/api/info).
It does two things.

**Metadata provider.** It answers the `fliks.librivox:id` namespace for the `book` kind: `search` and
`work`. A LibriVox project id is the identity; the `work` record carries the title, authors, readers
(as narrators), language, year and length, and the host writes only that id.

**Adding an audiobook.** *Free audiobooks* in the acquisition menu searches LibriVox by title (then by
author when no title matches) and lists the recordings. *Add to library* creates the book
(`media.resolveOrCreate`), then a scheduled call downloads the recording's sections one by one, as MP3,
into the plugin data directory and hands them to the library as one audio version (`library.ingest`,
`move`, with the recording's language). A recording is far larger than one call may run, so the call
keeps the sections already downloaded, schedules itself again when its time runs out, and ingests once
every section is there. The host retries a failed call.

The plugin needs the scopes `media:read`, `media:create` and `ingest:write`. The routes are guarded by
the `books` permission of the plugin (`read` to search, `create` to add), which an admin has. When
several libraries accept books, set the library id in the plugin's settings page; with one, it is used.

Requests are anonymous, identified by a `Fliks-LibriVox/<version>` user agent. The catalogue is paced at
one request per second by the host, and section downloads are one second apart. LibriVox recordings are
public domain; the MP3 files are served by the Internet Archive.

Requires Fliks 6.0.0 or later.

## Build and test

```sh
cargo test -p fk-plugin-librivox --lib
cargo build -p fk-plugin-librivox --release --target wasm32-wasip2
cargo test -p fk-plugin-librivox --test package   # needs the release component
```

`--lib` runs the lookups and the download against the recorded bodies in
`plugins/librivox/tests/fixtures` through a fake transport, with no network.
