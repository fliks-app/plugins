//! TheTVDB v4 requests and response mapping, over an `Http` that answers parsed JSON.
use crate::map::{arr, get, js_parse_int, nn, or, pluck, put, score, str_lower_or, text, year_of};
use fliks_plugin_sdk::rt::join_all;
use fliks_plugin_sdk_provider::ProviderError;
use provider_common::lang::normalize_language_code;
use serde_json::{Map, Value, json};
use std::cell::RefCell;
use std::collections::BTreeMap;
use std::time::{Duration, Instant};

const ART_POSTER: i64 = 2;
const ART_BACKGROUND: i64 = 3;
const ART_CLEARLOGO_SERIES: i64 = 23;
const ART_CLEARLOGO_MOVIE: i64 = 25;
const EPISODE_TRANSLATION_CONCURRENCY: usize = 8;
/// 50 000 episodes: far above any series, stops an upstream that never returns a short page.
const MAX_EPISODE_PAGES: u32 = 100;
/// Valid 30 days, refreshed after 25.
const TOKEN_TTL: Duration = Duration::from_secs(25 * 24 * 3600);

pub type Query = Vec<(String, String)>;
pub type Result<T> = std::result::Result<T, ProviderError>;

/// The TVDB calls: the configured key, the login, and a GET under `/v4` with the bearer token.
pub trait Http {
    fn api_key(&self) -> Result<String>;
    async fn login(&self, key: &str) -> Result<Value>;
    async fn get(&self, token: &str, path: &str, query: Query) -> Result<Value>;
}

pub use provider_common::http::{json_reply as reply, upstream};

/// The metadata language in TheTVDB's three-letter code; an unknown one is English.
pub fn code(language: &str) -> &'static str {
    match normalize_language_code(Some(language)).as_str() {
        "fr" => "fra",
        "es" => "spa",
        "de" => "deu",
        "it" => "ita",
        "pt" => "por",
        "nl" => "nld",
        "pl" => "pol",
        "ru" => "rus",
        "ja" => "jpn",
        "ko" => "kor",
        "zh" => "zho",
        _ => "eng",
    }
}

fn missing(what: &str) -> ProviderError {
    upstream(
        None,
        format!("Cannot read properties of undefined (reading '{what}')"),
    )
}

/// `parseInt(String(x), 10)` as JSON: NaN is `null`.
fn parse_int(v: &Value) -> Value {
    let s = match v {
        Value::String(s) => s.clone(),
        Value::Null => return Value::Null,
        o => o.to_string(),
    };
    js_parse_int(&s).map_or(Value::Null, Value::from)
}

fn remote_id(remote_ids: &Value, source: &str) -> Option<Value> {
    remote_ids
        .as_array()?
        .iter()
        .find(|r| get(r, "sourceName") == source)
        .map(|r| nn(r, "id"))
        .filter(|v| !v.is_null())
}

struct Token {
    key: String,
    value: String,
    expires: Instant,
}

/// The login token of the instance, shared by every call it serves.
#[derive(Default)]
pub struct Auth(RefCell<Option<Token>>);

pub struct Tvdb<H> {
    pub http: H,
    pub auth: std::rc::Rc<Auth>,
}

impl<H: Http> Tvdb<H> {
    async fn token(&self, stale: Option<&str>) -> Result<String> {
        let key = self.http.api_key()?;
        if let Some(t) = self.auth.0.borrow().as_ref()
            && t.key == key
            && t.expires > Instant::now()
            && stale != Some(t.value.as_str())
        {
            return Ok(t.value.clone());
        }
        let data = self.http.login(&key).await?;
        let value = get(get(&data, "data"), "token")
            .as_str()
            .ok_or_else(|| missing("token"))?
            .to_owned();
        *self.auth.0.borrow_mut() = Some(Token {
            key,
            value: value.clone(),
            expires: Instant::now() + TOKEN_TTL,
        });
        Ok(value)
    }

