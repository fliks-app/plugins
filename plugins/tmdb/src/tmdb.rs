//! TMDB v3 requests and response mapping, over an `Http` that answers parsed JSON.
use crate::map::{
    arr, get, image, js_parse_float, js_parse_int, nn, num_or_null, or, pluck, put, score,
    str_lower_or, text, text_or_null, year_of,
};
use fliks_plugin_sdk::rt::join_all;
use fliks_plugin_sdk_provider::{Lang, ProviderError};
use serde_json::{Map, Value, json};

pub const IMAGE_URL: &str = "https://image.tmdb.org/t/p";
const MAX_ADDITIONAL_FANARTS: usize = 5;
const SEASON_FETCH_CONCURRENCY: usize = 8;

pub type Query = Vec<(String, String)>;
pub type Result<T> = std::result::Result<T, ProviderError>;

/// One TMDB GET: the path under `/3` and its query without the API key.
pub trait Http {
    async fn get(&self, path: &str, query: Query) -> Result<Value>;
    fn warn(&self, _message: &str) {}
}

pub use provider_common::http::{json_reply as reply, upstream};

/// The provider asks for fewer calls: 429 or 503.
fn throttled(e: &ProviderError) -> bool {
    match e {
        ProviderError::RateLimited(_) => true,
        ProviderError::Upstream(u) => u.status == Some(503),
        _ => false,
    }
}

/// The metadata language in TMDB's dialect.
pub struct Resolved {
    locale: String,
    pub(crate) image_languages: String,
    logo_language: String,
    pub(crate) region: String,
    pub(crate) release_priority: Vec<String>,
}

impl Resolved {
    pub fn of(lang: &Lang) -> Self {
        let region = lang
            .region
            .clone()
            .or_else(|| lang.locale.split('-').nth(1).map(str::to_owned))
            .unwrap_or_default();
        Self {
            locale: lang.locale.clone(),
            image_languages: if lang.language == "en" {
                "en,null".into()
            } else {
                format!("{},en,null", lang.language)
            },
            logo_language: lang.language.clone(),
            release_priority: if region == "US" {
                vec!["US".into()]
            } else {
                vec![region.clone(), "US".into()]
            },
            region,
        }
    }
}

fn q(pairs: &[(&str, &str)]) -> Query {
    pairs
        .iter()
        .map(|(k, v)| ((*k).to_owned(), (*v).to_owned()))
        .collect()
}

fn results(v: &Value) -> Result<&[Value]> {
    v.get("results")
        .and_then(Value::as_array)
        .map(Vec::as_slice)
        .ok_or_else(|| upstream(None, "Cannot read properties of undefined (reading 'map')"))
}

fn kind_name(tv: bool) -> &'static str {
    if tv { "series" } else { "movie" }
}

pub struct Tmdb<H> {
    pub http: H,
}

impl<H: Http> Tmdb<H> {
    fn list_of(&self, data: &Value, tv: bool) -> Result<Vec<Value>> {
        Ok(results(data)?.iter().map(|r| result(r, tv)).collect())
    }

    pub async fn search(
        &self,
        tv: bool,
        query: &str,
        year: Option<i32>,
        lang: &Resolved,
    ) -> Result<Vec<Value>> {
        let mut params = q(&[("query", query), ("language", &lang.locale)]);
        if let Some(y) = year {
            let key = if tv { "first_air_date_year" } else { "year" };
            params.push((key.into(), y.to_string()));
        }
        let path = if tv { "/search/tv" } else { "/search/movie" };
        self.list_of(&self.http.get(path, params).await?, tv)
    }

    /// A global list: `trending` (param `window`), `popular`, `upcoming`, `genres` or `discover`.
    pub async fn list(
        &self,
        tv: bool,
        name: &str,
        params: &[(String, String)],
        lang: &Resolved,
    ) -> Result<Value> {
        let kind = if tv { "tv" } else { "movie" };
        let locale = q(&[("language", &lang.locale)]);
        let (path, query) = match name {
            "trending" => {
                let window = match param(params, "window") {
                    Some("day") => "day",
                    _ => "week",
                };
                (format!("/trending/{kind}/{window}"), locale)
            }
            "popular" => (format!("/{kind}/popular"), locale),
            "upcoming" if tv => ("/tv/on_the_air".into(), locale),
            "upcoming" => (
                "/movie/upcoming".into(),
                q(&[("language", &lang.locale), ("region", &lang.region)]),
            ),
            "genres" => {
                let d = self
                    .http
                    .get(&format!("/genre/{kind}/list"), locale)
                    .await?;
                return Ok(d.get("genres").cloned().unwrap_or_else(|| json!([])));
            }
            "discover" => (
                format!("/discover/{kind}"),
                discover_params(params, lang, tv),
            ),
            _ => return Err(ProviderError::Unsupported),
        };
        let d = self.http.get(&path, query).await?;
        Ok(Value::Array(self.list_of(&d, tv)?))
    }

