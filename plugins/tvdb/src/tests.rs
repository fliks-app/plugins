use crate::tvdb::*;
use fliks_plugin_sdk_provider::ProviderError;
use serde_json::{Value, json};
use std::cell::{Cell, RefCell};
use std::future::Future;
use std::rc::Rc;
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
    key: RefCell<String>,
    logins: Cell<u32>,
    seen: RefCell<Vec<(String, String, Query)>>,
}

impl Http for Fake {
    fn api_key(&self) -> Result<String> {
        Ok(self.key.borrow().clone())
    }

    async fn login(&self, _key: &str) -> Result<Value> {
        self.logins.set(self.logins.get() + 1);
        let (status, retry, body) = (self.handler)("/login");
        reply(status, retry, body.as_bytes())
    }

    async fn get(&self, token: &str, path: &str, query: Query) -> Result<Value> {
        self.seen
            .borrow_mut()
            .push((token.to_owned(), path.to_owned(), query));
        let (status, retry, body) = (self.handler)(path);
        reply(status, retry, body.as_bytes())
    }
}

fn tvdb(handler: impl Fn(&str) -> Reply + 'static) -> Tvdb<Fake> {
    Tvdb {
        http: Fake {
            handler: Box::new(handler),
            key: RefCell::new("k1".into()),
            logins: Cell::new(0),
            seen: RefCell::default(),
        },
        auth: Rc::default(),
    }
}

fn ok(body: &str) -> Reply {
    (200, None, body.to_owned())
}

const LOGIN: &str = r#"{"data":{"token":"tok"}}"#;
const SEARCH: &str = include_str!("../tests/fixtures/search.json");
const SERIES: &str = include_str!("../tests/fixtures/series_2000_extended.json");
const EPISODES: &str = include_str!("../tests/fixtures/series_2000_episodes_default.json");
const TRANSLATION: &str = include_str!("../tests/fixtures/episode_1_translation_fra.json");
const MOVIE: &str = include_str!("../tests/fixtures/movie_1001_extended.json");
const PERSON_7: &str = include_str!("../tests/fixtures/person_777_extended.json");
const PERSON_5: &str = include_str!("../tests/fixtures/person_5555_extended.json");

fn asked(t: &Tvdb<Fake>, i: usize) -> (String, String, Query) {
    t.http.seen.borrow()[i].clone()
}

fn has(query: &Query, k: &str, v: &str) -> bool {
    query.iter().any(|(a, b)| a == k && b == v)
}

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
    let e = reply(500, None, br#"{"message":"boom"}"#).unwrap_err();
    assert!(matches!(&e, ProviderError::Upstream(u)
        if u.status == Some(500) && u.message == "HTTP 500: boom"));
    let e = reply(502, None, b"<html>").unwrap_err();
    assert!(matches!(&e, ProviderError::Upstream(u)
        if u.message == "HTTP 502: Request failed with status code 502"));
    let e = reply(200, None, b"<html>").unwrap_err();
    assert!(matches!(&e, ProviderError::Upstream(u)
        if u.status == Some(200) && u.message == "invalid JSON response"));
}

#[test]
fn the_language_code_follows_the_server_table() {
    assert_eq!(code("fr"), "fra");
    assert_eq!(code("fr-CA"), "fra");
    assert_eq!(code("ZH"), "zho");
    assert_eq!(code("en"), "eng");
    assert_eq!(code("xx"), "eng");
}

#[test]
fn the_token_is_fetched_once_and_sent_as_the_bearer() {
    let t = tvdb(|p| if p == "/login" { ok(LOGIN) } else { ok(SEARCH) });
    run(t.search_titles(true, "sample", None, "eng")).unwrap();
    run(t.search_titles(true, "sample", None, "eng")).unwrap();
    assert_eq!(t.http.logins.get(), 1);
    assert_eq!(asked(&t, 1).0, "tok");
}