    /// The body of an authenticated GET; one re-login when the token is refused (401).
    async fn get(&self, path: &str, query: &[(&str, String)]) -> Result<Value> {
        let query: Query = query
            .iter()
            .map(|(k, v)| ((*k).to_owned(), v.clone()))
            .collect();
        let token = self.token(None).await?;
        match self.http.get(&token, path, query.clone()).await {
            Err(ProviderError::Unauthorized) => {
                let token = self.token(Some(&token)).await?;
                self.http.get(&token, path, query).await
            }
            other => other,
        }
    }

    async fn data(&self, path: &str, query: &[(&str, String)]) -> Result<Value> {
        Ok(nn(&self.get(path, query).await?, "data"))
    }

    pub async fn search_titles(
        &self,
        tv: bool,
        query: &str,
        year: Option<i32>,
        code: &str,
    ) -> Result<Vec<Value>> {
        let mut params = vec![("q", query.to_owned()), ("type", kind_name(tv).into())];
        if let Some(y) = year {
            params.push(("year", y.to_string()));
        }
        let d = self.data("/search", &params).await?;
        Ok(d.as_array()
            .map(|a| a.iter().map(|r| map_search_result(r, tv, code)).collect())
            .unwrap_or_default())
    }

    pub async fn details(&self, tv: bool, id: i64, code: &str) -> Result<Value> {
        let path = if tv {
            format!("/series/{id}/extended")
        } else {
            format!("/movies/{id}/extended")
        };
        let m = self.data(&path, &[("meta", "translations".into())]).await?;
        if m.is_null() {
            return Err(missing("translations"));
        }
        Ok(map_details(&m, tv, code))
    }

    /// Every episode, following the 500-per-page cursor (a full page asks for the next).
    async fn all_episodes(&self, id: i64, season: Option<i64>) -> Result<Vec<Value>> {
        let mut all = vec![];
        for page in 0..MAX_EPISODE_PAGES {
            let mut query = vec![("page", page.to_string())];
            query.extend(season.map(|n| ("season", n.to_string())));
            let d = self
                .data(&format!("/series/{id}/episodes/default"), &query)
                .await?;
            let eps = arr(&d, "episodes");
            all.extend(eps.iter().cloned());
            if eps.len() < 500 {
                break;
            }
        }
        Ok(all)
    }

    pub async fn season_stubs(&self, id: i64) -> Result<Vec<Value>> {
        let mut counts: BTreeMap<i64, i64> = BTreeMap::new();
        for ep in self.all_episodes(id, None).await? {
            *counts
                .entry(get(&ep, "seasonNumber").as_i64().unwrap_or_default())
                .or_default() += 1;
        }
        Ok(counts
            .into_iter()
            .map(|(n, c)| json!({"seasonNumber": n, "episodeCount": c}))
            .collect())
    }