    pub async fn details(&self, tv: bool, id: i64, lang: &Resolved) -> Result<Value> {
        let (path, append) = if tv {
            (
                format!("/tv/{id}"),
                "external_ids,images,credits,videos,keywords,alternative_titles",
            )
        } else {
            (
                format!("/movie/{id}"),
                "external_ids,images,release_dates,credits,videos,keywords,alternative_titles",
            )
        };
        let query = q(&[
            ("language", &lang.locale),
            ("include_image_language", &lang.image_languages),
            ("append_to_response", append),
        ]);
        let d = self.http.get(&path, query).await?;
        Ok(if tv {
            map_tv_details(&d, lang)
        } else {
            map_movie_details(&d, lang)
        })
    }

    /// Season numbers and episode counts in one call; season 0 is kept.
    pub async fn season_stubs(&self, id: i64, lang: &Resolved) -> Result<Vec<Value>> {
        let d = self
            .http
            .get(&format!("/tv/{id}"), q(&[("language", &lang.locale)]))
            .await?;
        Ok(arr(&d, "seasons")
            .iter()
            .map(|s| json!({"seasonNumber": nn(s, "season_number"), "episodeCount": or(s, "episode_count", json!(0))}))
            .collect())
    }

    async fn season(&self, id: i64, n: i64, lang: &Resolved) -> Result<Value> {
        let s = self
            .http
            .get(
                &format!("/tv/{id}/season/{n}"),
                q(&[("language", &lang.locale)]),
            )
            .await?;
        let episodes: Vec<Value> = arr(&s, "episodes")
            .iter()
            .map(|e| {
                json!({
                    "episodeNumber": nn(e, "episode_number"),
                    "title": nn(e, "name"),
                    "overview": text_or_null(e, "overview"),
                    "airDate": text_or_null(e, "air_date"),
                    "runtime": nn(e, "runtime"),
                    "stillUrl": image(IMAGE_URL, "w780", e, "still_path"),
                })
            })
            .collect();
        Ok(json!({
            "seasonNumber": nn(&s, "season_number"),
            "episodeCount": episodes.len(),
            "overview": text_or_null(&s, "overview"),
            "airDate": text_or_null(&s, "air_date"),
            "posterUrl": image(IMAGE_URL, "w500", &s, "poster_path"),
            "episodes": episodes,
        }))
    }

    /// Every season, 8 at a time, stub order kept. A failing season is dropped unless the provider
    /// is throttling: a partial list would make identify delete the missing seasons.
    /// With `only`, just that season (none when TMDB has no such season).
    pub async fn seasons(&self, id: i64, only: Option<i64>, lang: &Resolved) -> Result<Vec<Value>> {
        if let Some(n) = only {
            return match self.season(id, n, lang).await {
                Ok(s) => Ok(vec![s]),
                Err(ProviderError::NotFound) => Ok(vec![]),
                Err(e) => Err(e),
            };
        }
        let stubs = self.season_stubs(id, lang).await?;
        let mut out = Vec::new();
        for chunk in stubs.chunks(SEASON_FETCH_CONCURRENCY) {
            let fetched = join_all(chunk.iter().map(|stub| async move {
                let n = stub["seasonNumber"].as_i64().unwrap_or_default();
                match self.season(id, n, lang).await {
                    Ok(s) => Ok(Some(s)),
                    Err(e) if throttled(&e) => Err(e),
                    Err(_) => {
                        self.http
                            .warn(&format!("failed to fetch season {n} for TV {id}"));
                        Ok(None)
                    }
                }
            }))
            .await;
            for s in fetched {
                out.extend(s?);
            }
        }
        Ok(out)
    }

