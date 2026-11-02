# Changelog

## [2.0.0](https://github.com/fliks-app/plugins/compare/notify-v1.0.0...notify-v2.0.0) (2026-11-02)


### ⚠ BREAKING CHANGES

* **notify:** requires pluginApi 3 and fliks 6. Webhook payloads of media.imported, media.removed, request.created and request.approved carry externalIds instead of tmdbId; media.files.imported and acquisition.grabbed carry a family.

### Features

* **notify:** port to pluginApi 3 ([23fbba6](https://github.com/fliks-app/plugins/commit/23fbba6d92b25a375a2fca493d9c426807c6ce54))

Releases before the move to `plugins/notify` are on the [releases page](https://github.com/fliks-app/plugins/releases?q=notify-v).