    /// With `only`, just that season: the episode list is asked for it and its translations alone are read.
    pub async fn seasons(&self, id: i64, only: Option<i64>, code: &str) -> Result<Vec<Value>> {
        let mut episodes = self.all_episodes(id, only).await?;
        if let Some(n) = only {
            episodes.retain(|e| get(e, "seasonNumber").as_i64() == Some(n));
        }
        let series = self
            .data(
                &format!("/series/{id}/extended"),
                &[("short", "true".into())],
            )
            .await?;
        let bases: Vec<&Value> = arr(&series, "seasons")
            .iter()
            .filter(|s| {
                get(get(s, "type"), "type") == "official"
                    || get(get(s, "type"), "name") == "Aired Order"
            })
            .collect();
        let advertises = |ep: &Value| {
            ["overviewTranslations", "nameTranslations"]
                .iter()
                .any(|k| arr(ep, k).iter().any(|c| c == code))
        };
        let wanted: Vec<i64> = episodes
            .iter()
            .filter(|e| advertises(e))
            .filter_map(|e| get(e, "id").as_i64())
            .collect();
        let mut translations: BTreeMap<i64, Value> = BTreeMap::new();
        for chunk in wanted.chunks(EPISODE_TRANSLATION_CONCURRENCY) {
            let fetched = join_all(chunk.iter().map(|&eid| async move {
                let d = self
                    .data(&format!("/episodes/{eid}/translations/{code}"), &[])
                    .await
                    .ok()?;
                Some((eid, d))
            }))
            .await;
            translations.extend(fetched.into_iter().flatten());
        }
        let mut by_season: BTreeMap<i64, Vec<&Value>> = BTreeMap::new();
        for ep in &episodes {
            by_season
                .entry(get(ep, "seasonNumber").as_i64().unwrap_or_default())
                .or_default()
                .push(ep);
        }
        Ok(by_season
            .into_iter()
            .map(|(n, mut eps)| {
                let base = bases.iter().find(|s| get(s, "number").as_i64() == Some(n));
                eps.sort_by_key(|e| get(e, "number").as_i64().unwrap_or_default());
                let air = base
                    .map(|b| nn(b, "year"))
                    .filter(|v| !v.is_null())
                    .or_else(|| eps.first().map(|e| nn(e, "aired")).filter(|v| !v.is_null()))
                    .unwrap_or(Value::Null);
                let episodes: Vec<Value> = eps
                    .iter()
                    .map(|ep| {
                        let tr = get(ep, "id").as_i64().and_then(|i| translations.get(&i));
                        let pick = |k: &str| tr.and_then(|t| text(t, k)).or_else(|| text(ep, k));
                        json!({
                            "episodeNumber": nn(ep, "number"),
                            "title": pick("name").map_or_else(|| nn(ep, "name"), Value::from),
                            "overview": pick("overview").map_or(Value::Null, Value::from),
                            "airDate": nn(ep, "aired"),
                            "runtime": nn(ep, "runtime"),
                            "stillUrl": nn(ep, "image"),
                        })
                    })
                    .collect();
                json!({
                    "seasonNumber": n,
                    "episodeCount": episodes.len(),
                    "overview": null,
                    "airDate": air,
                    "posterUrl": base.and_then(|b| text(b, "image")).map_or(Value::Null, Value::from),
                    "episodes": episodes,
                })
            })
            .collect())
    }

    pub async fn person(&self, id: i64, code: &str) -> Result<Value> {
        let person = self
            .data(
                &format!("/people/{id}/extended"),
                &[("meta", "translations".into())],
            )
            .await?;
        let bio = |code: &str| {
            arr(&person, "biographies")
                .iter()
                .find(|b| get(b, "language") == code)
                .and_then(|b| get(b, "biography").as_str())
        };
        let s = |k: &str| get(&person, k).as_str().map(str::to_owned);
        Ok(json!({
            "name": s("name"),
            "biography": bio(code).or_else(|| bio("eng")).unwrap_or_default(),
            "birthday": s("birth"),
            "deathday": s("death"),
            "placeOfBirth": s("birthPlace"),
            "avatarUrl": s("image"),
            "knownForDepartment": infer_department(arr(&person, "characters")),
        }))
    }

