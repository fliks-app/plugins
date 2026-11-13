//! Golden vectors recorded from the TypeScript plugin at v0.12.1 (`tests/vectors/gen/gen.ts`),
//! and the manifest template against the published 0.12.1 manifest.

use fk_plugin_download::db::{HistoryRow, IndexerRow, MIGRATIONS};
use fk_plugin_download::{client, grab, poller, routes, throttle, torznab, util};
use fliks_plugin_route::{Matcher, ParamValue};
use fliks_plugin_sdk::bindings::fliks::plugin::types::{
    Creator, ExternalId, Family, Rejection, ReleaseCoverage, ReleaseLanguage, ScoredRelease, Span,
    Target, Unit, UnitRef, Want, WantDecision,
};
use serde_json::{Map, Value, json};
use std::collections::HashSet;

fn golden() -> Value {
    serde_json::from_str(include_str!("vectors/golden.json")).unwrap()
}

fn cases<'a>(g: &'a Value, k: &str) -> &'a Vec<Value> {
    g[k].as_array().unwrap_or_else(|| panic!("no {k} vectors"))
}

#[test]
fn migrations_are_byte_identical() {
    let g = golden();
    let recorded = cases(&g, "migrations");
    assert_eq!(recorded.len(), 8);
    for (i, m) in recorded.iter().enumerate() {
        assert_eq!(MIGRATIONS[i].0, m["name"].as_str().unwrap());
        assert_eq!(
            MIGRATIONS[i].1,
            m["up"].as_str().unwrap(),
            "{} differs",
            MIGRATIONS[i].0
        );
    }
    let names: Vec<&str> = MIGRATIONS[8..12].iter().map(|m| m.0).collect();
    assert_eq!(
        names,
        [
            "0009_indexer_throttle",
            "0010_poller_state",
            "0011_download_history_import_lease",
            "0012_item_history"
        ]
    );
    assert_eq!(MIGRATIONS[12].0, "0013_indexer_caps_books");
}

#[test]
fn html_entities() {
    for c in cases(&golden(), "htmlEntities") {
        assert_eq!(
            util::decode_html_entities(c["input"].as_str().unwrap()),
            c["output"].as_str().unwrap(),
            "{c}"
        );
    }
}

#[test]
fn encode_uri_component() {
    for c in cases(&golden(), "encodeUriComponent") {
        assert_eq!(
            util::encode_uri_component(c["input"].as_str().unwrap()),
            c["output"].as_str().unwrap()
        );
    }
}

#[test]
fn torznab_query() {
    for c in cases(&golden(), "torznabQuery") {
        let i = &c["input"];
        let s = |k: &str| i[k].as_str();
        let n = |k: &str| i[k].as_u64().map(|v| v as u32);
        let q = torznab::Query {
            t: s("t").unwrap(),
            q: s("q"),
            season: n("season"),
            ep: n("ep"),
            cat: s("cat").unwrap(),
            api_key: s("apiKey").unwrap(),
            tvdb_id: n("tvdbId"),
            imdb_id: s("imdbId"),
            tmdb_id: n("tmdbId"),
            supported_params: s("supportedParams"),
            ..Default::default()
        };
        assert_eq!(
            torznab::build_query(&q),
            c["output"].as_str().unwrap(),
            "{c}"
        );
    }
    for c in cases(&golden(), "describeQuery") {
        assert_eq!(
            torznab::describe_query(c["input"].as_str().unwrap()),
            c["output"].as_str().unwrap()
        );
    }
}

fn indexer(id: i64, settings: &Value) -> IndexerRow {
    serde_json::from_value(json!({
        "id": id, "name": "test-indexer", "implementation": "torznab", "settings": settings,
        "enableRss": true, "enableSearch": true, "enableInteractiveSearch": true, "priority": 25,
        "enabled": true, "capsSearchFallback": false, "capsMovieSearch": false, "capsTvSearch": false,
        "capsMovieSearchParams": null, "capsTvSearchParams": null, "capsBookSearch": false,
        "capsBookSearchParams": null, "capsCategories": null, "capsProbedAt": null,
        "requestDelay": 2, "createdAt": "", "updatedAt": "",
    }))
    .unwrap()
}

#[test]
fn torznab_items() {
    for c in cases(&golden(), "torznabItems") {
        let ix = indexer(7, &c["settings"]);
        let got =
            serde_json::to_value(torznab::parse_items(c["xml"].as_str().unwrap(), &ix)).unwrap();
        assert_eq!(got, c["output"]);
    }
}

