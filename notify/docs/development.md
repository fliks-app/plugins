# Development

How to build, test and publish this plugin. What it does for a user is in
[README.md](../README.md); how the plugin system itself works — tiers, manifest, contract, trust —
is in [Fliks's `docs/plugins.md`](https://github.com/fliks-app/fliks/blob/main/docs/plugins.md).

## Build

```sh
npm install
npm run typecheck
npm test
npm run build         # -> dist/plugin.js, dist/plugin.json, dist/logo.svg
node --check dist/plugin.js
npm run package       # -> dist/fliks-notify.fkplugin
```

`dist/plugin.json` is generated from `scripts/manifest-template.ts` and never hand-edited: a route,
a permission or a settings field is declared there and nowhere else.

## Test

```sh
npm test
```

No database and no core process: the delivery queue, the backoff ladder and the address guards are
all exercised against local stubs.

The bounds the tests pin, because they are the whole safety story of a retrying sender: a queue of
200 pending deliveries dropping oldest-first, a 200-entry delivery log, six attempts per delivery
over roughly four minutes, one delivery in flight at a time, and a 5 s per-request timeout that
never blocks the health reply.

## Install it on a dev instance

```sh
npm run build && npm run package
```

Upload `dist/fliks-notify.fkplugin` from **Settings → Advanced → Plugins**. A `process` plugin must
be signed, so a locally built archive needs its id in the server's `FLIKS_UNSIGNED_PLUGINS`.

## Publish

Pushing here publishes nothing. Copy the built `plugin.json`, `plugin.js` and `logo.svg` into
`plugins/fliks.notify/src/` in the catalogue repository, add a `versions/<version>.json` entry, then
run its **Package plugin** workflow for this id. A published version's bytes are pinned by their
recorded hash, so any change ships as a new version.
