# Changelog

## [3.0.0](https://github.com/fliks-app/plugins/compare/notify-v2.0.1...notify-v3.0.0) (2026-11-19)


### ⚠ BREAKING CHANGES

* every plugin requires fliks >=7.0.0 <8.0.0 and no longer loads on core 6.

### Features

* require fliks &gt;=7.0.0 &lt;8.0.0 and build against the 7.0.0 plugin WIT ([#159](https://github.com/fliks-app/plugins/issues/159)) ([33c68b0](https://github.com/fliks-app/plugins/commit/33c68b0c17d00aa347f5891b39c4116b3e09eeb6))

## [2.0.1](https://github.com/fliks-app/plugins/compare/notify-v2.0.0...notify-v2.0.1) (2026-11-16)


### Bug Fixes

* rebuild every plugin against the new provider interface ([#135](https://github.com/fliks-app/plugins/issues/135)) ([0cfe1ac](https://github.com/fliks-app/plugins/commit/0cfe1acc9424c435f3b752564d2f1b6dda9bb950))

## [2.0.0](https://github.com/fliks-app/plugins/compare/notify-v1.0.0...notify-v2.0.0) (2026-11-02)


### ⚠ BREAKING CHANGES

* **notify:** requires pluginApi 3 and fliks 6. Webhook payloads of media.imported, media.removed, request.created and request.approved carry externalIds instead of tmdbId; media.files.imported and acquisition.grabbed carry a family.

### Features

* **notify:** port to pluginApi 3 ([23fbba6](https://github.com/fliks-app/plugins/commit/23fbba6d92b25a375a2fca493d9c426807c6ce54))

Releases before the move to `plugins/notify` are on the [releases page](https://github.com/fliks-app/plugins/releases?q=notify-v).