fn base64(s: &str) -> Vec<u8> {
    let alphabet = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let (mut out, mut buf, mut bits) = (vec![], 0u32, 0);
    for c in s.bytes().filter(|c| *c != b'=') {
        buf = (buf << 6) | alphabet.iter().position(|a| *a == c).unwrap() as u32;
        bits += 6;
        if bits >= 8 {
            bits -= 8;
            out.push((buf >> bits) as u8);
        }
    }
    out
}

#[test]
fn torrent_hashes() {
    let g = golden();
    for c in cases(&g, "magnetHash") {
        assert_eq!(
            client::magnet_hash(c["input"].as_str().unwrap()).as_deref(),
            c["output"].as_str()
        );
    }
    for c in cases(&g, "infoHash") {
        assert_eq!(
            client::info_hash(&base64(c["input"].as_str().unwrap())).as_deref(),
            c["output"].as_str()
        );
    }
}

#[test]
fn torrent_names() {
    for c in cases(&golden(), "torrentName") {
        assert_eq!(
            poller::normalise_name(c["input"].as_str().unwrap()),
            c["output"].as_str().unwrap()
        );
    }
}

#[test]
fn stall_strikes() {
    for c in cases(&golden(), "stallStrikes") {
        let samples: Vec<(i64, i64)> = c["input"]["samples"]
            .as_array()
            .unwrap()
            .iter()
            .map(|s| {
                (
                    s["downloadedBytes"].as_i64().unwrap(),
                    util::parse_date_ms(s["checkedAt"].as_str().unwrap()).unwrap(),
                )
            })
            .collect();
        let min = c["input"]["minBytesPerSecond"].as_f64().unwrap();
        assert_eq!(
            client::stalled_strikes(&samples, min),
            c["output"].as_i64().unwrap(),
            "{c}"
        );
    }
}

#[test]
fn qbit_base_url() {
    for c in cases(&golden(), "qbitBaseUrl") {
        let s = c["input"].as_object().unwrap().clone();
        assert_eq!(client::base_url(&s).as_deref(), c["output"].as_str(), "{c}");
    }
}

#[test]
fn progress_state() {
    for c in cases(&golden(), "progressState") {
        assert_eq!(
            client::progress_state(c["input"].as_str().unwrap()),
            c["output"].as_str().unwrap()
        );
    }
}

#[test]
fn coverage_columns() {
    for c in cases(&golden(), "coverageColumns") {
        let span = |v: &Value| {
            v.as_object().map(|o| Span {
                first: o["from"].as_f64().unwrap(),
                last: o["to"].as_f64().unwrap(),
            })
        };
        let cov = c["input"].as_object().map(|o| ReleaseCoverage {
            groups: span(&o["seasons"]),
            units: span(&o["episodes"]),
            complete: o["completeSeries"].as_bool().unwrap(),
        });
        let got = serde_json::to_value(grab::coverage_columns(cov.as_ref())).unwrap();
        assert_eq!(got, c["output"], "{c}");
    }
}

#[test]
fn secret_settings() {
    for c in cases(&golden(), "secretSettings") {
        let existing = c["input"]["existing"].as_object();
        let incoming = c["input"]["incoming"].as_object().unwrap();
        assert_eq!(
            Value::Object(util::merge_secrets(existing, incoming, &["apiKey"])),
            c["merged"],
            "{c}"
        );
        assert_eq!(
            Value::Object(util::redact_secrets(existing, &["apiKey"])),
            c["redacted"],
            "{c}"
        );
    }
}