    /// The TMDB id and kind of the title an IMDB or TVDB id names.
    pub async fn find_by(
        &self,
        source: &str,
        id: &str,
        lang: &Resolved,
    ) -> Result<Option<(String, &'static str)>> {
        let external = match source {
            "imdb" => "imdb_id",
            "tvdb" => "tvdb_id",
            _ => return Ok(None),
        };
        let d = self
            .http
            .get(
                &format!("/find/{id}"),
                q(&[("external_source", external), ("language", &lang.locale)]),
            )
            .await?;
        let first = |k: &str| arr(&d, k).first().and_then(|m| id_string(get(m, "id")));
        Ok(first("movie_results")
            .map(|i| (i, kind_name(false)))
            .or_else(|| first("tv_results").map(|i| (i, kind_name(true)))))
    }

    pub async fn person(&self, id: i64, lang: &Resolved) -> Result<Value> {
        let d = self
            .http
            .get(&format!("/person/{id}"), q(&[("language", &lang.locale)]))
            .await?;
        let s = |k: &str| get(&d, k).as_str().map(str::to_owned);
        Ok(json!({
            "name": s("name"),
            "biography": s("biography"),
            "birthday": s("birthday"),
            "deathday": s("deathday"),
            "placeOfBirth": s("place_of_birth"),
            "avatarUrl": text(&d, "profile_path").map(|p| format!("{IMAGE_URL}/w500{p}")),
            "knownForDepartment": s("known_for_department"),
        }))
    }

    pub async fn credits(&self, id: i64, lang: &Resolved) -> Result<Value> {
        let d = self
            .http
            .get(
                &format!("/person/{id}/combined_credits"),
                q(&[("language", &lang.locale)]),
            )
            .await?;
        let list = |k: &str, extra: &[&str]| -> Result<Vec<Value>> {
            let items = d.get(k).and_then(Value::as_array).ok_or_else(|| {
                upstream(None, "Cannot read properties of undefined (reading 'map')")
            })?;
            Ok(items.iter().map(|c| credit_item(c, extra)).collect())
        };
        Ok(json!({
            "cast": list("cast", &["character"])?,
            "crew": list("crew", &["job", "department"])?,
        }))
    }
}

fn param<'a>(params: &'a [(String, String)], key: &str) -> Option<&'a str> {
    params
        .iter()
        .find(|(k, _)| k == key)
        .map(|(_, v)| v.as_str())
}

/// The `/discover/{movie,tv}` query of the list params `genres`, `sort`, `voteGte`, `yearGte`
/// and `yearLte`; an empty or non-numeric value is dropped.
fn discover_params(params: &[(String, String)], lang: &Resolved, tv: bool) -> Query {
    let given = |k: &str| param(params, k).filter(|s| !s.is_empty());
    let int = |k: &str| given(k).and_then(js_parse_int).filter(|n| *n != 0);
    let sort = given("sort").unwrap_or("popularity.desc");
    let sort = if tv {
        sort.replacen("primary_release_date", "first_air_date", 1)
    } else {
        sort.to_owned()
    };
    let mut p = q(&[("language", &lang.locale)]);
    if !tv {
        p.push(("include_adult".into(), "false".into()));
    }
    p.push(("sort_by".into(), sort));
    p.push(("vote_count.gte".into(), "50".into()));
    let genres: Vec<String> = param(params, "genres")
        .unwrap_or_default()
        .split(',')
        .filter_map(js_parse_int)
        .map(|g| g.to_string())
        .collect();
    if !genres.is_empty() {
        p.push(("with_genres".into(), genres.join(",")));
    }
    let vote = given("voteGte")
        .and_then(js_parse_float)
        .filter(|n| n.is_finite() && *n != 0.0);
    if let Some(v) = vote {
        p.push(("vote_average.gte".into(), v.to_string()));
    }
    let (gte, lte) = if tv {
        ("first_air_date.gte", "first_air_date.lte")
    } else {
        ("primary_release_date.gte", "primary_release_date.lte")
    };
    if let Some(y) = int("yearGte") {
        p.push((gte.into(), format!("{y}-01-01")));
    }
    if let Some(y) = int("yearLte") {
        p.push((lte.into(), format!("{y}-12-31")));
    }
    p
}

