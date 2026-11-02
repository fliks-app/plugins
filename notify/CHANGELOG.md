# Changelog

## [2.0.0](https://github.com/fliks-app/plugins/compare/notify-v1.0.0...notify-v2.0.0) (2026-11-02)


### ⚠ BREAKING CHANGES

* requires pluginApi 3 and fliks 6. Webhook payloads of media.imported, media.removed, request.created and request.approved carry externalIds instead of tmdbId; media.files.imported and acquisition.grabbed carry a family.

### Features

* **book-providers:** add the shared book provider crate ([#5](https://github.com/fliks-app/plugins/issues/5)) ([469639e](https://github.com/fliks-app/plugins/commit/469639ed09743e67006b8596442876d20220fe1a))
* port to pluginApi 3 ([#7](https://github.com/fliks-app/plugins/issues/7)) ([1a519eb](https://github.com/fliks-app/plugins/commit/1a519eb919190ed2c180cedf12c7719886b25cc7))