/// Every key but the ones the port changes equals the published 0.12.1 manifest, except the
/// renamed `coreRefs`, the per-kind `releasePicker`, the season/episode to group/item rename and
/// the additions checked below; strings may only be added.
#[test]
fn template_matches_the_published_manifest() {
    let renamed = include_str!("vectors/manifest-0.12.1.json")
        .replace("/:id/seasons/:seasonId/", "/:id/groups/:groupId/")
        .replace("/:id/episodes/:episodeId/", "/:id/items/:itemId/")
        .replace("media.season.actions", "media.group.actions")
        .replace("fliks-download.season.", "fliks-download.group.")
        .replace("download.season.", "download.group.")
        .replace("\"season.", "\"group.");
    let mut old: Value = serde_json::from_str(&renamed).unwrap();
    let mut new: Value = serde_json::from_str(include_str!("../plugin.template.json")).unwrap();
    // The grab buttons are core-owned since the provider contract: only settings and nav remain.
    let buttons = |c: &Value| {
        ["media.actions", "media.group.actions", "media.item.actions"]
            .contains(&c["slot"].as_str().unwrap_or_default())
    };
    let dropped = |v: &mut Value| {
        let all = v["ui"]["contributions"].as_array_mut().unwrap();
        all.extract_if(.., |c| buttons(c)).count()
    };
    assert_eq!(dropped(&mut new), 0);
    assert_eq!(dropped(&mut old), 3);
    assert_eq!(
        (
            &new["pluginApi"],
            &new["fliks"],
            &new["runtime"],
            &new["network"]
        ),
        (
            &json!(3),
            &json!(">=6.1.0 <7.0.0"),
            &json!("wasm"),
            &json!({ "internal": true })
        )
    );
    let secrets = new["database"]
        .as_object_mut()
        .unwrap()
        .remove("secrets")
        .unwrap();
    assert_eq!(
        secrets,
        json!([
            { "table": "indexers", "column": "settings", "jsonPath": "apiKey" },
            { "table": "indexer_sources", "column": "settings", "jsonPath": "apiKey" },
            { "table": "download_clients", "column": "settings", "jsonPath": "password" }
        ])
    );
    let core_refs = new["database"]
        .as_object_mut()
        .unwrap()
        .remove("coreRefs")
        .unwrap();
    assert_eq!(core_refs, json!(["item_groups", "items", "media", "users"]));
    old["database"].as_object_mut().unwrap().remove("coreRefs");
    // A new scope would hold the boot auto-update for an admin.
    assert_eq!(
        new["scopes"],
        json!([
            "media:read",
            "acquisition:candidates",
            "releases:score",
            "ingest:write",
            "events:emit",
            "config:rw"
        ])
    );
    let picker = new["ui"].as_object_mut().unwrap().remove("releasePicker");
    let item = json!({
        "search": "/:id/items/:itemId/releases",
        "grab": "/:id/items/:itemId/grab"
    });
    let work = json!({ "search": "/:id/releases", "grab": "/:id/grab" });
    assert_eq!(
        picker,
        Some(json!({
            "movie": { "work": work },
            "series": {
                "group": {
                    "search": "/:id/groups/:groupId/releases",
                    "grab": "/:id/groups/:groupId/grab"
                },
                "item": item
            },
            "book": { "work": work },
            "comic": { "work": work, "item": item },
            "album": { "work": work, "item": item }
        }))
    );
    old["ui"].as_object_mut().unwrap().remove("releasePicker");
    // The queue and history UI moved to the core: its routes, nav entries, pages and permission go,
    // and the plugin states its four actions.
    let actions: Vec<Value> = new["routes"]
        .as_array_mut()
        .unwrap()
        .extract_if(.., |r| {
            r["path"].as_str().is_some_and(|p| p.starts_with("/queue/"))
        })
        .collect();
    assert_eq!(actions.len(), 4);
    let queue_ui = |p: &str| p.starts_with("/queue") || p.starts_with("/history");
    old["routes"]
        .as_array_mut()
        .unwrap()
        .retain(|r| !queue_ui(r["path"].as_str().unwrap()));
    old["ui"]["contributions"]
        .as_array_mut()
        .unwrap()
        .retain(|c| {
            ![
                "fliks-download.nav.queue",
                "fliks-download.settings.history",
            ]
            .contains(&c["id"].as_str().unwrap())
        });
    old["ui"]["configPages"]
        .as_array_mut()
        .unwrap()
        .retain(|p| !["queue", "history"].contains(&p["id"].as_str().unwrap()));
    old["permissions"]
        .as_array_mut()
        .unwrap()
        .retain(|p| p != "queue");
    new.as_object_mut().unwrap().remove("acquisition");
    // Removed since: the core runs the automatic search, so no SearchMissing job and no immediate-search toggle.
    old["jobs"]
        .as_array_mut()
        .unwrap()
        .retain(|j| j["name"] != "SearchMissing");
    for page in old["ui"]["configPages"].as_array_mut().unwrap() {
        if let Some(fields) = page.get_mut("fields").and_then(Value::as_array_mut) {
            fields.retain(|f| f["key"] != "requestsAutoGrabOnApproval");
        }
    }
    let (old_i18n, new_i18n) = (old["i18n"].clone(), new["i18n"].clone());
    for k in [
        "pluginApi",
        "fliks",
        "runtime",
        "memoryMb",
        "scopes",
        "network",
        "version",
        "files",
        "i18n",
    ] {
        old.as_object_mut().unwrap().remove(k);
        new.as_object_mut().unwrap().remove(k);
    }
    assert_eq!(new, old);
    for (lang, strings) in old_i18n.as_object().unwrap() {
        for (k, v) in strings.as_object().unwrap() {
            let removed = k.starts_with("download.config.general.immediate_search")
                || k.starts_with("download.config.queue.")
                || k.starts_with("download.config.history.")
                || k.starts_with("download.status.")
                || k == "download.jobs.search_missing";
            if removed || k.starts_with("download.media.") || k.starts_with("download.group.") {
                continue;
            }
            assert_eq!(&new_i18n[lang][k], v, "{lang} {k}");
        }
    }
}

