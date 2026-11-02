use crate::tmdb::*;
use fliks_plugin_sdk_provider::Lang;
use serde_json::{Value, json};
use std::cell::RefCell;
use std::future::Future;
use std::task::{Context, Poll, Waker};

/// Runs a future whose every await is ready, as the fake answers are.
fn run<F: Future>(f: F) -> F::Output {
    let mut f = std::pin::pin!(f);
    match f.as_mut().poll(&mut Context::from_waker(Waker::noop())) {
        Poll::Ready(v) => v,
        Poll::Pending => panic!("pending"),
    }
}

type Reply = (u16, Option<&'static str>, String);

/// Answers each GET from `handler(path)` and records what was asked.
struct Fake {
    handler: Box<dyn Fn(&str) -> Reply>,
    seen: RefCell<Vec<(String, Query)>>,
    warns: RefCell<Vec<String>>,
}

impl Http for Fake {
    async fn get(&self, path: &str, query: Query) -> Result<Value> {
        self.seen.borrow_mut().push((path.to_owned(), query));
        let (status, retry, body) = (self.handler)(path);
        reply(status, retry, body.as_bytes())
    }

    fn warn(&self, message: &str) {
        self.warns.borrow_mut().push(message.to_owned());
    }
}

fn tmdb(handler: impl Fn(&str) -> Reply + 'static) -> Tmdb<Fake> {
    Tmdb {
        http: Fake {
            handler: Box::new(handler),
            seen: RefCell::default(),
            warns: RefCell::default(),
        },
    }
}

fn ok(body: &str) -> Reply {
    (200, None, body.to_owned())
}

fn lang(language: &str, region: &str) -> Resolved {
    Resolved::of(&Lang {
        language: language.into(),
        region: Some(region.into()),
        locale: format!("{language}-{region}"),
    })
}

fn en() -> Resolved {
    lang("en", "US")
}

fn asked(t: &Tmdb<Fake>, i: usize) -> (String, Query) {
    t.http.seen.borrow()[i].clone()
}

fn has(query: &Query, k: &str, v: &str) -> bool {
    query.iter().any(|(a, b)| a == k && b == v)
}

const MOVIE: &str = include_str!("../tests/fixtures/movie_1001.json");
const SHOW: &str = include_str!("../tests/fixtures/tv_2000.json");
const SEASON_0: &str = include_str!("../tests/fixtures/tv_2000_season_0.json");
const SEASON_1: &str = include_str!("../tests/fixtures/tv_2000_season_1.json");

/// A two-season series whose second season answers `second`.
fn seasons_with(second: u16) -> (Tmdb<Fake>, Result<Vec<Value>>) {
    let t = tmdb(move |path| match path {
        "/tv/1" => ok(
            r#"{"seasons":[{"season_number":1,"episode_count":1},{"season_number":2,"episode_count":1}]}"#,
        ),
        "/tv/1/season/1" => ok(r#"{"season_number":1,"episodes":[]}"#),
        _ => (second, Some("0"), "{}".into()),
    });
    let r = run(t.seasons(1, &en()));
    (t, r)
}

#[test]
fn a_missing_season_is_dropped() {
    let (t, r) = seasons_with(404);
    assert_eq!(r.unwrap().len(), 1);
    assert_eq!(t.http.warns.borrow().len(), 1);
}

#[test]
fn a_throttled_season_fails_the_list_instead_of_shortening_it() {
    let (_, r) = seasons_with(429);
    assert!(matches!(r, Err(ProviderError::RateLimited(Some(0)))));
    let (_, r) = seasons_with(503);
    assert!(matches!(r, Err(ProviderError::Upstream(u)) if u.status == Some(503)));
}

use fliks_plugin_sdk_provider::ProviderError;

#[test]
fn the_api_answers_map_to_provider_errors() {
    assert!(matches!(
        reply(429, Some(" 7 "), b"{}"),
        Err(ProviderError::RateLimited(Some(7)))
    ));
    assert!(matches!(
        reply(429, Some("Sun, 06 Nov 1994 08:49:37 GMT"), b"{}"),
        Err(ProviderError::RateLimited(None))
    ));
    assert!(matches!(
        reply(401, None, b"{}"),
        Err(ProviderError::Unauthorized)
    ));
    assert!(matches!(
        reply(404, None, b"{}"),
        Err(ProviderError::NotFound)
    ));
    let e = reply(500, None, br#"{"status_message":"boom"}"#).unwrap_err();
    assert!(matches!(&e, ProviderError::Upstream(u)
        if u.status == Some(500) && u.message == "HTTP 500: boom"));
    let e = reply(502, None, b"<html>").unwrap_err();
    assert!(matches!(&e, ProviderError::Upstream(u)
        if u.message == "HTTP 502: Request failed with status code 502"));
    let e = reply(200, None, b"<html>").unwrap_err();
    assert!(matches!(&e, ProviderError::Upstream(u)
        if u.status == Some(200) && u.message == "invalid JSON response"));
    assert_eq!(reply(200, None, b"[1]").unwrap(), json!([1]));
}

#[test]
fn the_language_follows_the_resolved_locale() {
    let fr = lang("fr", "FR");
    assert_eq!(fr.image_languages, "fr,en,null");
    assert_eq!(fr.release_priority, ["FR", "US"]);
    assert_eq!(en().image_languages, "en,null");
    assert_eq!(en().release_priority, ["US"]);
    let no_region = Resolved::of(&Lang {
        language: "fr".into(),
        region: None,
        locale: "fr-CA".into(),
    });
    assert_eq!(no_region.region, "CA");
}

#[test]
fn search_maps_the_results_and_passes_the_year() {
    let body = include_str!("../tests/fixtures/search_tv.json");
    let t = tmdb(move |_| ok(body));
    let found = run(t.search(true, "sample", Some(2008), &en())).unwrap();
    let (path, query) = asked(&t, 0);
    assert_eq!(path, "/search/tv");
    assert!(has(&query, "query", "sample") && has(&query, "language", "en-US"));
    assert!(has(&query, "first_air_date_year", "2008"));
    assert_eq!(found.len(), 2);
    assert_eq!(
        found[0],
        json!({
            "tmdbId": 2000, "provider": "tmdb", "title": "Sample Show",
            "originalTitle": "Sample Show (orig)", "overview": "Overview of Sample Show",
            "year": 2008, "posterUrl": "https://image.tmdb.org/t/p/w500/poster2000.jpg",
            "fanartUrl": null, "rating": 8.1, "genres": [], "genreIds": [18],
            "mediaType": "series"
        })
    );
    let t = tmdb(|_| ok(r#"{"results":[]}"#));
    run(t.search(false, "x", Some(1999), &en())).unwrap();
    assert!(has(&asked(&t, 0).1, "year", "1999"));
    assert_eq!(asked(&t, 0).0, "/search/movie");
}

#[test]
fn a_search_without_results_fails_like_the_node_client() {
    let t = tmdb(|_| ok("{}"));
    let e = run(t.search(false, "x", None, &en())).unwrap_err();
    assert!(matches!(e, ProviderError::Upstream(u) if u.status.is_none()));
}

#[test]
fn movie_details_follow_the_server_mapping() {
    let t = tmdb(|_| ok(MOVIE));
    let d = run(t.details(false, 1001, &en())).unwrap();
    let (path, query) = asked(&t, 0);
    assert_eq!(path, "/movie/1001");
    assert!(has(&query, "include_image_language", "en,null"));
    assert!(has(
        &query,
        "append_to_response",
        "external_ids,images,release_dates,credits,videos,keywords,alternative_titles"
    ));
    let img = "https://image.tmdb.org/t/p";
    assert_eq!(d["tmdbId"], 1001);
    assert_eq!(d["provider"], "tmdb");
    assert_eq!(d["title"], "Sample Film");
    assert_eq!(d["year"], 1999);
    assert_eq!(d["runtime"], 136);
    assert_eq!(d["status"], "released");
    assert_eq!(d["budget"], 63_000_000);
    assert_eq!(d["revenue"], Value::Null);
    assert_eq!(d["imdbId"], "tt0000002");
    assert_eq!(d["tvdbId"], Value::Null);
    assert_eq!(d["tagline"], "A tagline.");
    assert_eq!(d["genres"], json!(["Action", "Science Fiction"]));
    assert_eq!(d["genreIds"], json!([28, 878]));
    assert_eq!(d["posterUrl"], format!("{img}/w500/poster1001.jpg"));
    assert_eq!(d["logoUrl"], format!("{img}/original/logo_en.png"));
    assert_eq!(
        d["additionalFanartUrls"],
        json!([
            format!("{img}/original/b3.jpg"),
            format!("{img}/original/b2.jpg"),
            format!("{img}/original/b4.jpg"),
        ])
    );
    assert_eq!(d["inCinemas"], "1999-03-31");
    assert_eq!(d["digitalRelease"], "1999-09-21");
    assert_eq!(d["physicalRelease"], "2000-01-01");
    assert_eq!(d["keywords"], json!(["hacker", "dystopia"]));
    assert_eq!(d["alternativeTitles"], json!(["Sample", "Sample 1"]));
    assert_eq!(d["tmdbCollectionId"], 2344);
    assert_eq!(d["tmdbCollectionName"], "Sample Collection");
    assert_eq!(
        d["cast"][0],
        json!({
            "externalId": 6384, "name": "Actor One", "character": "Role One",
            "avatarUrl": format!("{img}/w185/actor1.jpg"), "order": 0
        })
    );
    assert_eq!(d["cast"][1]["avatarUrl"], Value::Null);
    assert_eq!(d["crew"][0]["job"], "Director");
    assert_eq!(
        d["videos"],
        json!([{"key": "abc123", "site": "YouTube", "type": "Trailer", "name": "Official Trailer"}])
    );
}

#[test]
fn the_language_picks_the_logo_and_the_release_country() {
    let t = tmdb(|_| ok(MOVIE));
    let d = run(t.details(false, 1001, &lang("fr", "FR"))).unwrap();
    assert!(has(&asked(&t, 0).1, "include_image_language", "fr,en,null"));
    assert_eq!(
        d["logoUrl"],
        "https://image.tmdb.org/t/p/original/logo_fr.png"
    );
    assert_eq!(d["inCinemas"], "1999-06-23");
    assert_eq!(d["digitalRelease"], "1999-09-21");
}

#[test]
fn series_details_follow_the_server_mapping() {
    let t = tmdb(|_| ok(SHOW));
    let d = run(t.details(true, 2000, &en())).unwrap();
    let (path, query) = asked(&t, 0);
    assert_eq!(path, "/tv/2000");
    assert!(has(
        &query,
        "append_to_response",
        "external_ids,images,credits,videos,keywords,alternative_titles"
    ));
    assert_eq!(d["mediaType"], "series");
    assert_eq!(d["title"], "Sample Show");
    assert_eq!(d["year"], 2008);
    assert_eq!(d["tvdbId"], 1000);
    assert_eq!(d["imdbId"], "tt0000001");
    assert_eq!(d["runtime"], 47);
    assert_eq!(d["seasonCount"], 3);
    assert_eq!(d["episodeCount"], 5);
    assert_eq!(d["status"], "ended");
    assert_eq!(d["tagline"], Value::Null);
    assert_eq!(d["logoUrl"], Value::Null);
    assert_eq!(d["productionCountries"], json!(["US"]));
    assert_eq!(
        d["productionCompanies"],
        json!(["Network One", "Studio Two"])
    );
    assert_eq!(d["keywords"], json!(["keyword two"]));
    assert_eq!(d["alternativeTitles"], json!(["Sample Show (US)"]));
    assert_eq!(
        d["additionalFanartUrls"],
        json!(["https://image.tmdb.org/t/p/original/bb2.jpg"])
    );
    for k in ["inCinemas", "budget", "tmdbCollectionId"] {
        assert_eq!(d[k], Value::Null, "{k}");
    }
}

#[test]
fn child_stubs_keep_season_zero_and_default_the_count() {
    let t = tmdb(|_| ok(SHOW));
    let stubs = run(t.season_stubs(2000, &en())).unwrap();
    assert_eq!(
        stubs,
        [
            json!({"seasonNumber": 0, "episodeCount": 1}),
            json!({"seasonNumber": 1, "episodeCount": 2}),
            json!({"seasonNumber": 2, "episodeCount": 0}),
        ]
    );
}

#[test]
fn seasons_keep_the_stub_order_and_map_the_episodes() {
    let t = tmdb(|path| match path {
        "/tv/2000" => ok(SHOW),
        "/tv/2000/season/0" => ok(SEASON_0),
        "/tv/2000/season/1" => ok(SEASON_1),
        _ => (404, None, "{}".into()),
    });
    let seasons = run(t.seasons(2000, &en())).unwrap();
    let numbers: Vec<_> = seasons.iter().map(|s| s["seasonNumber"].clone()).collect();
    assert_eq!(numbers, [json!(0), json!(1)]);
    assert_eq!(seasons[1]["episodeCount"], 2);
    assert_eq!(seasons[1]["overview"], "Season 1 overview");
    assert_eq!(seasons[0]["airDate"], Value::Null);
    assert_eq!(
        seasons[1]["posterUrl"],
        "https://image.tmdb.org/t/p/w500/s1.jpg"
    );
    assert_eq!(
        seasons[1]["episodes"][0],
        json!({
            "episodeNumber": 1, "title": "Episode 1", "overview": null,
            "airDate": "2008-01-20", "runtime": 47,
            "stillUrl": "https://image.tmdb.org/t/p/w780/still0.jpg"
        })
    );
    assert_eq!(seasons[1]["episodes"][1]["stillUrl"], Value::Null);
}

#[test]
fn find_by_answers_the_first_match_and_ignores_other_sources() {
    let body = include_str!("../tests/fixtures/find_169.json");
    let t = tmdb(move |_| ok(body));
    let found = run(t.find_by("imdb", "tt0000002", &en())).unwrap();
    assert_eq!(found, Some(("1002".to_owned(), "movie")));
    let (path, query) = asked(&t, 0);
    assert_eq!(path, "/find/tt0000002");
    assert!(has(&query, "external_source", "imdb_id"));
    run(t.find_by("tvdb", "5", &en())).unwrap();
    assert!(has(&asked(&t, 1).1, "external_source", "tvdb_id"));
    assert_eq!(run(t.find_by("isbn", "1", &en())).unwrap(), None);
    assert_eq!(t.http.seen.borrow().len(), 2);
    let t = tmdb(|_| ok(r#"{"movie_results":[],"tv_results":[{"id":"abc"}]}"#));
    let found = run(t.find_by("tvdb", "5", &en())).unwrap();
    assert_eq!(found, Some(("abc".to_owned(), "series")));
}

#[test]
fn a_person_and_credits_follow_the_server_mapping() {
    let person = include_str!("../tests/fixtures/person_603.json");
    let t = tmdb(move |_| ok(person));
    let p = run(t.person(603, &en())).unwrap();
    assert_eq!(
        p,
        json!({
            "name": "Shared Id", "biography": "Bio", "birthday": "1963-12-18", "deathday": null,
            "placeOfBirth": "Shawnee, Oklahoma, USA",
            "avatarUrl": "https://image.tmdb.org/t/p/w500/person-600x900.jpg",
            "knownForDepartment": "Acting"
        })
    );
    let credits = include_str!("../tests/fixtures/person_603_combined_credits.json");
    let t = tmdb(move |_| ok(credits));
    let c = run(t.credits(603, &en())).unwrap();
    assert_eq!(asked(&t, 0).0, "/person/603/combined_credits");
    assert_eq!(
        c["cast"][0],
        json!({
            "externalId": 1001, "title": "Parity Movie 01", "mediaType": "movie",
            "character": "Role One",
            "posterUrl": "https://image.tmdb.org/t/p/w500/p1.jpg",
            "fanartUrl": "https://image.tmdb.org/t/p/original/f1.jpg",
            "releaseDate": "2020-01-01", "rating": 7.5
        })
    );
    assert_eq!(c["cast"][1]["title"], "Parity Series B");
    assert_eq!(c["cast"][1]["mediaType"], "series");
    assert_eq!(c["cast"][1]["releaseDate"], "2019-05-05");
    assert_eq!(c["cast"][1]["rating"], 0);
    assert_eq!(c["cast"][2]["releaseDate"], Value::Null);
    assert_eq!(c["cast"][2]["rating"], 0);
    assert_eq!(c["crew"][0]["job"], "Director");
    assert_eq!(c["crew"][0]["department"], "Directing");
    let t = tmdb(|_| ok(r#"{"cast":[]}"#));
    assert!(run(t.credits(1, &en())).is_err());
}

type ListCase<'a> = (bool, &'a str, &'a [(String, String)], &'a str);

#[test]
fn lists_ask_the_right_endpoint() {
    let list = r#"{"results":[{"id":1,"title":"Sample"}]}"#;
    let t = tmdb(move |_| ok(list));
    let none: &[(String, String)] = &[];
    let day = [("window".to_owned(), "day".to_owned())];
    let cases: [ListCase; 6] = [
        (false, "trending", none, "/trending/movie/week"),
        (true, "trending", &day, "/trending/tv/day"),
        (false, "popular", none, "/movie/popular"),
        (true, "popular", none, "/tv/popular"),
        (false, "upcoming", none, "/movie/upcoming"),
        (true, "upcoming", none, "/tv/on_the_air"),
    ];
    for (i, (tv, name, params, path)) in cases.into_iter().enumerate() {
        let v = run(t.list(tv, name, params, &lang("fr", "FR"))).unwrap();
        assert_eq!(v[0]["tmdbId"], 1);
        let (got, query) = asked(&t, i);
        assert_eq!(got, path);
        assert!(has(&query, "language", "fr-FR"));
        assert_eq!(
            has(&query, "region", "FR"),
            path == "/movie/upcoming",
            "{path}"
        );
    }
    assert!(matches!(
        run(t.list(false, "nope", none, &en())),
        Err(ProviderError::Unsupported)
    ));
}

#[test]
fn genres_are_the_raw_list() {
    let body = include_str!("../tests/fixtures/genre_movie_list.json");
    let t = tmdb(move |_| ok(body));
    let v = run(t.list(false, "genres", &[], &en())).unwrap();
    assert_eq!(asked(&t, 0).0, "/genre/movie/list");
    assert_eq!(v[1], json!({"id": 878, "name": "Science Fiction"}));
}

fn pairs(p: &[(&str, &str)]) -> Vec<(String, String)> {
    p.iter()
        .map(|(k, v)| ((*k).to_owned(), (*v).to_owned()))
        .collect()
}

#[test]
fn discover_keeps_the_integers_and_drops_the_nan_and_zero() {
    let t = tmdb(|_| ok(r#"{"results":[]}"#));
    let p = pairs(&[
        ("genres", "28,abc,12"),
        ("sort", "primary_release_date.desc"),
        ("voteGte", "7.5x"),
        ("yearGte", "1999"),
        ("yearLte", "0"),
    ]);
    run(t.list(true, "discover", &p, &en())).unwrap();
    let (path, query) = asked(&t, 0);
    assert_eq!(path, "/discover/tv");
    assert_eq!(
        query,
        pairs(&[
            ("language", "en-US"),
            ("sort_by", "first_air_date.desc"),
            ("vote_count.gte", "50"),
            ("with_genres", "28,12"),
            ("vote_average.gte", "7.5"),
            ("first_air_date.gte", "1999-01-01"),
        ])
    );
    run(t.list(
        false,
        "discover",
        &pairs(&[("genres", "28|12"), ("voteGte", "NaN")]),
        &en(),
    ))
    .unwrap();
    assert_eq!(
        asked(&t, 1).1,
        pairs(&[
            ("language", "en-US"),
            ("include_adult", "false"),
            ("sort_by", "popularity.desc"),
            ("vote_count.gte", "50"),
            ("with_genres", "28"),
        ])
    );
}

#[test]
fn small_mappers() {
    assert_eq!(tv_status("Returning Series"), "continuing");
    assert_eq!(tv_status("Canceled"), "ended");
    assert_eq!(tv_status("In Production"), "announced");
    assert_eq!(tv_status("Planned"), "tba");
    assert_eq!(tv_status("x"), "unknown");
    assert_eq!(crate::map::js_parse_int(" 12abc"), Some(12));
    assert_eq!(crate::map::js_parse_int("abc"), None);
    assert_eq!(crate::map::js_parse_float("7.5x"), Some(7.5));
    assert_eq!(crate::map::js_parse_float(".x"), None);
}

#[test]
fn the_template_declares_the_key_and_what_the_code_serves() {
    let t: Value = serde_json::from_str(include_str!("../plugin.template.json")).unwrap();
    assert_eq!(t["id"], "fliks.tmdb");
    assert_eq!(t["provider"]["requires"], json!(["apiKey"]));
    assert_eq!(t["provider"]["imageHosts"], json!(["image.tmdb.org"]));
    assert_eq!(t["scopes"], json!([]));
    let field = &t["ui"]["configPages"][0]["fields"][0];
    assert_eq!(
        (&field["key"], &field["default"]),
        (&json!("apiKey"), &json!("fliks:apiKey"))
    );
    assert_eq!(field["secret"], true);
}
