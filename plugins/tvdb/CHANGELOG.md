# Changelog

## [1.0.2](https://github.com/fliks-app/plugins/compare/tvdb-v1.0.1...tvdb-v1.0.2) (2026-11-22)


### Bug Fixes

* **download,tvdb:** review fixes for opaque refs and per-episode names ([#198](https://github.com/fliks-app/plugins/issues/198)) ([df0496b](https://github.com/fliks-app/plugins/commit/df0496bb7af27484291b940ce12890f8b5f57baf))

## [1.0.1](https://github.com/fliks-app/plugins/compare/tvdb-v1.0.0...tvdb-v1.0.1) (2026-11-22)


### Bug Fixes

* **tmdb,tvdb,comicvine:** atomic season reads, one-call translated episodes, interactive-safe comicvine limits ([#192](https://github.com/fliks-app/plugins/issues/192)) ([5a0f2b0](https://github.com/fliks-app/plugins/commit/5a0f2b0be9f277610f4a7c8e2c25ae80208de18c))

## [1.0.0](https://github.com/fliks-app/plugins/compare/tvdb-v0.3.2...tvdb-v1.0.0) (2026-11-19)


### ⚠ BREAKING CHANGES

* every plugin requires fliks >=7.0.0 <8.0.0 and no longer loads on core 6.

### Features

* require fliks &gt;=7.0.0 &lt;8.0.0 and build against the 7.0.0 plugin WIT ([#159](https://github.com/fliks-app/plugins/issues/159)) ([33c68b0](https://github.com/fliks-app/plugins/commit/33c68b0c17d00aa347f5891b39c4116b3e09eeb6))

## [0.3.2](https://github.com/fliks-app/plugins/compare/tvdb-v0.3.1...tvdb-v0.3.2) (2026-11-16)


### Bug Fixes

* rebuild every plugin against the new provider interface ([#135](https://github.com/fliks-app/plugins/issues/135)) ([0cfe1ac](https://github.com/fliks-app/plugins/commit/0cfe1acc9424c435f3b752564d2f1b6dda9bb950))

## [0.3.1](https://github.com/fliks-app/plugins/compare/tvdb-v0.3.0...tvdb-v0.3.1) (2026-11-05)


### Bug Fixes

* **tvdb:** drop the missing artwork placeholder and count aired seasons only ([#86](https://github.com/fliks-app/plugins/issues/86)) ([2ba2f4d](https://github.com/fliks-app/plugins/commit/2ba2f4db5dfd2fe59e19681dbf77a3544dc30e5b))

## [0.3.0](https://github.com/fliks-app/plugins/compare/tvdb-v0.2.0...tvdb-v0.3.0) (2026-11-04)


### Features

* **tmdb,tvdb:** fetch one season when fetch-children is asked for it ([#58](https://github.com/fliks-app/plugins/issues/58)) ([18e3dce](https://github.com/fliks-app/plugins/commit/18e3dce97c92c7cc858f60d2c01c7cac3b887ad7))

## [0.2.0](https://github.com/fliks-app/plugins/compare/tvdb-v0.1.0...tvdb-v0.2.0) (2026-11-02)


### Features

* **tvdb:** add the TheTVDB metadata provider plugin ([#13](https://github.com/fliks-app/plugins/issues/13)) ([40025cb](https://github.com/fliks-app/plugins/commit/40025cb488e06223d5ed4475b0f5a3f32d382988))
