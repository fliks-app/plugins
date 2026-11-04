# Project Gutenberg

A source of free ebooks for Fliks, backed by [Gutendex](https://gutendex.com), the JSON API of
Project Gutenberg. It does two things.

**Metadata provider.** It answers the `fliks.gutenberg:id` namespace for the `book` kind: `search`
and `work`. A Gutenberg book id is the identity; the host builds the book from the `work` record
(title, authors, language) and writes only that id.

**Adding a book.** *Free ebooks* in the acquisition menu searches Gutendex (title or author) and lists
the books that have an EPUB. *Add to library* creates the book (`media.resolveOrCreate`), then a
scheduled call downloads the EPUB, preferring EPUB 3 with images and falling back to the plain EPUB, into
the plugin data directory and hands it to the library (`library.ingest`, `move`, with the book's
language). The call is retried by the host if the site or the ingest fails for a while.

The plugin needs the scopes `media:read`, `media:create` and `ingest:write`. The routes are guarded by
the `books` permission of the plugin (`read` to search, `create` to add), which an admin has. When
several libraries accept books, set the library id in the plugin's settings page; with one, it is used.

Requests are anonymous, identified by a `Fliks-Gutenberg/<version>` user agent. The catalogue is
paced at one request per second by the host, and file downloads are two seconds apart. A book is
public domain in the United States; whether it is in yours is up to you.

Requires Fliks 6.0.0 or later.

## Build and test

```sh
cargo test -p fk-plugin-gutenberg --lib
cargo build -p fk-plugin-gutenberg --release --target wasm32-wasip2
cargo test -p fk-plugin-gutenberg --test package   # needs the release component
```

`--lib` runs the lookups and the download against the recorded bodies in
`plugins/gutenberg/tests/fixtures` through a fake transport, with no network.
