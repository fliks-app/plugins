# Changelog

## [2.8.0](https://github.com/fliks-app/plugins/compare/download-v2.7.0...download-v2.8.0) (2026-11-09)


### Features

* **download:** let the core run automatic acquisition ([#116](https://github.com/fliks-app/plugins/issues/116)) ([60062c6](https://github.com/fliks-app/plugins/commit/60062c62bee4bdb3fc41c987ce0461d46f70f22b))

## [2.7.0](https://github.com/fliks-app/plugins/compare/download-v2.6.0...download-v2.7.0) (2026-11-09)


### Features

* **download:** skip an auto-grab another provider has in flight ([#114](https://github.com/fliks-app/plugins/issues/114)) ([7d23fc0](https://github.com/fliks-app/plugins/commit/7d23fc0ed5e5c94a380f5b122e806d066ff29e33))

## [2.6.0](https://github.com/fliks-app/plugins/compare/download-v2.5.0...download-v2.6.0) (2026-11-09)


### Features

* **download:** act as an acquisition provider ([#112](https://github.com/fliks-app/plugins/issues/112)) ([b785653](https://github.com/fliks-app/plugins/commit/b785653158bda6a45836d53b66be734a58913406))

## [2.5.0](https://github.com/fliks-app/plugins/compare/download-v2.4.1...download-v2.5.0) (2026-11-08)


### Features

* **download,musicbrainz,music-lists:** acquire albums, serve track popularity, add the music lists plugin ([#96](https://github.com/fliks-app/plugins/issues/96)) ([bd7001f](https://github.com/fliks-app/plugins/commit/bd7001f9a9503a31c42ee124786b33be2ba5b957))

## [2.4.1](https://github.com/fliks-app/plugins/compare/download-v2.4.0...download-v2.4.1) (2026-11-07)


### Bug Fixes

* **download:** search each target once per rss run and honour flagged matches ([#91](https://github.com/fliks-app/plugins/issues/91)) ([6e307ad](https://github.com/fliks-app/plugins/commit/6e307ad288f74af6a60b7195aeebcd8286536d51))

## [2.4.0](https://github.com/fliks-app/plugins/compare/download-v2.3.0...download-v2.4.0) (2026-11-05)


### Features

* **download:** seeders/leechers column and release replace in the queue ([#88](https://github.com/fliks-app/plugins/issues/88)) ([0c9dd80](https://github.com/fliks-app/plugins/commit/0c9dd80de28acb5c5f32456d854a1764b1026b82))

## [2.3.0](https://github.com/fliks-app/plugins/compare/download-v2.2.0...download-v2.3.0) (2026-11-05)


### Features

* **download:** group and item release routes, family on search and grab ([#84](https://github.com/fliks-app/plugins/issues/84)) ([da854e6](https://github.com/fliks-app/plugins/commit/da854e69bfac30d8fc1007020013ed594ff6352c))

## [2.2.0](https://github.com/fliks-app/plugins/compare/download-v2.1.0...download-v2.2.0) (2026-11-04)


### Features

* **download:** search and grab comic issues ([#60](https://github.com/fliks-app/plugins/issues/60)) ([d3769d5](https://github.com/fliks-app/plugins/commit/d3769d5d5fceb21ccde95a696e4b02c2be897181))

## [2.1.0](https://github.com/fliks-app/plugins/compare/download-v2.0.0...download-v2.1.0) (2026-11-04)


### Features

* **download:** acquire books end to end ([#40](https://github.com/fliks-app/plugins/issues/40)) ([32924f0](https://github.com/fliks-app/plugins/commit/32924f051fdf13ca762e45c2a8e03ac1b521e48a))
* **download:** acquire books per language ([#44](https://github.com/fliks-app/plugins/issues/44)) ([d8d6d81](https://github.com/fliks-app/plugins/commit/d8d6d810cfceb5b8ee24843c0b897e41563e8af6))
* **download:** grab cbz books and keep the format table in step with the ingest lists ([#47](https://github.com/fliks-app/plugins/issues/47)) ([ae46224](https://github.com/fliks-app/plugins/commit/ae46224a71bab08896fb8a8e546416dc23e9f79f))
* **download:** query indexers from the target's search-units ([#62](https://github.com/fliks-app/plugins/issues/62)) ([49127b8](https://github.com/fliks-app/plugins/commit/49127b8f512241115bd8684b0f183b43bad7146e))
* **download:** search books by category with t=book ([#31](https://github.com/fliks-app/plugins/issues/31)) ([bf60322](https://github.com/fliks-app/plugins/commit/bf60322449f9d8bbd02132bfbef09f2da5d9470b))


### Bug Fixes

* **download:** fall back to t=search only when the tracker refuses the query ([#61](https://github.com/fliks-app/plugins/issues/61)) ([bd4d3eb](https://github.com/fliks-app/plugins/commit/bd4d3ebcc8d663733e68fc2762c304ed7d2610cf))
* **download:** list every category Fliks manages ([#41](https://github.com/fliks-app/plugins/issues/41)) ([28910e9](https://github.com/fliks-app/plugins/commit/28910e9ab20530fb749eae26030f97651ae08caf))
* **download:** remember a tracker's t=book refusal ([#59](https://github.com/fliks-app/plugins/issues/59)) ([5e85941](https://github.com/fliks-app/plugins/commit/5e8594114891cba4ef686d9e81fe302ed05ccb63)), closes [#42](https://github.com/fliks-app/plugins/issues/42)
* **download:** send every release language on the wire ([#57](https://github.com/fliks-app/plugins/issues/57)) ([d669a60](https://github.com/fliks-app/plugins/commit/d669a607f52a524a75d327dda60b81cdb0b5ba07))

## [2.0.0](https://github.com/fliks-app/plugins/compare/download-v1.2.0...download-v2.0.0) (2026-11-02)


### ⚠ BREAKING CHANGES

* **download:** requires pluginApi 3 and fliks >=6.0.0 <7.0.0.

### Features

* **download:** port to pluginApi 3 and key download history by item ([051acbb](https://github.com/fliks-app/plugins/commit/051acbb785bda569e55c763bbc718400835d00d7))

Releases before the move to `plugins/download` are on the [releases page](https://github.com/fliks-app/plugins/releases?q=download-v).