/// The host's own manifest validation, so a shape it refuses fails here and not at install.
#[test]
fn template_passes_the_host_validation() {
    let t: Value = serde_json::from_str(include_str!("../plugin.template.json")).unwrap();
    fliks_plugin_manifest::manifest::validate_shape(&t).unwrap();
}

#[test]
fn only_a_book_want_carries_a_language_to_the_host() {
    assert_eq!(grab::host_language(Some(Family::Image), Some("fr")), None);
    assert_eq!(grab::host_language(Some(Family::Video), Some("fr")), None);
    assert_eq!(
        grab::host_language(Some(Family::Text), Some("fr")),
        Some("fr")
    );
    assert_eq!(
        grab::host_language(Some(Family::Audio), Some("fr")),
        Some("fr")
    );
    assert_eq!(grab::host_language(None, Some("fr")), Some("fr"));
}

#[test]
fn routes_equal_the_manifest_both_ways() {
    let t: Value = serde_json::from_str(include_str!("../plugin.template.json")).unwrap();
    let manifest: Vec<(String, String)> = t["routes"]
        .as_array()
        .unwrap()
        .iter()
        .map(|r| {
            (
                r["method"].as_str().unwrap().to_owned(),
                r["path"].as_str().unwrap().to_owned(),
            )
        })
        .collect();
    let table: Vec<(String, String)> = routes::table()
        .into_iter()
        .map(|(m, p, _)| (m.to_owned(), p.to_owned()))
        .collect();
    assert_eq!(table.len(), 35);
    assert_eq!(table, manifest);
}

#[test]
fn every_job_has_a_handler() {
    let t: Value = serde_json::from_str(include_str!("../plugin.template.json")).unwrap();
    let jobs: Vec<&str> = t["jobs"]
        .as_array()
        .unwrap()
        .iter()
        .map(|j| j["name"].as_str().unwrap())
        .collect();
    assert_eq!(jobs.len(), 4);
    for j in jobs {
        assert!(
            fk_plugin_download::job_handler(j).is_some(),
            "{j} has no handler"
        );
    }
    assert!(fk_plugin_download::job_handler("Nope").is_none());
}

fn target(want: Option<WantDecision>) -> Target {
    Target {
        media_id: 1,
        kind: "movie".into(),
        family: Family::Video,
        language: None,
        title: String::new(),
        original_title: None,
        alternative_titles: vec![],
        creators: vec![],
        year: None,
        runtime_minutes: None,
        external_ids: vec![],
        edition_ids: vec![],
        library_id: 1,
        want: want.map(|decision| Want {
            decision,
            family: Family::Video,
            language: None,
            allowed_quality_ids: vec![],
            allowed_language_ids: vec![],
            min_resolution: 0,
            resolution_upgrade_only: false,
        }),
        expected_titles: vec![],
        search_title: String::new(),
        scope: vec![],
        search_units: vec![],
    }
}

fn unit(group: Option<u32>, number: Option<f64>) -> Unit {
    Unit {
        ref_: UnitRef {
            group,
            number,
            end_number: None,
            label: None,
            absolute: None,
        },
        group_id: group.map(|_| 20),
        item_id: number.map(|_| 50),
        title: None,
        air_date: None,
        unit_count: None,
    }
}

