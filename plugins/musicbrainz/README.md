# MusicBrainz

Album, release and artist metadata from [MusicBrainz](https://musicbrainz.org) for the `album` kind, as
the provider of the core namespace `musicbrainz` (`server/docs/spec/music.md` rules 18 to 24).

- `search` takes free text, a MusicBrainz id or a musicbrainz.org URL, the artist as `author` or
  `author_id`, and re-ranks by exact title, primary type (album, EP, single), release count and, when
  ListenBrainz answers, popularity.
- `work` is a release group, `children` its releases, `edition` one release with its tracklist, the
  performer, conductor, composer and lyricist credits and the works a track plays.
- `person` is an artist, `credits` its discography (`{albums}`), `findBy` resolves a Spotify or Deezer
  album id (through the URL relations) and an ISRC.
- `work` carries the group's `genres`, most voted first; the annotation (editor notes) is not an overview. `edition` carries each track's ListenBrainz `popularity`, from one batch call per release.
- Covers are the Cover Art Archive's `front-500` of the group and of the release. A merged id is
  answered under its new id: the record's `externalId` differs from the id asked.

Settings: `baseUrl` (a mirror, on your network or not; MusicBrainz when empty) and the secret `listenbrainzToken`, sent to
ListenBrainz when set. The host paces the provider at one request a second.

## Build and test

```sh
cargo test -p fk-plugin-musicbrainz --lib
cargo build -p fk-plugin-musicbrainz --release --target wasm32-wasip2
cargo test -p fk-plugin-musicbrainz --test package   # needs the release component
```

`--lib` runs the mapping against the bodies in `plugins/musicbrainz/tests/fixtures`; no network.