    pub async fn credits(&self, id: i64) -> Result<Value> {
        let person = self.data(&format!("/people/{id}/extended"), &[]).await?;
        let chars = arr(&person, "characters");
        let credit = |c: &Value| {
            let work = match get(c, "series") {
                Value::Null => get(c, "movie"),
                w => w,
            };
            let external = [get(c, "seriesId"), get(c, "movieId")]
                .into_iter()
                .find(|v| !v.is_null())
                .cloned()
                .unwrap_or(json!(0));
            let poster = match get(work, "image") {
                Value::Null => nn(c, "image"),
                i => i.clone(),
            };
            let mut o = Map::new();
            o.insert("externalId".into(), external);
            o.insert(
                "title".into(),
                get(work, "name").as_str().unwrap_or_default().into(),
            );
            let is_movie = get(c, "movieId").as_i64().is_some_and(|n| n != 0);
            o.insert("mediaType".into(), kind_name(!is_movie).into());
            o.insert("posterUrl".into(), poster);
            let year = match get(work, "year") {
                Value::String(y) if !y.is_empty() => Some(y.clone()),
                Value::Number(n) if n.as_f64() != Some(0.0) => Some(n.to_string()),
                _ => None,
            };
            o.insert(
                "releaseDate".into(),
                year.map_or(Value::Null, |y| json!(format!("{y}-01-01"))),
            );
            o.insert("rating".into(), json!(0));
            o
        };
        let acting =
            |c: &&Value| matches!(get(c, "peopleType").as_str(), Some("Actor" | "Guest Star"));
        let cast: Vec<Value> = chars
            .iter()
            .filter(acting)
            .map(|c| {
                let mut o = credit(c);
                put(&mut o, "character", c, "name");
                Value::Object(o)
            })
            .collect();
        let crew: Vec<Value> = chars
            .iter()
            .filter(|c| !acting(c))
            .map(|c| {
                let mut o = credit(c);
                put(&mut o, "job", c, "peopleType");
                put(&mut o, "department", c, "peopleType");
                Value::Object(o)
            })
            .collect();
        Ok(json!({"cast": cast, "crew": crew}))
    }

    /// The first `series` then `movie` of the remote-id lookup; a failed call is `None`, a failed
    /// login is not.
    pub async fn find_by_imdb(&self, id: &str) -> Result<Option<(String, &'static str)>> {
        self.token(None).await?;
        let Ok(d) = self.data(&format!("/search/remoteid/{id}"), &[]).await else {
            return Ok(None);
        };
        for r in d.as_array().map(Vec::as_slice).unwrap_or_default() {
            for tv in [true, false] {
                let w = get(r, kind_name(tv));
                if !w.is_null() {
                    return Ok(id_string(get(w, "id")).map(|i| (i, kind_name(tv))));
                }
            }
        }
        Ok(None)
    }
}

fn kind_name(tv: bool) -> &'static str {
    if tv { "series" } else { "movie" }
}

/// JS `String(n)` of an id; a missing one is no id.
fn id_string(v: &Value) -> Option<String> {
    match v {
        Value::Null => None,
        Value::String(s) => Some(s.clone()),
        other => Some(other.to_string()),
    }
}

pub fn infer_department(chars: &[Value]) -> String {
    let types: Vec<Option<&str>> = chars
        .iter()
        .map(|c| get(c, "peopleType").as_str())
        .collect();
    let has = |t: &str| types.contains(&Some(t));
    if chars.is_empty() || has("Actor") {
        "Acting".into()
    } else if has("Director") {
        "Directing".into()
    } else if has("Writer") {
        "Writing".into()
    } else {
        types[0].unwrap_or("Acting").to_owned()
    }
}

pub fn map_search_result(r: &Value, tv: bool, code: &str) -> Value {
    let ids = get(r, "remote_ids");
    let by_code = |k: &str| match get(get(r, k), code) {
        Value::Null => None,
        x => Some(x.clone()),
    };
    let tvdb_id = match get(r, "tvdb_id") {
        Value::Null => get(r, "id"),
        x => x,
    };
    let poster = [get(r, "image_url"), get(r, "poster")]
        .into_iter()
        .find(|v| !v.is_null())
        .cloned()
        .unwrap_or(Value::Null);
    json!({
        "tmdbId": parse_int(&remote_id(ids, "TheMovieDB.com").unwrap_or(json!("0"))),
        "tvdbId": parse_int(tvdb_id),
        "imdbId": remote_id(ids, "IMDB").unwrap_or(Value::Null),
        "provider": "tvdb",
        "title": by_code("translations").unwrap_or_else(|| nn(r, "name")),
        "originalTitle": nn(r, "name"),
        "overview": by_code("overviews").or_else(|| Some(nn(r, "overview")).filter(|v| !v.is_null())).unwrap_or(json!("")),
        "year": text(r, "year").and_then(js_parse_int).map_or(Value::Null, Value::from),
        "posterUrl": poster,
        "rating": 0,
        "genres": or(r, "genres", json!([])),
        "mediaType": kind_name(tv),
    })
}