/// A search or list entry.
pub fn result(r: &Value, tv: bool) -> Value {
    let (title, original, date) = if tv {
        ("name", "original_name", "first_air_date")
    } else {
        ("title", "original_title", "release_date")
    };
    let mut o = Map::new();
    put(&mut o, "tmdbId", r, "id");
    o.insert("provider".into(), "tmdb".into());
    put(&mut o, "title", r, title);
    put(&mut o, "originalTitle", r, original);
    put(&mut o, "overview", r, "overview");
    o.insert("year".into(), year_of(r, date));
    o.insert(
        "posterUrl".into(),
        image(IMAGE_URL, "w500", r, "poster_path"),
    );
    o.insert(
        "fanartUrl".into(),
        image(IMAGE_URL, "original", r, "backdrop_path"),
    );
    o.insert("rating".into(), or(r, "vote_average", json!(0)));
    o.insert("genres".into(), json!([]));
    o.insert("genreIds".into(), or(r, "genre_ids", json!([])));
    o.insert("mediaType".into(), kind_name(tv).into());
    Value::Object(o)
}

fn credits_of(d: &Value) -> (Value, Value) {
    let url = |c: &Value| image(IMAGE_URL, "w185", c, "profile_path");
    let cast = arr(get(d, "credits"), "cast")
        .iter()
        .map(|c| {
            let mut o = Map::new();
            put(&mut o, "externalId", c, "id");
            put(&mut o, "name", c, "name");
            put(&mut o, "character", c, "character");
            o.insert("avatarUrl".into(), url(c));
            put(&mut o, "order", c, "order");
            Value::Object(o)
        })
        .collect();
    let crew = arr(get(d, "credits"), "crew")
        .iter()
        .map(|c| {
            let mut o = Map::new();
            put(&mut o, "externalId", c, "id");
            put(&mut o, "name", c, "name");
            put(&mut o, "job", c, "job");
            put(&mut o, "department", c, "department");
            o.insert("avatarUrl".into(), url(c));
            Value::Object(o)
        })
        .collect();
    (Value::Array(cast), Value::Array(crew))
}

/// One title of a person's combined credits.
fn credit_item(c: &Value, extra: &[&str]) -> Value {
    let mut o = Map::new();
    put(&mut o, "externalId", c, "id");
    let title = text(c, "title")
        .or_else(|| get(c, "name").as_str())
        .unwrap_or_default();
    o.insert("title".into(), title.into());
    o.insert(
        "mediaType".into(),
        kind_name(get(c, "media_type") == "tv").into(),
    );
    for k in extra {
        put(&mut o, k, c, k);
    }
    o.insert(
        "posterUrl".into(),
        image(IMAGE_URL, "w500", c, "poster_path"),
    );
    o.insert(
        "fanartUrl".into(),
        image(IMAGE_URL, "original", c, "backdrop_path"),
    );
    let date = match get(c, "release_date") {
        Value::Null => nn(c, "first_air_date"),
        x => x.clone(),
    };
    o.insert("releaseDate".into(), date);
    o.insert("rating".into(), or(c, "vote_average", json!(0)));
    Value::Object(o)
}

/// Fields both details shapes share.
fn common_details(d: &Value, lang: &Resolved, tv: bool) -> Map<String, Value> {
    let mut o = Map::new();
    o.insert("tmdbId".into(), nn(d, "id"));
    o.insert("provider".into(), "tmdb".into());
    o.insert("overview".into(), nn(d, "overview"));
    o.insert(
        "posterUrl".into(),
        image(IMAGE_URL, "w500", d, "poster_path"),
    );
    o.insert(
        "fanartUrl".into(),
        image(IMAGE_URL, "original", d, "backdrop_path"),
    );
    o.insert(
        "logoUrl".into(),
        pick_logo(IMAGE_URL, get(d, "images"), &lang.logo_language),
    );
    o.insert(
        "additionalFanartUrls".into(),
        pick_additional_fanarts(IMAGE_URL, get(d, "images"), get(d, "backdrop_path")).into(),
    );
    o.insert("rating".into(), or(d, "vote_average", json!(0)));
    o.insert("genres".into(), pluck(arr(d, "genres"), "name").into());
    o.insert("genreIds".into(), pluck(arr(d, "genres"), "id").into());
    o.insert("mediaType".into(), kind_name(tv).into());
    o.insert("originalLanguage".into(), nn(d, "original_language"));
    o.insert("voteCount".into(), nn(d, "vote_count"));
    o.insert("popularity".into(), nn(d, "popularity"));
    o.insert("tagline".into(), text_or_null(d, "tagline"));
    let (cast, crew) = credits_of(d);
    o.insert("cast".into(), cast);
    o.insert("crew".into(), crew);
    let videos = arr(get(d, "videos"), "results")
        .iter()
        .map(|v| {
            let mut m = Map::new();
            for k in ["key", "site", "type", "name"] {
                put(&mut m, k, v, k);
            }
            Value::Object(m)
        })
        .collect::<Vec<_>>();
    o.insert("videos".into(), videos.into());
    o
}