fn ranked(id: u64, rejections: u64) -> grab::Ranked {
    let raw = release(&json!({ "title": id.to_string() }));
    let scored = ScoredRelease {
        id: id.to_string(),
        quality_id: 0,
        quality_name: String::new(),
        rank: 0,
        allowed: true,
        custom_format_score: 0,
        blocklisted: false,
        languages: vec![],
        language_allowed: true,
        whole_group: false,
        coverage: None,
        size_deviation: None,
        video_codec: None,
        rejections: (0..rejections)
            .map(|_| Rejection {
                code: "x".into(),
                params: vec![],
            })
            .collect(),
    };
    grab::Ranked { raw, scored }
}

#[test]
fn pick_release() {
    for c in cases(&golden(), "pickRelease") {
        let want = match c["input"]["want"]["decision"].as_str() {
            None => None,
            Some("missing") => Some(WantDecision::Missing),
            Some("upgrade") => Some(WantDecision::Upgrade),
            Some("skip") => Some(WantDecision::Skip),
            Some(d) => panic!("{d}"),
        };
        let sorted: Vec<_> = c["input"]["sorted"]
            .as_array()
            .unwrap()
            .iter()
            .map(|r| ranked(r["id"].as_u64().unwrap(), r["rejections"].as_u64().unwrap()))
            .collect();
        let got: Vec<u64> = grab::pick(&sorted, &target(want))
            .into_iter()
            .map(|i| sorted[i].scored.id.parse().unwrap())
            .collect();
        let expected: Vec<u64> = c["output"]
            .as_array()
            .unwrap()
            .iter()
            .map(|v| v.as_u64().unwrap())
            .collect();
        assert_eq!(got, expected, "{c}");
    }
}

#[test]
fn movies_series_and_books_of_a_served_family_are_acquired() {
    use grab::{Media, family_of, family_str};
    assert_eq!(Media::of("movie", Family::Video), Some(Media::Movie));
    assert_eq!(Media::of("series", Family::Video), Some(Media::Series));
    assert_eq!(Media::of("book", Family::Text), Some(Media::Ebook));
    assert_eq!(Media::of("book", Family::Audio), Some(Media::Audiobook));
    assert_eq!(Media::of("album", Family::Audio), Some(Media::Album));
    for (kind, family) in [
        ("book", Family::Video),
        ("book", Family::Image),
        ("comic", Family::Text),
        ("album", Family::Text),
        ("", Family::Video),
    ] {
        assert_eq!(Media::of(kind, family), None, "{kind}");
    }
    for f in [Family::Video, Family::Audio, Family::Text, Family::Image] {
        assert_eq!(family_of(family_str(f)), Some(f));
    }
}

#[test]
fn a_target_is_the_work_a_season_or_an_episode() {
    use grab::Piece;
    let mut t = target(None);
    assert_eq!(grab::piece(&t), Piece::Work);
    t.scope = vec![unit(Some(2), None)];
    assert_eq!(grab::piece(&t), Piece::Season(2));
    t.scope = vec![unit(Some(2), Some(5.0))];
    assert_eq!(grab::piece(&t), Piece::Episode(2, 5));
    assert_eq!(grab::numbers(&t), (Some(2), Some(5)));
}

#[test]
fn torznab_ids_come_from_the_external_ids() {
    let id = |provider: &str, value: &str| ExternalId {
        provider: provider.into(),
        value: value.into(),
    };
    let list = [
        id("isbn", "9780000000002"),
        id("imdb", "tt0000001"),
        id("tmdb", "603"),
        id("tvdb", "bad"),
    ];
    let ids = torznab::Ids::of(&list);
    assert_eq!(
        (ids.imdb, ids.tmdb, ids.tvdb),
        (Some("tt0000001"), Some(603), None)
    );
    let none = torznab::Ids::of(&[]);
    assert_eq!((none.imdb, none.tmdb, none.tvdb), (None, None, None));
}

fn history_row(item: Option<i64>, season: Option<i64>) -> HistoryRow {
    serde_json::from_value(json!({
        "id": 1, "sourceTitle": "t", "quality": "q", "language": null, "torrentHash": null,
        "size": null, "infoUrl": null, "status": "grabbed", "statusMessage": null,
        "grabSource": "auto", "mediaId": 1, "itemId": item, "seasonId": season,
        "indexerId": null, "downloadClientId": null, "createdAt": "", "updatedAt": "",
        "coverSeasonFrom": null, "coverSeasonTo": null, "coverEpisodeFrom": null,
        "coverEpisodeTo": null, "coverCompleteSeries": null
    }))
    .unwrap()
}