#[test]
fn a_changed_key_logs_in_again() {
    let t = tvdb(|p| if p == "/login" { ok(LOGIN) } else { ok(SEARCH) });
    run(t.search_titles(true, "sample", None, "eng")).unwrap();
    *t.http.key.borrow_mut() = "k2".into();
    run(t.search_titles(true, "sample", None, "eng")).unwrap();
    assert_eq!(t.http.logins.get(), 2);
}

#[test]
fn a_refused_token_logs_in_again_once() {
    let hits = Cell::new(0);
    let t = tvdb(move |p| match p {
        "/login" => ok(LOGIN),
        _ => {
            hits.set(hits.get() + 1);
            if hits.get() == 1 {
                (401, None, "{}".into())
            } else {
                ok(SEARCH)
            }
        }
    });
    run(t.search_titles(true, "sample", None, "eng")).unwrap();
    assert_eq!(t.http.logins.get(), 2);
    let t = tvdb(|p| {
        if p == "/login" {
            ok(LOGIN)
        } else {
            (401, None, "{}".into())
        }
    });
    let e = run(t.search_titles(true, "sample", None, "eng")).unwrap_err();
    assert!(matches!(e, ProviderError::Unauthorized));
    assert_eq!(t.http.logins.get(), 2);
}

#[test]
fn a_login_without_a_token_fails() {
    let t = tvdb(|_| ok(r#"{"data":{}}"#));
    let e = run(t.search_titles(true, "x", None, "eng")).unwrap_err();
    assert!(matches!(e, ProviderError::Upstream(u) if u.status.is_none()));
}

#[test]
fn search_maps_the_results_and_passes_the_year() {
    let t = tvdb(|p| if p == "/login" { ok(LOGIN) } else { ok(SEARCH) });
    let found = run(t.search_titles(true, "sample", Some(2008), "fra")).unwrap();
    let (_, path, query) = asked(&t, 0);
    assert_eq!(path, "/search");
    assert!(has(&query, "q", "sample") && has(&query, "type", "series"));
    assert!(has(&query, "year", "2008"));
    assert_eq!(found.len(), 2);
    assert_eq!(
        found[0],
        json!({
            "tmdbId": 3000, "tvdbId": 2000, "imdbId": "tt0000001", "provider": "tvdb",
            "title": "Sample Show FR", "originalTitle": "Sample Show",
            "overview": "Resume fr", "year": 2008,
            "posterUrl": "https://artworks.thetvdb.com/p.jpg", "rating": 0,
            "genres": ["Drama"], "mediaType": "series"
        })
    );
    assert_eq!(
        found[1],
        json!({
            "tmdbId": 0, "tvdbId": 999, "imdbId": null, "provider": "tvdb",
            "title": "Other Show", "originalTitle": "Other Show", "overview": "", "year": null,
            "posterUrl": "https://artworks.thetvdb.com/o.jpg", "rating": 0,
            "genres": [], "mediaType": "series"
        })
    );
    let t = tvdb(|p| {
        if p == "/login" {
            ok(LOGIN)
        } else {
            ok(r#"{"data":[]}"#)
        }
    });
    assert!(
        run(t.search_titles(false, "x", None, "eng"))
            .unwrap()
            .is_empty()
    );
    assert!(has(&asked(&t, 0).2, "type", "movie"));
}

#[test]
fn search_drops_the_missing_artwork_placeholder() {
    let t = tvdb(|p| {
        if p == "/login" {
            ok(LOGIN)
        } else {
            ok(
                r#"{"data":[{"tvdb_id":"7","name":"No Art","image_url":"https://artworks.thetvdb.com/banners/images/missing/series.jpg"}]}"#,
            )
        }
    });
    let found = run(t.search_titles(true, "x", None, "eng")).unwrap();
    assert_eq!(found[0]["posterUrl"], Value::Null);
}

#[test]
fn series_details_follow_the_server_mapping() {
    let t = tvdb(|p| if p == "/login" { ok(LOGIN) } else { ok(SERIES) });
    let d = run(t.details(true, 2000, "fra")).unwrap();
    let (_, path, query) = asked(&t, 0);
    assert_eq!(path, "/series/2000/extended");
    assert!(has(&query, "meta", "translations"));
    let img = "https://artworks.thetvdb.com";
    assert_eq!(d["tmdbId"], 3000);
    assert_eq!(
        (&d["tvdbId"], &d["imdbId"]),
        (&json!(2000), &json!("tt0000001"))
    );
    assert_eq!(d["title"], "Sample Show (fr)");
    assert_eq!(d["originalTitle"], "Sample Show");
    assert_eq!(d["overview"], "Un professeur.");
    assert_eq!(d["year"], 2008);
    assert_eq!(d["posterUrl"], format!("{img}/poster.jpg"));
    assert_eq!(d["fanartUrl"], format!("{img}/bg2.jpg"));
    assert_eq!(d["additionalFanartUrls"], json!([format!("{img}/bg1.jpg")]));
    assert_eq!(d["logoUrl"], format!("{img}/logo.png"));
    assert_eq!(d["genres"], json!(["Drama"]));
    assert_eq!(d["keywords"], json!(["crime"]));
    assert_eq!(d["alternativeTitles"], json!(["BB"]));
    assert_eq!(d["mediaType"], "series");
    // Season 0 and the DVD order's season 1 are not aired seasons.
    assert_eq!(d["seasonCount"], 1);
    assert_eq!(d["releaseDate"], "2008-01-20");
    assert_eq!(d["status"], "ended");
    assert_eq!(d["productionCountries"], json!(["usa"]));
    assert_eq!(d["productionCompanies"], json!(["Network A", "Studio A"]));
    assert_eq!(
        d["cast"],
        json!([{"externalId": 1, "name": "Actor One", "character": "Role One",
                "avatarUrl": format!("{img}/bc.jpg"), "order": 1}])
    );
    assert_eq!(
        d["crew"],
        json!([{"externalId": 2, "name": "Creator One", "job": "Creator",
                "department": "Creator", "avatarUrl": null}])
    );
    assert_eq!(
        d["videos"],
        json!([
            {"key": "abcdefghijk", "site": "YouTube", "type": "Trailer", "name": "Trailer"},
            {"key": "", "site": "Unknown", "type": "Trailer", "name": "Empty"}
        ])
    );
    let en = map_details(
        &serde_json::from_str::<Value>(SERIES).unwrap()["data"],
        true,
        "eng",
    );
    assert_eq!(en["title"], "Sample Show");
    assert_eq!(en["overview"], "");
    assert_eq!(en["fanartUrl"], format!("{img}/bg1.jpg"));
}

#[test]
fn movie_details_follow_the_server_mapping() {
    let t = tvdb(|p| if p == "/login" { ok(LOGIN) } else { ok(MOVIE) });
    let d = run(t.details(false, 1001, "eng")).unwrap();
    assert_eq!(asked(&t, 0).1, "/movies/1001/extended");
    assert_eq!(d["tmdbId"], 5001);
    assert_eq!(d["title"], "Sample Film");
    assert_eq!(d["year"], 1999);
    assert_eq!(d["posterUrl"], "https://artworks.thetvdb.com/mp.jpg");
    assert_eq!(d["mediaType"], "movie");
    assert_eq!(d["runtime"], 136);
    assert_eq!(d["releaseDate"], "1999-03-31");
    assert_eq!(d["inCinemas"], "1999-03-31");
    assert_eq!(d["digitalRelease"], "1999-09-21");
    assert_eq!(d["physicalRelease"], Value::Null);
    assert_eq!(d["status"], "released");
    assert_eq!(
        (&d["budget"], &d["revenue"]),
        (&json!(63000000), &Value::Null)
    );
    assert_eq!(d["productionCompanies"], json!(["Company B", "Studio B"]));
    assert_eq!(d["seasonCount"], Value::Null);
}

#[test]
fn details_without_a_body_fail_like_the_node_client() {
    let t = tvdb(|p| {
        if p == "/login" {
            ok(LOGIN)
        } else {
            ok(r#"{"status":"success"}"#)
        }
    });
    let e = run(t.details(true, 1, "eng")).unwrap_err();
    assert!(matches!(e, ProviderError::Upstream(u) if u.status.is_none()));
}

fn series_with_episodes() -> Tvdb<Fake> {
    tvdb(|p| match p {
        "/login" => ok(LOGIN),
        "/series/2000/episodes/default" => ok(EPISODES),
        "/series/2000/extended" => ok(SERIES),
        "/episodes/1/translations/fra" => ok(TRANSLATION),
        _ => (404, None, "{}".into()),
    })
}

#[test]
fn child_stubs_count_the_episodes_of_each_season() {
    let t = series_with_episodes();
    let stubs = run(t.season_stubs(2000)).unwrap();
    assert_eq!(
        stubs,
        [
            json!({"seasonNumber": 0, "episodeCount": 1}),
            json!({"seasonNumber": 1, "episodeCount": 2})
        ]
    );
    let (_, _, query) = asked(&t, 0);
    assert!(has(&query, "page", "0"));
}

#[test]
fn a_full_page_asks_for_the_next() {
    let ep = r#"{"seasonNumber":1,"number":1}"#;
    let full = format!(r#"{{"data":{{"episodes":[{}]}}}}"#, vec![ep; 500].join(","));
    let t = tvdb(move |p| match p {
        "/login" => ok(LOGIN),
        _ => ok(&full),
    });
    // Every page is full, so it stops at the page cap.
    let stubs = run(t.season_stubs(1)).unwrap();
    assert_eq!(stubs, [json!({"seasonNumber": 1, "episodeCount": 50_000})]);
    assert!(has(&asked(&t, 99).2, "page", "99"));
}

#[test]
fn seasons_pick_the_translation_and_fall_back_to_the_episode() {
    let t = series_with_episodes();
    let seasons = run(t.seasons(2000, None, "fra")).unwrap();
    assert_eq!(seasons.len(), 2);
    assert_eq!(
        seasons[1],
        json!({
            "seasonNumber": 1, "episodeCount": 2, "overview": null, "airDate": "2008-01-20",
            "posterUrl": null,
            "episodes": [
                {"episodeNumber": 1, "title": "Pilote", "overview": "Le pilote.",
                 "airDate": "2008-01-20", "runtime": 58,
                 "stillUrl": "https://artworks.thetvdb.com/e1.jpg"},
                {"episodeNumber": 2, "title": "Second Episode", "overview": null,
                 "airDate": "2008-01-27", "runtime": 48, "stillUrl": null}
            ]
        })
    );
    assert_eq!(seasons[0]["airDate"], Value::Null);
    assert_eq!(seasons[0]["episodes"][0]["overview"], Value::Null);
}

#[test]
fn one_season_asks_for_it_and_keeps_only_its_episodes() {
    let t = series_with_episodes();
    let seasons = run(t.seasons(2000, Some(1), "fra")).unwrap();
    assert_eq!(seasons.len(), 1);
    assert_eq!(seasons[0]["seasonNumber"], 1);
    let (_, _, query) = asked(&t, 0);
    assert!(has(&query, "season", "1"));
}

#[test]
fn a_failing_translation_keeps_the_episode_untranslated() {
    let t = tvdb(|p| match p {
        "/login" => ok(LOGIN),
        "/series/2000/episodes/default" => ok(EPISODES),
        "/series/2000/extended" => ok(SERIES),
        _ => (500, None, "{}".into()),
    });
    let seasons = run(t.seasons(2000, None, "fra")).unwrap();
    assert_eq!(seasons[1]["episodes"][0]["title"], "Pilot");
}

#[test]
fn find_by_answers_the_first_series_then_movie() {
    let t = tvdb(|p| match p {
        "/login" => ok(LOGIN),
        _ => ok(r#"{"data":[{"movie":{"id":7}},{"series":{"id":"8"}}]}"#),
    });
    let found = run(t.find_by_imdb("tt0000001")).unwrap();
    assert_eq!(found, Some(("7".into(), "movie")));
    assert_eq!(asked(&t, 0).1, "/search/remoteid/tt0000001");
    let t = tvdb(|p| match p {
        "/login" => ok(LOGIN),
        _ => ok(r#"{"data":[{"series":{"id":8},"movie":{"id":7}}]}"#),
    });
    assert_eq!(
        run(t.find_by_imdb("tt0000001")).unwrap(),
        Some(("8".into(), "series"))
    );
}

#[test]
fn find_by_swallows_a_failed_lookup_but_not_a_failed_login() {
    let t = tvdb(|p| {
        if p == "/login" {
            ok(LOGIN)
        } else {
            (500, None, "{}".into())
        }
    });
    assert_eq!(run(t.find_by_imdb("tt1")).unwrap(), None);
    let t = tvdb(|_| (401, None, "{}".into()));
    assert!(matches!(
        run(t.find_by_imdb("tt1")),
        Err(ProviderError::Unauthorized)
    ));
}

#[test]
fn a_person_and_credits_follow_the_server_mapping() {
    let t = tvdb(|p| match p {
        "/login" => ok(LOGIN),
        "/people/777/extended" => ok(PERSON_7),
        _ => ok(PERSON_5),
    });
    let p = run(t.person(777, "fra")).unwrap();
    assert_eq!(
        p,
        json!({
            "name": "Person Seven", "biography": "English bio.", "birthday": "1970-01-02",
            "deathday": null, "placeOfBirth": "Lyon", "avatarUrl": null,
            "knownForDepartment": "Acting"
        })
    );
    let fr = run(t.person(5555, "fra")).unwrap();
    assert_eq!(fr["biography"], "Bio fr.");
    assert_eq!(fr["knownForDepartment"], "Writing");
    let c = run(t.credits(777)).unwrap();
    assert_eq!(
        c["cast"],
        json!([{
            "externalId": 2000, "title": "Sample Show", "mediaType": "series", "posterUrl": null,
            "releaseDate": "2008-01-01", "rating": 0, "character": "Role"
        }])
    );
    assert_eq!(
        c["crew"],
        json!([{
            "externalId": 1001, "title": "Sample Film", "mediaType": "movie", "posterUrl": null,
            "releaseDate": "1999-01-01", "rating": 0, "job": "Director", "department": "Director"
        }])
    );
}

#[test]
fn small_mappers() {
    assert_eq!(video_key("https://youtu.be/abcdefghijk?t=1"), "abcdefghijk");
    assert_eq!(video_key("https://example.org/x"), "https://example.org/x");
    assert_eq!(infer_department(&[]), "Acting");
    assert_eq!(
        infer_department(&[
            json!({"peopleType": "Director"}),
            json!({"peopleType": "Writer"})
        ]),
        "Directing"
    );
}

#[test]
fn the_template_declares_the_key_and_what_the_code_serves() {
    let t: Value = serde_json::from_str(include_str!("../plugin.template.json")).unwrap();
    assert_eq!(t["id"], "fliks.tvdb");
    assert_eq!(t["provider"]["requires"], json!(["apiKey"]));
    assert_eq!(t["provider"]["imageHosts"], json!(["artworks.thetvdb.com"]));
    assert_eq!(t["provider"]["findBy"], json!(["imdb"]));
    assert_eq!(t["scopes"], json!([]));
    let field = &t["ui"]["configPages"][0]["fields"][0];
    assert_eq!(
        (&field["key"], &field["default"]),
        (&json!("apiKey"), &json!("fliks:apiKey"))
    );
    assert_eq!(field["secret"], true);
}
