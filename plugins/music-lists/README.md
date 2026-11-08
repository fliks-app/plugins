# Music lists

`fliks.musiclists` follows the albums and tracks of lists kept elsewhere: ListenBrainz, Last.fm,
Spotify, Deezer and public MusicBrainz collections. A job reads each enabled list every six hours
(`SyncLists`, also triggerable) and acts on the entries it has not seen, through the host:

- **albums**: `media-resolve-or-create` with the album's MusicBrainz release group, Spotify or Deezer id;
- **tracks**: `music-want-recording` with an ISRC (the album is created, only that track monitored) or a
  recording id (a track the library already holds);
- **playlist**: the tracks resolved that way, mirrored with `playlist-sync` into a playlist named like the
  list, owned by the user who saved it. A track not yet in the library is wanted too.

The plugin never resolves a title itself. Entries without an id the host can resolve (a Last.fm track with
no recording id, a Spotify track with neither ISRC nor album) are skipped.

| Source | Lists | Credentials |
|---|---|---|
| ListenBrainz | a playlist, playlists made for you, top albums, top tracks, loved tracks | user name, optional token |
| Last.fm | loved tracks, top tracks (only those with a recording id) | user name, API key |
| Spotify | a playlist, saved albums, saved tracks | your own client id and secret |
| Deezer | a public playlist, favourite albums | none |
| MusicBrainz | a public collection of release groups or recordings | none |

Spotify: register `https://YOUR-FLIKS/api/plugins/fliks.musiclists/spotify/callback` as a redirect URI of
your Spotify app, save the list, then open `https://YOUR-FLIKS/api/plugins/fliks.musiclists/spotify/LIST-ID/authorize` once.
Artists are not followed: the host has no call to follow one.

Scopes: `media:read`, `media:create`, `playlists:write`. A list's credentials are plugin secrets.

## Build and test

```sh
cargo test -p fk-plugin-music-lists --lib
cargo build -p fk-plugin-music-lists --release --target wasm32-wasip2
cargo test -p fk-plugin-music-lists --test package   # needs the release component
```