#[test]
fn the_wire_episode_id_is_the_item_only_when_grouped() {
    assert_eq!(history_row(Some(7), Some(3)).episode_id(), Some(7));
    assert_eq!(history_row(Some(7), None).episode_id(), None);
    assert_eq!(history_row(None, Some(3)).episode_id(), None);
}

#[test]
fn season_target_picks_packs_only() {
    let mut t = target(Some(WantDecision::Missing));
    t.kind = "series".into();
    t.scope = vec![unit(Some(1), None)];
    let mut sorted: Vec<_> = (1..=3).map(|id| ranked(id, 0)).collect();
    sorted[1].scored.whole_group = true;
    assert_eq!(grab::pick(&sorted, &t), vec![1]);
}

fn release(v: &Value) -> torznab::Release {
    let mut r = json!({
        "title": "T", "downloadUrl": "https://t/x", "indexerId": 1, "indexerName": "ix", "size": 10,
        "seeders": 2, "leechers": 1, "publishDate": "2024-01-01T00:00:00.000Z", "freeleech": false,
        "downloadVolumeFactor": 1,
    });
    r.as_object_mut()
        .unwrap()
        .extend(v.as_object().unwrap().clone());
    torznab::Release {
        title: r["title"].as_str().unwrap().into(),
        download_url: r["downloadUrl"].as_str().unwrap().into(),
        info_url: None,
        indexer_id: r["indexerId"].as_i64().unwrap(),
        indexer_name: r["indexerName"].as_str().unwrap().into(),
        size: r["size"].as_i64().unwrap(),
        seeders: r["seeders"].as_i64().unwrap(),
        leechers: r["leechers"].as_i64().unwrap(),
        publish_date: r["publishDate"].as_str().map(str::to_owned),
        freeleech: r["freeleech"].as_bool().unwrap(),
        download_volume_factor: r["downloadVolumeFactor"].as_f64().unwrap(),
    }
}

#[test]
fn score_request() {
    for c in cases(&golden(), "scoreRequest") {
        let input = &c["input"];
        let releases: Vec<_> = input["releases"]
            .as_array()
            .unwrap()
            .iter()
            .map(release)
            .collect();
        let indexers: Vec<_> = input["settings"]
            .as_object()
            .unwrap()
            .iter()
            .map(|(id, s)| indexer(id.parse().unwrap(), s))
            .collect();
        let blocked: HashSet<String> = releases
            .iter()
            .filter(|r| {
                input["blocked"]
                    .as_array()
                    .unwrap()
                    .iter()
                    .any(|b| b.as_str().unwrap().eq_ignore_ascii_case(&r.title))
            })
            .map(|r| r.title.clone())
            .collect();
        let got = grab::score_request(&releases, &indexers, &blocked);
        let expected = c["output"].as_array().unwrap();
        assert_eq!(got.len(), expected.len());
        for (g, e) in got.iter().zip(expected) {
            assert_eq!(
                (g.id.as_str(), g.title.as_str(), g.source_ref.as_str()),
                (
                    e["id"].as_str().unwrap(),
                    e["title"].as_str().unwrap(),
                    e["sourceRef"].as_str().unwrap()
                )
            );
            assert_eq!(
                (g.size, g.seeders, g.leechers, g.blocked),
                (
                    e["size"].as_u64().unwrap(),
                    e["seeders"].as_u64().unwrap() as u32,
                    e["leechers"].as_u64().unwrap() as u32,
                    e["blocked"].as_bool().unwrap()
                ),
                "{e}"
            );
            assert_eq!(g.publish_date, e["publishDate"].as_str().unwrap());
            assert_eq!(g.freeleech, e["freeleech"].as_bool());
            assert_eq!(g.download_volume_factor, e["downloadVolumeFactor"].as_f64());
            assert_eq!(
                g.min_seeders.map(f64::from),
                e["minSeeders"].as_f64().map(f64::ceil),
                "{e}"
            );
            assert_eq!(
                g.unknown_language_iso_code.as_deref(),
                e["unknownLanguageIsoCode"].as_str(),
                "{e}"
            );
        }
    }
}

#[test]
fn retry_after() {
    let now = util::parse_date_ms("2023-11-14T22:13:20Z").unwrap();
    let different: Vec<String> = cases(&golden(), "retryAfter")
        .iter()
        .filter(|c| {
            throttle::parse_retry_after(c["input"].as_str().unwrap(), now)
                != c["output"].as_i64().unwrap()
        })
        .map(|c| c.to_string())
        .collect();
    assert!(different.is_empty(), "{}", different.join("\n"));
}