/// The translation of `code`: its name, overview and tagline; `None` when neither exists.
fn pick_translation(
    t: &Value,
    code: &str,
) -> Option<(Option<String>, Option<String>, Option<String>)> {
    let find = |k: &str| arr(t, k).iter().find(|x| get(x, "language") == code);
    let (name, overview) = (find("nameTranslations"), find("overviewTranslations"));
    if name.is_none() && overview.is_none() {
        return None;
    }
    let s = |v: Option<&Value>, k: &str| v.and_then(|v| get(v, k).as_str()).map(str::to_owned);
    Some((
        s(name, "name"),
        s(overview, "overview"),
        s(name, "tagline").or_else(|| s(overview, "tagline")),
    ))
}

/// Configured language first, then by score; `None` when the type has no artwork.
fn pick_artwork(artworks: &Value, kind: i64, code: &str) -> Option<String> {
    let mut a: Vec<&Value> = artworks
        .as_array()?
        .iter()
        .filter(|a| get(a, "type") == kind)
        .collect();
    a.sort_by(|x, y| {
        let (xl, yl) = (get(x, "language") == code, get(y, "language") == code);
        yl.cmp(&xl)
            .then(score(y, "score").total_cmp(&score(x, "score")))
    });
    a.first()
        .and_then(|a| get(a, "image").as_str())
        .map(str::to_owned)
}

fn pick_artworks(artworks: &Value, kind: i64, n: usize, skip: Option<&str>) -> Vec<Value> {
    let mut a: Vec<&Value> = arr_of(artworks)
        .iter()
        .filter(|a| get(a, "type") == kind && text(a, "image").is_some_and(|i| Some(i) != skip))
        .collect();
    a.sort_by(|x, y| score(y, "score").total_cmp(&score(x, "score")));
    a.into_iter().take(n).map(|a| nn(a, "image")).collect()
}

fn arr_of(v: &Value) -> &[Value] {
    v.as_array().map(Vec::as_slice).unwrap_or_default()
}

fn release_date(releases: &Value, kind: &str) -> Option<Value> {
    arr_of(releases)
        .iter()
        .find(|r| {
            get(r, "detail")
                .as_str()
                .is_some_and(|d| d.to_lowercase().contains(&kind.to_lowercase()))
        })
        .map(|r| nn(r, "date"))
        .filter(|v| !v.is_null())
}

/// The 11-character YouTube id after `v=` or `youtu.be/`, else the whole URL.
pub fn video_key(url: &str) -> String {
    let id = |s: &str| {
        let t: String = s.chars().take(11).collect();
        (t.chars().count() == 11
            && t.chars()
                .all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-'))
        .then_some(t)
    };
    for (i, _) in url.char_indices() {
        let rest = &url[i..];
        for prefix in ["v=", "youtu.be/"] {
            if let Some(found) = rest.strip_prefix(prefix).and_then(id) {
                return found;
            }
        }
    }
    url.to_owned()
}

fn aliases(list: &Value, canonical: &str) -> Vec<Value> {
    let mut seen = vec![canonical.to_lowercase()];
    let mut out = vec![];
    for a in arr_of(list) {
        let t = get(a, "name").as_str().unwrap_or_default().trim();
        if t.is_empty() || seen.contains(&t.to_lowercase()) {
            continue;
        }
        seen.push(t.to_lowercase());
        out.push(Value::from(t));
    }
    out
}

fn cast_of(chars: &Value) -> Vec<Value> {
    let mut c: Vec<&Value> = arr_of(chars)
        .iter()
        .filter(|c| matches!(get(c, "peopleType").as_str(), Some("Actor" | "Guest Star")))
        .collect();
    c.sort_by(|a, b| {
        get(a, "sort")
            .as_f64()
            .unwrap_or(999.0)
            .total_cmp(&get(b, "sort").as_f64().unwrap_or(999.0))
    });
    c.into_iter()
        .enumerate()
        .map(|(i, c)| {
            json!({
                "externalId": nn(c, "peopleId"), "name": nn(c, "personName"), "character": nn(c, "name"),
                "avatarUrl": nn(c, "personImgURL"),
                "order": or(c, "sort", json!(i)),
            })
        })
        .collect()
}

fn crew_of(chars: &Value) -> Vec<Value> {
    arr_of(chars)
        .iter()
        .filter(|c| text(c, "peopleType").is_some_and(|t| t != "Actor" && t != "Guest Star"))
        .map(|c| {
            json!({
                "externalId": nn(c, "peopleId"), "name": nn(c, "personName"), "job": nn(c, "peopleType"),
                "department": nn(c, "peopleType"), "avatarUrl": nn(c, "personImgURL"),
            })
        })
        .collect()
}

fn series_status(s: &str) -> &'static str {
    match s {
        "Continuing" => "continuing",
        "Ended" => "ended",
        "Upcoming" => "announced",
        _ => "unknown",
    }
}