pub fn map_movie_details(d: &Value, lang: &Resolved) -> Value {
    let mut o = common_details(d, lang, false);
    let dates = release_dates(
        arr(get(d, "release_dates"), "results"),
        &lang.release_priority,
    );
    let imdb = match get(get(d, "external_ids"), "imdb_id") {
        Value::Null => nn(d, "imdb_id"),
        x => x.clone(),
    };
    o.insert("tvdbId".into(), Value::Null);
    o.insert("seasonCount".into(), Value::Null);
    o.insert("episodeCount".into(), Value::Null);
    o.insert("imdbId".into(), imdb);
    o.insert("title".into(), nn(d, "title"));
    o.insert("originalTitle".into(), nn(d, "original_title"));
    o.insert("year".into(), year_of(d, "release_date"));
    o.insert("runtime".into(), nn(d, "runtime"));
    o.insert("releaseDate".into(), nn(d, "release_date"));
    o.insert("inCinemas".into(), dates.0.map_or(Value::Null, Value::from));
    o.insert(
        "digitalRelease".into(),
        dates.1.map_or(Value::Null, Value::from),
    );
    o.insert(
        "physicalRelease".into(),
        dates.2.map_or(Value::Null, Value::from),
    );
    o.insert("status".into(), str_lower_or(d, "status", "unknown").into());
    o.insert("budget".into(), num_or_null(d, "budget"));
    o.insert("revenue".into(), num_or_null(d, "revenue"));
    o.insert(
        "productionCountries".into(),
        pluck(arr(d, "production_countries"), "name").into(),
    );
    o.insert(
        "productionCompanies".into(),
        pluck(arr(d, "production_companies"), "name").into(),
    );
    o.insert(
        "keywords".into(),
        pluck(arr(get(d, "keywords"), "keywords"), "name").into(),
    );
    o.insert(
        "alternativeTitles".into(),
        alternative_titles(
            arr(get(d, "alternative_titles"), "titles"),
            d,
            "title",
            "original_title",
        )
        .into(),
    );
    let coll = get(d, "belongs_to_collection");
    o.insert("tmdbCollectionId".into(), nn(coll, "id"));
    o.insert("tmdbCollectionName".into(), nn(coll, "name"));
    Value::Object(o)
}

pub fn map_tv_details(d: &Value, lang: &Resolved) -> Value {
    let mut o = common_details(d, lang, true);
    let ext = get(d, "external_ids");
    o.insert("tvdbId".into(), nn(ext, "tvdb_id"));
    o.insert("imdbId".into(), nn(ext, "imdb_id"));
    o.insert("title".into(), nn(d, "name"));
    o.insert("originalTitle".into(), nn(d, "original_name"));
    o.insert("year".into(), year_of(d, "first_air_date"));
    o.insert(
        "runtime".into(),
        arr(d, "episode_run_time")
            .first()
            .cloned()
            .unwrap_or(Value::Null),
    );
    o.insert("seasonCount".into(), nn(d, "number_of_seasons"));
    o.insert("episodeCount".into(), nn(d, "number_of_episodes"));
    o.insert("releaseDate".into(), nn(d, "first_air_date"));
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
        tv_status(get(d, "status").as_str().unwrap_or_default()).into(),
    );
    o.insert(
        "productionCountries".into(),
        or(d, "origin_country", json!([])),
    );
    let companies: Vec<Value> = pluck(arr(d, "networks"), "name")
        .into_iter()
        .chain(pluck(arr(d, "production_companies"), "name"))
        .collect();
    o.insert("productionCompanies".into(), companies.into());
    o.insert(
        "keywords".into(),
        pluck(arr(get(d, "keywords"), "results"), "name").into(),
    );
    o.insert(
        "alternativeTitles".into(),
        alternative_titles(
            arr(get(d, "alternative_titles"), "results"),
            d,
            "name",
            "original_name",
        )
        .into(),
    );
    o.insert("tmdbCollectionId".into(), Value::Null);
    o.insert("tmdbCollectionName".into(), Value::Null);
    Value::Object(o)
}