/// The host's own matcher, tried in table order with the method upper-cased, as `Router::handle`.
fn resolve(method: &str, path: &str) -> Option<Map<String, Value>> {
    let method = method.to_ascii_uppercase();
    routes::table()
        .into_iter()
        .filter(|(m, _, _)| *m == method)
        .find_map(|(_, p, _)| Matcher::new(p).unwrap().matches(path))
        .map(|params| {
            params
                .into_iter()
                .map(|(k, v)| match v {
                    ParamValue::One(s) => (k, json!(s)),
                    ParamValue::Many(p) => (k, json!(p)),
                })
                .collect()
        })
}

#[test]
fn route_match() {
    // The queue and history routes moved to the core: their frozen matches no longer apply.
    let moved = |c: &Value| {
        let p = c["path"].as_str().unwrap();
        (p.starts_with("/queue") || p.starts_with("/history")) && !c["output"].is_null()
    };
    let different: Vec<String> = cases(&golden(), "routeMatch")
        .iter()
        .filter(|c| !moved(c))
        .filter_map(|c| {
            let got = resolve(c["method"].as_str().unwrap(), c["path"].as_str().unwrap())
                .map_or(Value::Null, Value::Object);
            (got != c["output"]).then(|| format!("{c} -> {got}"))
        })
        .collect();
    // DL7: the host's matcher ignores case and a trailing `/`, and a capture is never empty.
    assert_eq!(different.len(), 3, "{}", different.join("\n"));
    for path in ["/1/Releases", "/1/releases", "//releases"] {
        assert!(different.iter().any(|d| d.contains(path)), "{path}");
    }
}

#[test]
fn wire_lists_every_release_language() {
    let mut r = ranked(1, 0);
    r.scored.languages = vec![
        ReleaseLanguage {
            id: 5,
            name: "Italian".into(),
        },
        ReleaseLanguage {
            id: 2,
            name: "French".into(),
        },
    ];
    assert_eq!(
        grab::to_wire(&r)["languages"],
        json!([{"id": 5, "name": "Italian"}, {"id": 2, "name": "French"}])
    );
}

fn issue(label: Option<&str>, number: f64) -> Target {
    let mut t = target(Some(WantDecision::Missing));
    t.kind = "comic".into();
    t.family = Family::Image;
    t.search_title = "Star Gardens".into();
    let mut u = unit(None, Some(number));
    u.ref_.label = label.map(str::to_owned);
    t.scope = vec![u];
    t
}

#[test]
fn a_comic_issue_is_searched_padded_then_bare() {
    let q = |t: &Target, c| grab::issue_queries(t, c);
    assert_eq!(
        q(&issue(Some("3"), 3.0), None),
        ["Star Gardens 003", "Star Gardens 3"]
    );
    assert_eq!(
        q(&issue(Some("1.5"), 1.5), None),
        ["Star Gardens 001.5", "Star Gardens 1.5"]
    );
    assert_eq!(
        q(&issue(Some("annual 2"), 2.0), None),
        ["Star Gardens annual 002", "Star Gardens annual 2"]
    );
    assert_eq!(q(&issue(Some("-1"), -1.0), None), ["Star Gardens -1"]);
    assert_eq!(q(&issue(Some("120"), 120.0), None), ["Star Gardens 120"]);
    assert_eq!(q(&issue(None, 7.0), None)[0], "Star Gardens 007");
    assert_eq!(q(&issue(Some("3"), 3.0), Some(" my words ")), ["my words"]);
    let mut volume = issue(None, 0.0);
    volume.scope.clear();
    assert_eq!(q(&volume, None), ["Star Gardens"]);
}

#[test]
fn a_comic_is_acquired_as_a_comic_in_its_own_category() {
    assert_eq!(
        grab::Media::of("comic", Family::Image).map(grab::Media::as_str),
        Some("comic")
    );
    assert!(grab::Media::of("comic", Family::Text).is_none());
}

fn track(disc: u32, number: f64) -> Target {
    let mut t = target(Some(WantDecision::Missing));
    t.kind = "album".into();
    t.family = Family::Audio;
    t.search_title = "Night Drive".into();
    t.creators = vec![Creator {
        name: " The Fixtures ".into(),
        role: "artist".into(),
    }];
    t.scope = vec![unit(Some(disc), Some(number))];
    t
}