pub fn map_details(m: &Value, tv: bool, code: &str) -> Value {
    let loc = pick_translation(get(m, "translations"), code);
    let remote = get(m, "remoteIds");
    let art = get(m, "artworks");
    let fanart = pick_artwork(art, ART_BACKGROUND, code);
    let opt = |s: Option<String>| s.map_or(Value::Null, Value::from);
    let mut o = Map::new();
    o.insert(
        "tmdbId".into(),
        parse_int(&remote_id(remote, "TheMovieDB.com").unwrap_or(json!("0"))),
    );
    o.insert("tvdbId".into(), nn(m, "id"));
    o.insert(
        "imdbId".into(),
        remote_id(remote, "IMDB").unwrap_or(Value::Null),
    );
    o.insert("provider".into(), "tvdb".into());
    let localized_name = loc
        .as_ref()
        .and_then(|l| l.0.clone())
        .filter(|n| !n.is_empty());
    o.insert(
        "title".into(),
        localized_name.map_or_else(|| nn(m, "name"), Value::from),
    );
    o.insert("originalTitle".into(), nn(m, "name"));
    o.insert(
        "overview".into(),
        loc.as_ref()
            .and_then(|l| l.1.clone())
            .unwrap_or_default()
            .into(),
    );
    o.insert("year".into(), year_of(m, "year"));
    let poster = pick_artwork(art, ART_POSTER, code).map_or_else(|| nn(m, "image"), Value::from);
    o.insert("posterUrl".into(), poster);
    o.insert("fanartUrl".into(), opt(fanart.clone()));
    let logo = if tv {
        ART_CLEARLOGO_SERIES
    } else {
        ART_CLEARLOGO_MOVIE
    };
    o.insert("logoUrl".into(), opt(pick_artwork(art, logo, code)));
    o.insert(
        "additionalFanartUrls".into(),
        pick_artworks(art, ART_BACKGROUND, 5, fanart.as_deref()).into(),
    );
    o.insert("rating".into(), json!(0));
    o.insert("genres".into(), pluck(arr(m, "genres"), "name").into());
    o.insert("mediaType".into(), kind_name(tv).into());
    o.insert("voteCount".into(), Value::Null);
    o.insert("popularity".into(), Value::Null);
    o.insert("tagline".into(), opt(loc.and_then(|l| l.2)));
    o.insert("cast".into(), cast_of(get(m, "characters")).into());
    o.insert("crew".into(), crew_of(get(m, "characters")).into());
    o.insert("videos".into(), trailers(m).into());
    o.insert(
        "keywords".into(),
        pluck(arr(m, "tagOptions"), "name").into(),
    );
    o.insert(
        "alternativeTitles".into(),
        aliases(
            get(m, "aliases"),
            get(m, "name").as_str().unwrap_or_default(),
        )
        .into(),
    );
    o.insert("tmdbCollectionId".into(), Value::Null);
    o.insert("tmdbCollectionName".into(), Value::Null);
    o.insert("originalLanguage".into(), nn(m, "originalLanguage"));
    o.insert("episodeCount".into(), Value::Null);
    if tv {
        series_fields(&mut o, m);
    } else {
        movie_fields(&mut o, m);
    }
    Value::Object(o)
}