/// JS `String(n)` of an id; a missing one is no id.
fn id_string(v: &Value) -> Option<String> {
    match v {
        Value::Null => None,
        Value::String(s) => Some(s.clone()),
        other => Some(other.to_string()),
    }
}

/// Best clearlogo: community vote plus a language bonus, the provider's order breaking ties.
/// An SVG is fetched as the PNG rendition TMDB serves for it: the image store decodes no SVG.
pub fn pick_logo(base: &str, images: &Value, prefer: &str) -> Value {
    let bonus = |l: &Value| match get(l, "iso_639_1").as_str() {
        Some(x) if x == prefer => 0.5,
        Some("en") => 0.25,
        _ => 0.0,
    };
    let mut best: Option<(&Value, f64)> = None;
    for l in arr(images, "logos")
        .iter()
        .filter(|l| text(l, "file_path").is_some())
    {
        let s = score(l, "vote_average") + bonus(l);
        if best.is_none_or(|(_, b)| s > b) {
            best = Some((l, s));
        }
    }
    best.and_then(|(l, _)| text(l, "file_path"))
        .map_or(Value::Null, |p| {
            let p = match p.len().checked_sub(4) {
                Some(i) if p.is_char_boundary(i) && p[i..].eq_ignore_ascii_case(".svg") => {
                    format!("{}.png", &p[..i])
                }
                _ => p.to_owned(),
            };
            json!(format!("{base}/original{p}"))
        })
}

/// The top backdrops other than the primary one, by vote.
pub fn pick_additional_fanarts(base: &str, images: &Value, primary: &Value) -> Vec<Value> {
    let mut c: Vec<&Value> = arr(images, "backdrops")
        .iter()
        .filter(|b| text(b, "file_path").is_some() && get(b, "file_path") != primary)
        .collect();
    c.sort_by(|a, b| score(b, "vote_average").total_cmp(&score(a, "vote_average")));
    c.into_iter()
        .take(MAX_ADDITIONAL_FANARTS)
        .map(|b| image(base, "original", b, "file_path"))
        .collect()
}

/// Trimmed alternative titles, de-duplicated case-insensitively against the titles and each other.
pub fn alternative_titles(raw: &[Value], d: &Value, primary: &str, original: &str) -> Vec<Value> {
    let mut seen: Vec<String> = [primary, original]
        .iter()
        .filter_map(|k| text(d, k))
        .map(str::to_lowercase)
        .collect();
    let mut out = vec![];
    for e in raw {
        let t = text(e, "title")
            .or_else(|| get(e, "name").as_str())
            .unwrap_or_default()
            .trim();
        if t.is_empty() || seen.contains(&t.to_lowercase()) {
            continue;
        }
        seen.push(t.to_lowercase());
        out.push(Value::from(t));
    }
    out
}

/// `(inCinemas, digitalRelease, physicalRelease)`: countries by priority, the first carrying a type wins.
pub fn release_dates(
    results: &[Value],
    priority: &[String],
) -> (Option<String>, Option<String>, Option<String>) {
    let rank = |c: &Value| {
        let code = get(c, "iso_3166_1").as_str().unwrap_or_default();
        priority.iter().position(|p| p == code).unwrap_or(99)
    };
    let mut sorted: Vec<&Value> = results.iter().collect();
    sorted.sort_by_key(|c| rank(c));
    let mut dates = std::collections::HashMap::<i64, String>::new();
    for country in sorted {
        for rd in arr(country, "release_dates") {
            let (Some(date), Some(kind)) = (text(rd, "release_date"), get(rd, "type").as_i64())
            else {
                continue;
            };
            dates
                .entry(kind)
                .or_insert_with(|| date.chars().take(10).collect());
        }
    }
    let d = |k: i64| dates.get(&k).cloned();
    (d(3).or_else(|| d(2)).or_else(|| d(1)), d(4), d(5))
}

pub fn tv_status(s: &str) -> &'static str {
    match s {
        "Returning Series" => "continuing",
        "Ended" | "Canceled" => "ended",
        "In Production" => "announced",
        "Planned" => "tba",
        _ => "unknown",
    }
}