#[test]
fn an_album_is_searched_by_artist_and_title_then_title_alone() {
    let q = |t: &Target, c| grab::album_queries(t, c);
    assert_eq!(
        q(&track(1, 3.0), None),
        ["The Fixtures Night Drive", "Night Drive"]
    );
    assert_eq!(q(&track(1, 3.0), Some(" my words ")), ["my words"]);
    let mut various = track(1, 3.0);
    various.creators[0].name = "Various Artists".into();
    assert_eq!(q(&various, None), ["Night Drive"]);
}

#[test]
fn a_track_is_searched_by_its_own_title() {
    let mut t = track(1, 3.0);
    t.scope[0].title = Some(" Red Lights ".into());
    assert_eq!(
        grab::album_queries(&t, None),
        ["The Fixtures Red Lights", "Red Lights"]
    );
}

#[test]
fn a_track_want_is_the_album_work_not_an_episode() {
    use grab::Piece;
    let mut t = track(2, 5.0);
    assert_eq!(grab::piece(&t), Piece::Work);
    t.scope = vec![unit(Some(0), Some(1.0))];
    assert_eq!(grab::piece(&t), Piece::Work);
    let mut searched = grab::Searched::default();
    assert!(searched.first(&track(1, 1.0)));
    assert!(!searched.first(&track(1, 2.0)), "tracks share one search");
}

#[test]
fn an_album_pick_keeps_the_allowed_and_unrejected() {
    let mut denied = ranked(1, 0);
    denied.scored.allowed = false;
    let sorted = [denied, ranked(2, 1), ranked(3, 0)];
    assert_eq!(grab::pick(&sorted, &track(1, 1.0)), [2]);
    assert!(poller::MUSIC_EXTS.contains(&".wav"));
}

#[test]
fn a_comic_pick_keeps_the_allowed_and_unrejected() {
    let mut denied = ranked(1, 0);
    denied.scored.allowed = false;
    let sorted = [denied, ranked(2, 1), ranked(3, 0)];
    assert_eq!(grab::pick(&sorted, &issue(Some("3"), 3.0)), [2]);
}

#[test]
fn a_comic_match_is_the_issue_with_the_same_decimal_number_and_label() {
    let m = |label: &str, number: f64| UnitRef {
        group: None,
        number: Some(number),
        end_number: None,
        label: Some(label.into()),
        absolute: None,
    };
    let t = issue(Some("1.5"), 1.5);
    assert!(grab::same_issue(&t, &m("1.5", 1.5)));
    assert!(!grab::same_issue(&t, &m("1", 1.0)), "1.5 is not issue 1");
    let annual = issue(Some("annual 2"), 2.0);
    assert!(grab::same_issue(&annual, &m("Annual 2", 2.0)));
    assert!(
        !grab::same_issue(&annual, &m("2", 2.0)),
        "an annual is not issue 2"
    );
    assert!(poller::COMIC_EXTS.contains(&".pdf"));
}

#[test]
fn an_rss_run_searches_each_target_once() {
    let mut run = grab::Searched::default();
    let ep = |media: u32, item: u32| {
        let mut t = target(Some(WantDecision::Missing));
        t.media_id = media;
        let mut u = unit(Some(1), Some(f64::from(item)));
        u.item_id = Some(item);
        t.scope = vec![u];
        t
    };
    assert!(run.first(&ep(1, 1)));
    assert!(!run.first(&ep(1, 1)), "same target from a second feed");
    assert!(run.first(&ep(1, 2)));
    assert!(run.first(&ep(2, 1)));

    let mut pack = ep(3, 1);
    pack.scope[0].item_id = None;
    let mut episode = ep(3, 1);
    episode.scope[0].group_id = pack.scope[0].group_id;
    assert!(run.first(&pack), "a season pack");
    assert!(run.first(&episode), "an episode of the same group");

    let book = |family, language: &str| {
        let mut t = ep(4, 1);
        t.kind = "book".into();
        t.family = family;
        t.language = Some(language.into());
        t
    };
    assert!(run.first(&book(Family::Text, "fr")));
    assert!(run.first(&book(Family::Text, "en")));
    assert!(run.first(&book(Family::Audio, "fr")));
    assert!(!run.first(&book(Family::Text, "fr")));
}