fn trailers(m: &Value) -> Vec<Value> {
    arr(m, "trailers")
        .iter()
        .map(|t| {
            let url = get(t, "url").as_str().unwrap_or_default();
            let mut v = Map::new();
            v.insert("key".into(), video_key(url).into());
            v.insert(
                "site".into(),
                if url.contains("youtube") {
                    "YouTube"
                } else {
                    "Unknown"
                }
                .into(),
            );
            v.insert("type".into(), "Trailer".into());
            put(&mut v, "name", t, "name");
            Value::Object(v)
        })
        .collect()
}

fn series_fields(o: &mut Map<String, Value>, m: &Value) {
    let count = arr(m, "seasons")
        .iter()
        .filter(|s| get(s, "number") != 0)
        .count();
    o.insert(
        "seasonCount".into(),
        if count == 0 {
            Value::Null
        } else {
            count.into()
        },
    );
    o.insert("runtime".into(), Value::Null);
    o.insert("releaseDate".into(), nn(m, "firstAired"));
    for k in [
        "inCinemas",
        "digitalRelease",
        "physicalRelease",
        "budget",
        "revenue",
    ] {
        o.insert(k.into(), Value::Null);
    }
    o.insert(
        "status".into(),
        series_status(get(get(m, "status"), "name").as_str().unwrap_or_default()).into(),
    );
    let country = get(m, "originalCountry");
    o.insert(
        "productionCountries".into(),
        if country.as_str().is_some_and(|c| !c.is_empty()) {
            json!([country])
        } else {
            json!([])
        },
    );
    let companies: Vec<Value> = pluck(arr(m, "networks"), "name")
        .into_iter()
        .chain(pluck(arr(get(m, "companies"), "production"), "name"))
        .chain(pluck(arr(m, "studios"), "name"))
        .collect();
    o.insert("productionCompanies".into(), companies.into());
}

fn movie_fields(o: &mut Map<String, Value>, m: &Value) {
    let first = nn(get(m, "first_release"), "date");
    o.insert("seasonCount".into(), Value::Null);
    o.insert("runtime".into(), nn(m, "runtime"));
    o.insert("releaseDate".into(), first.clone());
    let releases = get(m, "releases");
    o.insert(
        "inCinemas".into(),
        release_date(releases, "Theatrical").unwrap_or(first),
    );
    o.insert(
        "digitalRelease".into(),
        release_date(releases, "Digital").unwrap_or(Value::Null),
    );
    o.insert(
        "physicalRelease".into(),
        release_date(releases, "Physical").unwrap_or(Value::Null),
    );
    o.insert(
        "status".into(),
        str_lower_or(get(m, "status"), "name", "unknown").into(),
    );
    let int = |k: &str| match get(m, k)
        .as_str()
        .filter(|s| !s.is_empty())
        .and_then(js_parse_int)
    {
        Some(n) if n != 0 => Value::from(n),
        _ => Value::Null,
    };
    o.insert("budget".into(), int("budget"));
    o.insert("revenue".into(), int("boxOffice"));
    o.insert(
        "productionCountries".into(),
        pluck(arr(m, "production_countries"), "name").into(),
    );
    let companies: Vec<Value> = pluck(arr(get(m, "companies"), "production"), "name")
        .into_iter()
        .chain(pluck(arr(get(m, "companies"), "studio"), "name"))
        .collect();
    o.insert("productionCompanies".into(), companies.into());
}
