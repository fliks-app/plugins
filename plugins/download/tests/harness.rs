#![cfg(not(target_arch = "wasm32"))]
//! The built component under the real host and a scratch Postgres. Needs the release component
//! (`cargo build --release --target wasm32-wasip2`) and `FLIKS_HARNESS_PG_URL`.

use fk_plugin_download::throttle::sql;
use fliks_plugin_sdk::harness::fliks_server::events::Bus;
use fliks_plugin_sdk::harness::fliks_server::plugins::HttpResponse;
use fliks_plugin_sdk::harness::{Harness, Setup, request, until};
use serde_json::{Value, json};
use sqlx::{AssertSqlSafe, Connection, PgConnection};
use std::io::{BufRead, BufReader, Read, Write};
use std::net::{SocketAddr, TcpListener};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

const MAX: Duration = Duration::from_secs(20);
const GOLDEN: &str = include_str!("vectors/golden.json");
const HASH: &str = "0123456789abcdef0123456789abcdef01234567";

fn setup() -> Setup {
    let mut s = Setup::from_crate(env!("CARGO_MANIFEST_DIR"));
    s.network_internal = true;
    // The harness core reports 6.0.0; the manifest's real range is pinned by the vectors test.
    s.template["fliks"] = json!(">=6.0.0 <7.0.0");
    s
}

fn body(r: &HttpResponse) -> Value {
    serde_json::from_slice(&r.body).unwrap_or(Value::Null)
}

fn logged(h: &Harness, text: &str) -> bool {
    h.logs().iter().any(|l| l.contains(text))
}

/// A superuser session inside the plugin schema.
async fn plugin_db(h: &Harness) -> PgConnection {
    let mut c = PgConnection::connect_with(&h.pool.connect_options())
        .await
        .unwrap();
    sqlx::raw_sql("SET search_path = plugin_fliks_download")
        .execute(&mut c)
        .await
        .unwrap();
    c
}

async fn exec(c: &mut PgConnection, q: impl Into<String>) {
    sqlx::raw_sql(AssertSqlSafe(q.into()))
        .execute(c)
        .await
        .unwrap();
}

async fn scalar_i64(c: &mut PgConnection, q: impl Into<String>) -> i64 {
    sqlx::query_scalar(AssertSqlSafe(q.into()))
        .fetch_one(c)
        .await
        .unwrap()
}

async fn ledger(c: &mut PgConnection) -> Vec<String> {
    sqlx::query_scalar(r#"SELECT "name" FROM "_migrations" ORDER BY "name""#)
        .fetch_all(c)
        .await
        .unwrap()
}

async fn add_indexer(c: &mut PgConnection, name: &str, settings: Value) -> i64 {
    sqlx::query_scalar::<_, i32>(
        r#"INSERT INTO "indexers" ("name", "implementation", "settings", "requestDelay", "capsMovieSearch", "capsProbedAt")
           VALUES ($1, 'torznab', $2, 1, true, now()) RETURNING "id""#,
    )
    .bind(name)
    .bind(settings)
    .fetch_one(c)
    .await
    .unwrap()
    .into()
}

async fn status_of(c: &mut PgConnection, id: i64) -> (String, Option<String>) {
    sqlx::query_as(
        r#"SELECT "status"::text, "statusMessage" FROM "download_history" WHERE "id" = $1"#,
    )
    .bind(id as i32)
    .fetch_one(c)
    .await
    .unwrap()
}

/// The ref the poller reports for history row `row`: its id and creation time.
async fn ref_of(h: &Harness, row: i64) -> String {
    sqlx::query_scalar(
        r#"SELECT "id"::text || '-' || to_char("createdAt" AT TIME ZONE 'UTC', 'YYYYMMDDHH24MISSMS')
           FROM "download_history" WHERE "id" = $1"#,
    )
    .bind(row as i32)
    .fetch_optional(&mut plugin_db(h).await)
    .await
    .unwrap()
    .unwrap_or_else(|| format!("{row}-0"))
}

/// A queue action on the history row `row`, as the core's queue routes send it.
async fn act(h: &Harness, action: &str, row: i64) -> HttpResponse {
    let b = json!({ "ref": ref_of(h, row).await });
    h.proxy("POST", &format!("/queue/{action}"), Some(&b)).await
}

/// What the core holds of this plugin's transfers: (ref, state, progress), oldest first.
async fn queue_rows(h: &Harness) -> Vec<(String, String, f64)> {
    sqlx::query_as(
        r#"SELECT "ref", state, progress FROM acquisition_queue WHERE "pluginId" = $1 ORDER BY id"#,
    )
    .bind(&h.id)
    .fetch_all(&h.pool)
    .await
    .unwrap()
}

/// A monitored, profiled movie in core, searchable through `media.acquisitionContext`.
async fn add_movie(h: &Harness, title: &str) -> i64 {
    add_movie_in(h, title, None).await
}

/// The same, anchored to a library folder when `library` is given: what an import needs.
async fn add_movie_in(h: &Harness, title: &str, library: Option<&std::path::Path>) -> i64 {
    add_work(h, title, "movie", library).await
}

/// A monitored work of `kind` with its video want, edition and, for a movie, its own item.
async fn add_work(h: &Harness, title: &str, kind: &str, library: Option<&std::path::Path>) -> i64 {
    let lib: i32 = sqlx::query_scalar("INSERT INTO libraries (name) VALUES ($1) RETURNING id")
        .bind(title)
        .fetch_one(&h.pool)
        .await
        .unwrap();
    if let Some(root) = library {
        sqlx::query(r#"INSERT INTO library_folders ("libraryId", path) VALUES ($1, $2)"#)
            .bind(lib)
            .bind(root.to_string_lossy().into_owned())
            .execute(&h.pool)
            .await
            .unwrap();
    }
    let qp: i32 = sqlx::query_scalar(
        "INSERT INTO quality_profiles (name, cutoff, items) VALUES ('Any', 0, '[]') RETURNING id",
    )
    .fetch_one(&h.pool)
    .await
    .unwrap();
    let media: i32 = sqlx::query_scalar(
        r#"INSERT INTO media (title, type, year, monitored, "libraryId", "folderName")
           VALUES ($1, $2, 2020, true, $3, $1 || ' (2020)') RETURNING id"#,
    )
    .bind(title)
    .bind(kind)
    .bind(lib)
    .fetch_one(&h.pool)
    .await
    .unwrap();
    sqlx::query(
        r#"INSERT INTO wants ("mediaId", family, "qualityProfileId") VALUES ($1, 'video', $2)"#,
    )
    .bind(media)
    .bind(qp)
    .execute(&h.pool)
    .await
    .unwrap();
    let edition: i32 = sqlx::query_scalar(
        r#"INSERT INTO editions ("mediaId", family) VALUES ($1, 'video') RETURNING id"#,
    )
    .bind(media)
    .fetch_one(&h.pool)
    .await
    .unwrap();
    if kind == "movie" {
        sqlx::query(
            r#"INSERT INTO items ("mediaId", "editionId", monitored) VALUES ($1, $2, true)"#,
        )
        .bind(media)
        .bind(edition)
        .execute(&h.pool)
        .await
        .unwrap();
    }
    media.into()
}

// --- stub HTTP servers ---------------------------------------------------------------------

#[derive(Clone, Debug)]
struct Hit {
    target: String,
    body: String,
    start: Instant,
    end: Instant,
}

type Reply = (u16, Vec<(&'static str, String)>, Vec<u8>);

/// One thread per connection, `connection: close`; every request is recorded.
struct Stub {
    addr: SocketAddr,
    hits: Arc<Mutex<Vec<Hit>>>,
}

impl Stub {
    fn start(answer: impl Fn(&str, &str, &str) -> Reply + Send + Sync + 'static) -> Stub {
        let l = TcpListener::bind("127.0.0.1:0").unwrap();
        let addr = l.local_addr().unwrap();
        let hits: Arc<Mutex<Vec<Hit>>> = Arc::default();
        let (answer, log) = (Arc::new(answer), hits.clone());
        std::thread::spawn(move || {
            for c in l.incoming().flatten() {
                let (answer, log) = (answer.clone(), log.clone());
                std::thread::spawn(move || serve(c, &*answer, &log));
            }
        });
        Stub { addr, hits }
    }

    fn hits(&self) -> Vec<Hit> {
        self.hits.lock().unwrap().clone()
    }

    fn hits_to(&self, path: &str) -> Vec<Hit> {
        self.hits()
            .into_iter()
            .filter(|h| h.target.starts_with(path))
            .collect()
    }
}

fn serve(
    c: std::net::TcpStream,
    answer: &dyn Fn(&str, &str, &str) -> Reply,
    log: &Mutex<Vec<Hit>>,
) {
    let start = Instant::now();
    let mut r = BufReader::new(c.try_clone().unwrap());
    let mut line = String::new();
    if r.read_line(&mut line).is_err() {
        return;
    }
    let mut parts = line.split_whitespace();
    let method = parts.next().unwrap_or("").to_owned();
    let target = parts.next().unwrap_or("").to_owned();
    let mut len = 0;
    loop {
        let mut h = String::new();
        if r.read_line(&mut h).unwrap_or(0) == 0 || h == "\r\n" {
            break;
        }
        if let Some((k, v)) = h.split_once(':')
            && k.eq_ignore_ascii_case("content-length")
        {
            len = v.trim().parse().unwrap_or(0);
        }
    }
    let mut b = vec![0; len];
    let _ = r.read_exact(&mut b);
    let body = String::from_utf8_lossy(&b).into_owned();
    let (status, headers, payload) = answer(&method, &target, &body);
    // Logged before the reply: the caller may read the log as soon as it has the answer.
    log.lock().unwrap().push(Hit {
        target,
        body,
        start,
        end: Instant::now(),
    });
    let mut out = format!(
        "HTTP/1.1 {status} X\r\ncontent-length: {}\r\nconnection: close\r\n",
        payload.len()
    );
    for (k, v) in headers {
        out.push_str(&format!("{k}: {v}\r\n"));
    }
    out.push_str("\r\n");
    let mut c = c;
    let _ = c.write_all(out.as_bytes());
    let _ = c.write_all(&payload);
}

/// `name` resolves to a private address the host vets, then connects to the stub instead.
fn route(h: &Harness, name: &str, ip: &str, port: u16, stub: &Stub) {
    h.targets.answer(name, &[ip]);
    h.targets.route(&format!("{ip}:{port}"), stub.addr);
}

const EMPTY_FEED: &str = r#"<?xml version="1.0"?><rss><channel></channel></rss>"#;
const CAPS: &str = r#"<caps><searching><search available="yes"/><movie-search available="yes" supportedParams="q,imdbid"/><tv-search available="no"/></searching></caps>"#;

/// qBittorrent that holds one torrent once it is added, under the category it was added with
/// and listed only for that category; `stop`/`start` flip its state.
fn fake_qbit() -> Stub {
    let state = Mutex::new(None::<&str>);
    let category = Mutex::new(String::new());
    Stub::start(move |_, target, body| {
        let path = target.split('?').next().unwrap_or("");
        let ok = |b: &str| (200, vec![], b.as_bytes().to_vec());
        match path {
            "/api/v2/auth/login" => (
                200,
                vec![("set-cookie", "SID=s; path=/".into())],
                b"Ok.".to_vec(),
            ),
            "/api/v2/torrents/info" => {
                let asked = target
                    .split_once("category=")
                    .map(|(_, c)| c.split('&').next().unwrap_or(""));
                let held = category.lock().unwrap().clone();
                let t = state.lock().unwrap().filter(|_| asked.is_none_or(|a| a == held)).map(|s| {
                    json!({
                        "hash": HASH, "name": "Some.Movie.2020.1080p", "size": 1000, "downloaded": 500,
                        "progress": 0.5, "dlspeed": 100, "ratio": 0, "eta": 10,
                        "state": s, "save_path": "/downloads", "added_on": 1,
                        "num_complete": 12, "num_incomplete": -1
                    })
                });
                ok(&json!(t.into_iter().collect::<Vec<_>>()).to_string())
            }
            "/api/v2/torrents/add" => {
                *category.lock().unwrap() = body
                    .split_once("category=")
                    .map(|(_, c)| c.split('&').next().unwrap_or("").to_owned())
                    .unwrap_or_default();
                *state.lock().unwrap() = Some("downloading");
                ok("Ok.")
            }
            "/api/v2/torrents/start" => {
                *state.lock().unwrap() = Some("downloading");
                ok("Ok.")
            }
            "/api/v2/torrents/stop" => {
                *state.lock().unwrap() = Some("stoppedDL");
                ok("")
            }
            "/api/v2/torrents/delete" => {
                *state.lock().unwrap() = None;
                ok("")
            }
            _ => (404, vec![], vec![]),
        }
    })
}

async fn add_qbit(h: &Harness, qbit: &Stub) -> i64 {
    route(h, "qbit.test", "10.77.0.2", 8080, qbit);
    let r = h
        .proxy(
            "POST",
            "/download-clients",
            Some(&json!({
                "name": "qb", "implementation": "qbittorrent",
                "settings": { "host": "qbit.test", "port": 8080, "username": "u", "password": "pw", "category": "fliks", "movieCategory": "movies" }
            })),
        )
        .await;
    assert_eq!(r.status, 201, "{:?}", body(&r));
    body(&r)["id"].as_i64().unwrap()
}

/// Transmission that asks once for a session id, then holds one torrent once it is added, with
/// the labels `torrent-set` gave it.
fn fake_transmission() -> Stub {
    let (session, status) = (Mutex::new(false), Mutex::new(None::<i64>));
    let labels = Mutex::new(json!([]));
    Stub::start(move |_, _, body| {
        if !std::mem::replace(&mut *session.lock().unwrap(), true) {
            return (409, vec![("x-transmission-session-id", "s".into())], vec![]);
        }
        let req: Value = serde_json::from_str(body).unwrap_or_default();
        let mut status = status.lock().unwrap();
        let arguments = match req["method"].as_str().unwrap_or("") {
            "torrent-add" => {
                *status = Some(4);
                json!({ "torrent-added": { "id": 1, "hashString": HASH, "name": "Some.Movie.2020.1080p" } })
            }
            "torrent-get" => json!({ "torrents": status.map(|st| json!({
                "hashString": HASH, "name": "Some.Movie.2020.1080p", "sizeWhenDone": 1000,
                "downloadedEver": 500, "percentDone": 0.5, "rateDownload": 100, "uploadRatio": -1,
                "doneDate": 0, "eta": 10, "status": st, "error": 0, "downloadDir": "/downloads",
                "addedDate": 1, "labels": *labels.lock().unwrap(), "peersSendingToUs": 2,
                "trackerStats": [
                    { "seederCount": 3, "leecherCount": -1 },
                    { "seederCount": 8, "leecherCount": 4 },
                ],
            })).into_iter().collect::<Vec<_>>() }),
            "torrent-set" => {
                *labels.lock().unwrap() = req["arguments"]["labels"].clone();
                json!({})
            }
            "torrent-stop" => {
                *status = Some(0);
                json!({})
            }
            "torrent-start" => {
                *status = Some(4);
                json!({})
            }
            "torrent-remove" => {
                *status = None;
                json!({})
            }
            _ => json!({}),
        };
        let reply = json!({ "result": "success", "arguments": arguments });
        (200, vec![], reply.to_string().into_bytes())
    })
}

fn rpc_calls(stub: &Stub, method: &str) -> Vec<Value> {
    stub.hits_to("/transmission/rpc")
        .iter()
        .filter_map(|h| serde_json::from_str::<Value>(&h.body).ok())
        .filter(|b| b["method"] == method)
        .collect()
}

// --- tests ---------------------------------------------------------------------------------

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn start_migrates_and_routes_answer() {
    let Some(h) = Harness::start(setup()).await else {
        return;
    };
    let mut db = plugin_db(&h).await;
    assert_eq!(ledger(&mut db).await.len(), 15);
    assert!(h.health().await.unwrap().ok);
    for p in ["/indexers", "/indexer-sources", "/download-clients"] {
        let r = h.proxy("GET", &format!("{p}/implementations"), None).await;
        assert_eq!(r.status, 200, "{p}");
        assert!(body(&r)[0]["fields"].is_array(), "{p}");
        assert_eq!(body(&h.proxy("GET", p, None).await), json!([]), "{p}");
    }
    assert!(queue_rows(&h).await.is_empty());
    assert_eq!(
        body(&h.proxy("GET", "/blocklist", None).await),
        json!({ "data": [], "total": 0, "page": 1, "pageSize": 25 })
    );

    let r = h
        .proxy(
            "POST",
            "/indexers",
            Some(&json!({ "name": "x", "implementation": "torznab", "useFor": [] })),
        )
        .await;
    assert_eq!(
        (r.status, body(&r)),
        (
            400,
            json!({ "error": { "key": "download.config.indexers.errors.no_usage" } })
        )
    );
    let r = h
        .proxy(
            "POST",
            "/indexers",
            Some(&json!({ "implementation": "torznab" })),
        )
        .await;
    assert_eq!(
        (r.status, body(&r)),
        (
            400,
            json!({ "error": { "key": "download.http.errors.bad_body", "detail": "name" } })
        )
    );
    let r = h
        .proxy(
            "POST",
            "/download-clients",
            Some(&json!({ "name": "d", "implementation": "deluge" })),
        )
        .await;
    assert_eq!(
        (r.status, body(&r)),
        (
            400,
            json!({ "error": { "key": "download.http.errors.bad_body", "detail": "implementation" } })
        )
    );
    let r = h.proxy("DELETE", "/blocklist/999", None).await;
    assert_eq!(
        (r.status, body(&r)),
        (
            404,
            json!({ "error": { "key": "download.http.errors.not_found", "detail": "999" } })
        )
    );
    assert_eq!(act(&h, "retry-import", 999).await.status, 404);
    let r = h.proxy("GET", "/indexers/7/stats", None).await;
    assert_eq!((r.status, body(&r)), (200, json!([])));

    // Secrets never leave the plugin; a blank one on save keeps the stored value.
    let qbit = fake_qbit();
    let id = add_qbit(&h, &qbit).await;
    let listed = body(&h.proxy("GET", "/download-clients", None).await);
    assert_ne!(listed[0]["settings"]["password"], json!("pw"));
    let r = h
        .proxy(
            "PUT",
            &format!("/download-clients/{id}"),
            Some(&json!({ "settings": { "host": "qbit.test", "port": 8080, "password": "" } })),
        )
        .await;
    assert_eq!(r.status, 200, "{:?}", body(&r));
    let stored: Value =
        sqlx::query_scalar(r#"SELECT "settings" FROM "download_clients" WHERE "id" = $1"#)
            .bind(id as i32)
            .fetch_one(&mut db)
            .await
            .unwrap();
    assert_eq!(stored["password"], json!("pw"));
    let r = h
        .proxy(
            "POST",
            "/download-clients/test-connection",
            Some(&json!({ "implementation": "qbittorrent", "id": id, "settings": { "host": "qbit.test", "port": 8080 } })),
        )
        .await;
    assert_eq!(body(&r)["ok"], json!(true), "{:?}", body(&r));
    assert!(
        qbit.hits_to("/api/v2/auth/login")[0]
            .body
            .contains("password=pw")
    );
    assert_eq!(
        h.proxy("DELETE", &format!("/download-clients/{id}"), None)
            .await
            .status,
        200
    );
    assert_eq!(
        h.proxy("DELETE", &format!("/download-clients/{id}"), None)
            .await
            .status,
        404
    );
    h.finish().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn an_adopted_ledger_runs_only_the_new_migrations() {
    let Some(h) = Harness::start(setup()).await else {
        return;
    };
    let mut db = plugin_db(&h).await;
    // The schema a pluginApi 1 install leaves: 0001..0008 and the Node ledger.
    exec(
        &mut db,
        r#"DROP TABLE "indexer_throttle"; DROP TABLE "poller_state";
           ALTER TABLE "download_history" DROP COLUMN "importLeaseUntil";
           ALTER TABLE "download_history" RENAME COLUMN "itemId" TO "episodeId";
           DELETE FROM "_migrations" WHERE "name" >= '0009'"#,
    )
    .await;
    let ix = add_indexer(&mut db, "old", json!({})).await;
    h.restart().await;
    until("the re-run migrations", MAX, async || {
        ledger(&mut plugin_db(&h).await).await.len() == 15
    })
    .await;
    assert!(logged(
        &h,
        "migrations applied: 0009_indexer_throttle, 0010_poller_state, 0011_download_history_import_lease, 0012_item_history, 0013_indexer_caps_books, 0014_download_history_family, 0015_download_history_retry_later"
    ));
    let backfilled = scalar_i64(
        &mut db,
        &format!(r#"SELECT count(*) FROM "indexer_throttle" WHERE "indexerId" = {ix}"#),
    )
    .await;
    assert_eq!(backfilled, 1);
    h.finish().await;
}

async fn claim(
    c: &mut PgConnection,
    id: i64,
    token: &str,
    lease: f64,
    delay_ms: i64,
) -> Option<i64> {
    sqlx::query_scalar(sql::CLAIM)
        .bind(id as i32)
        .bind(token)
        .bind(lease)
        .bind(delay_ms)
        .fetch_optional(c)
        .await
        .unwrap()
}

async fn release(c: &mut PgConnection, id: i64, token: &str, delay: f64, outcome: &str) -> u64 {
    sqlx::query(sql::RELEASE)
        .bind(id as i32)
        .bind(token)
        .bind(delay)
        .bind(outcome)
        .execute(c)
        .await
        .unwrap()
        .rows_affected()
}

async fn fail(c: &mut PgConnection, id: i64) -> Option<i64> {
    sqlx::query_scalar::<_, i32>(sql::NOTIFY_FAILURE)
        .bind(id as i32)
        .bind("HTTP 500")
        .fetch_optional(c)
        .await
        .unwrap()
        .map(i64::from)
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn throttle_statements_hold_under_contention() {
    let Some(h) = Harness::start(setup()).await else {
        return;
    };
    let (mut a, mut b) = (plugin_db(&h).await, plugin_db(&h).await);
    let ix = add_indexer(&mut a, "t", json!({})).await;

    // Two sessions racing the first claim: exactly one holds the slot.
    let (x, y) = tokio::join!(
        claim(&mut a, ix, "a", 30.0, 2000),
        claim(&mut b, ix, "b", 30.0, 2000)
    );
    assert_eq!(x.is_some() as u8 + y.is_some() as u8, 1, "{x:?} {y:?}");
    let (holder, other) = if x.is_some() { ("a", "b") } else { ("b", "a") };
    assert_eq!(claim(&mut a, ix, "c", 30.0, 2000).await, None);
    assert_eq!(
        release(&mut a, ix, other, 2.0, "resolved").await,
        0,
        "a stranger's token frees nothing"
    );
    assert_eq!(release(&mut a, ix, holder, 2.0, "cancelled").await, 1);
    let wait = claim(&mut a, ix, "c", 30.0, 2000)
        .await
        .expect("free again");
    assert!(
        (1500..=2000).contains(&wait),
        "spacing from the release: {wait} ms"
    );

    // A slot given back before its request was sent keeps the spacing where it was.
    std::thread::sleep(Duration::from_millis(300));
    assert_eq!(release(&mut a, ix, "c", 2.0, "not-started").await, 1);
    let again = claim(&mut a, ix, "d", 30.0, 2000)
        .await
        .expect("free again");
    assert!(
        again <= wait - 250,
        "not pushed back: {again} ms after {wait} ms"
    );

    // A lease that ran out frees the slot; its holder's late release changes nothing.
    exec(&mut a, &format!(r#"UPDATE "indexer_throttle" SET "busyUntil" = now() - interval '1 second' WHERE "indexerId" = {ix}"#)).await;
    assert!(claim(&mut b, ix, "e", 30.0, 0).await.is_some());
    assert_eq!(release(&mut a, ix, "d", 0.0, "threw").await, 0);
    assert_eq!(release(&mut b, ix, "e", 0.0, "resolved").await, 1);

    // One ladder step per window, even when two failures race.
    let (x, y) = tokio::join!(fail(&mut a, ix), fail(&mut b, ix));
    assert_eq!((x.or(y), x.and(y)), (Some(1), None), "{x:?} {y:?}");
    exec(
        &mut a,
        &format!(
            r#"UPDATE "indexer_throttle" SET "cooldownUntil" = now() WHERE "indexerId" = {ix}"#
        ),
    )
    .await;
    assert_eq!(fail(&mut a, ix).await, Some(2));
    let secs = scalar_i64(
        &mut a,
        &format!(r#"SELECT extract(epoch FROM "cooldownUntil" - now())::bigint FROM "indexer_throttle" WHERE "indexerId" = {ix}"#),
    )
    .await;
    assert!((115..=120).contains(&secs), "step 2 is 2 min: {secs}");

    // Retry-After widens the window and names it; a shorter one only keeps the gate.
    let retry = async |c: &mut PgConnection, ms: f64, header: &str| {
        sqlx::query(sql::RETRY_AFTER)
            .bind(ix as i32)
            .bind(ms)
            .bind(header)
            .execute(c)
            .await
            .unwrap();
    };
    retry(&mut a, 600_000.0, "600").await;
    retry(&mut a, 1_000.0, "1").await;
    let (reason, detail): (String, String) = sqlx::query_as(
        r#"SELECT "cooldownReason", "cooldownDetail" FROM "indexer_throttle" WHERE "indexerId" = $1"#,
    )
    .bind(ix as i32)
    .fetch_one(&mut a)
    .await
    .unwrap();
    assert_eq!((reason.as_str(), detail.as_str()), ("rate-limit", "600"));
    let open: Vec<i32> = sqlx::query_scalar(AssertSqlSafe(format!(
        r#"SELECT "indexerId" FROM ({}) c"#,
        sql::COOLDOWNS
    )))
    .fetch_all(&mut a)
    .await
    .unwrap();
    assert_eq!(open, [ix as i32]);

    // A success resets the ladder and the window.
    assert!(claim(&mut a, ix, "f", 30.0, 0).await.is_some());
    assert_eq!(release(&mut a, ix, "f", 0.0, "resolved").await, 1);
    let after = scalar_i64(
        &mut a,
        &format!(r#"SELECT "failureCount"::bigint FROM "indexer_throttle" WHERE "indexerId" = {ix} AND "cooldownUntil" IS NULL"#),
    )
    .await;
    assert_eq!(after, 0);

    // `clearAll` counts only the windows that were open.
    let iy = add_indexer(&mut a, "u", json!({})).await;
    exec(&mut a, &format!(r#"INSERT INTO "indexer_throttle" ("indexerId", "failureCount", "cooldownUntil") VALUES ({iy}, 1, now() - interval '1 minute')"#)).await;
    assert_eq!(fail(&mut a, ix).await, Some(1));
    let cleared: i32 = sqlx::query_scalar(sql::CLEAR_ALL)
        .fetch_one(&mut a)
        .await
        .unwrap();
    assert_eq!(cleared, 1);
    assert_eq!(scalar_i64(&mut a, r#"SELECT count(*) FROM "indexer_throttle" WHERE "failureCount" <> 0 OR "cooldownUntil" IS NOT NULL"#).await, 0);
    h.finish().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn import_leases_rearm_only_expired_rows() {
    let Some(h) = Harness::start(setup()).await else {
        return;
    };
    let mut db = plugin_db(&h).await;
    let mut ids = vec![];
    for lease in [
        "now() - interval '1 minute'",
        "NULL",
        "now() + interval '10 minutes'",
    ] {
        let id: i32 = sqlx::query_scalar(AssertSqlSafe(format!(
            r#"INSERT INTO "download_history" ("sourceTitle", "quality", "status", "importLeaseUntil")
               VALUES ('x', 'HD', 'importing', {lease}) RETURNING "id""#
        )))
        .fetch_one(&mut db)
        .await
        .unwrap();
        ids.push(i64::from(id));
    }
    h.job("ImportCompleted", None).await.unwrap();
    let statuses = async |db: &mut PgConnection| {
        let mut s = vec![];
        for id in &ids {
            s.push(status_of(db, *id).await.0);
        }
        s
    };
    assert_eq!(statuses(&mut db).await, ["grabbed", "grabbed", "importing"]);
    assert!(logged(&h, "2 importing entries outlived their lease"));

    // `start` sweeps too, so a restart re-arms what a trapped job left behind.
    exec(&mut db, r#"UPDATE "download_history" SET "importLeaseUntil" = now() - interval '1 second' WHERE "status" = 'importing'"#).await;
    h.restart().await;
    until("the start sweep", MAX, async || {
        statuses(&mut plugin_db(&h).await).await == ["grabbed", "grabbed", "grabbed"]
    })
    .await;
    h.finish().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn an_indexer_save_probes_its_caps_once() {
    let Some(h) = Harness::start(setup()).await else {
        return;
    };
    // The indexer moved: the probe follows the redirect, as `fetch` did.
    let tz = Stub::start(|_, target, _| match target.strip_prefix("/api?") {
        Some(q) => (301, vec![("location", format!("/v2/api?{q}"))], vec![]),
        None => (200, vec![], CAPS.as_bytes().to_vec()),
    });
    route(&h, "torznab.test", "10.77.0.1", 80, &tz);
    let r = h
        .proxy(
            "POST",
            "/indexers",
            Some(&json!({
                "name": "tz", "implementation": "torznab", "useFor": ["auto", "manual"],
                "settings": { "baseUrl": "http://torznab.test/api", "apiKey": "k" }
            })),
        )
        .await;
    assert_eq!(r.status, 201, "{:?}", body(&r));
    let created = body(&r);
    assert_eq!(
        (
            &created["enableRss"],
            &created["enableSearch"],
            &created["enableInteractiveSearch"]
        ),
        (&json!(false), &json!(true), &json!(true))
    );
    let id = created["id"].as_i64().unwrap();
    let mut db = plugin_db(&h).await;
    until("the caps call", MAX, async || {
        scalar_i64(
            &mut plugin_db(&h).await,
            &format!(r#"SELECT count(*) FROM "indexers" WHERE "id" = {id} AND "capsProbedAt" IS NOT NULL"#),
        )
        .await
            == 1
    })
    .await;
    let (movie, params): (bool, Option<String>) = sqlx::query_as(
        r#"SELECT "capsMovieSearch", "capsMovieSearchParams" FROM "indexers" WHERE "id" = $1"#,
    )
    .bind(id as i32)
    .fetch_one(&mut db)
    .await
    .unwrap();
    assert_eq!((movie, params.as_deref()), (true, Some("q,imdbid")));
    let caps = tz.hits_to("/api?t=caps");
    assert_eq!(caps.len(), 1);
    assert_eq!(caps[0].target, "/api?t=caps&apikey=k");
    assert_eq!(tz.hits_to("/v2/api?t=caps&apikey=k").len(), 1);
    let sources = h.st.plugins.pickers.sources_of(&h.id);
    assert_eq!(sources.len(), 1, "the sources follow the learnt caps");
    let listed = body(&h.proxy("GET", "/indexers", None).await);
    assert_ne!(
        listed[0]["settings"]["apiKey"],
        json!("k"),
        "the key is redacted"
    );
    h.finish().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn two_searches_share_one_slot_per_indexer() {
    let Some(h) = Harness::start(setup()).await else {
        return;
    };
    let tz = Stub::start(|_, _, _| {
        std::thread::sleep(Duration::from_millis(300));
        (200, vec![], EMPTY_FEED.as_bytes().to_vec())
    });
    route(&h, "torznab.test", "10.77.0.1", 80, &tz);
    let mut db = plugin_db(&h).await;
    add_indexer(
        &mut db,
        "tz",
        json!({ "baseUrl": "http://torznab.test/api", "apiKey": "k" }),
    )
    .await;
    let media = add_movie(&h, "Some Movie").await;
    let path = format!("/{media}/releases");
    let (x, y) = tokio::join!(h.proxy("GET", &path, None), h.proxy("GET", &path, None));
    for r in [&x, &y] {
        assert_eq!((r.status, body(r)), (200, json!([])), "{:?}", body(r));
    }
    let mut hits = tz.hits();
    hits.sort_by_key(|h| h.start);
    assert_eq!(hits.len(), 2, "{hits:?}");
    let gap = hits[1].start.duration_since(hits[0].end);
    assert!(
        gap >= Duration::from_millis(900),
        "requestDelay spacing: {gap:?}"
    );
    assert!(
        hits[0].target.contains("t=search&q=Some%20Movie"),
        "{}",
        hits[0].target
    );
    let stats = body(&h.proxy("GET", "/indexers/1/stats", None).await);
    assert_eq!(stats[0]["queries"], json!(2), "{stats}");
    h.finish().await;
}

/// A series whose aired season 1 (10 episodes) is released as scene seasons 1 and 2.
async fn split_season(h: &Harness) -> (i64, i32) {
    let media = add_work(h, "Some Show", "series", None).await;
    let season: i32 = sqlx::query_scalar(
        r#"INSERT INTO item_groups ("mediaId", "number", monitored) VALUES ($1, 1, true) RETURNING id"#,
    )
    .bind(media as i32)
    .fetch_one(&h.pool)
    .await
    .unwrap();
    sqlx::raw_sql(AssertSqlSafe(format!(
        r#"INSERT INTO items ("mediaId", "editionId", "groupId", "number", monitored)
             SELECT {media}, (SELECT id FROM editions WHERE "mediaId" = {media}), {season}, n, true
             FROM generate_series(1, 10) n;
           INSERT INTO media_external_ids ("mediaId", kind, provider, value) VALUES ({media}, 'series', 'tvdb', '9');
           INSERT INTO scene_mappings ("tvdbId", entries) VALUES (9, '{{"entries": [{}]}}')"#,
        (1..=10)
            .map(|n| format!(
                r#"{{"sceneSeason": {}, "sceneEpisode": {}, "season": 1, "episode": {n}}}"#,
                if n <= 5 { 1 } else { 2 },
                if n <= 5 { n } else { n - 5 }
            ))
            .collect::<Vec<_>>()
            .join(",")
    )))
    .execute(&h.pool)
    .await
    .unwrap();
    (media, season)
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_split_season_issues_one_query_per_scene_season() {
    let Some(h) = Harness::start(setup()).await else {
        return;
    };
    let feed = |titles: &[&str]| {
        let items: String = titles
            .iter()
            .map(|t| format!("<item><title>{t}</title><link>http://dl.test/{t}.torrent</link><size>1000</size></item>"))
            .collect();
        format!(r#"<?xml version="1.0"?><rss><channel>{items}</channel></rss>"#).into_bytes()
    };
    // Both scene seasons list `Some.Show.S02.B`: it must come back once.
    let (first, second) = (
        feed(&["Some.Show.S01.A", "Some.Show.S02.B"]),
        feed(&["Some.Show.S02.B", "Some.Show.S02.C"]),
    );
    let tz = Stub::start(move |_, target, _| {
        let feed = if target.contains("Show%20S01") {
            &first
        } else {
            &second
        };
        (200, vec![], feed.clone())
    });
    route(&h, "torznab.test", "10.77.0.1", 80, &tz);
    let mut db = plugin_db(&h).await;
    add_indexer(
        &mut db,
        "tz",
        json!({ "baseUrl": "http://torznab.test/api", "apiKey": "k" }),
    )
    .await;
    let (media, season) = split_season(&h).await;
    let r = h
        .proxy("GET", &format!("/{media}/groups/{season}/releases"), None)
        .await;
    assert_eq!(r.status, 200, "{:?}", body(&r));
    let queries: Vec<String> = tz
        .hits()
        .into_iter()
        .map(|h| h.target)
        .filter(|t| t.contains("q=Some%20Show"))
        .collect();
    assert_eq!(queries.len(), 2, "{queries:?}");
    assert!(queries.iter().any(|q| q.contains("Some%20Show%20S01")));
    assert!(queries.iter().any(|q| q.contains("Some%20Show%20S02")));
    let titles: Vec<String> = body(&r)
        .as_array()
        .unwrap()
        .iter()
        .map(|x| x["title"].as_str().unwrap_or_default().to_owned())
        .collect();
    assert_eq!(titles.len(), 3, "merged and deduplicated: {titles:?}");
    h.finish().await;
}

fn one_item_feed(title: &str) -> Vec<u8> {
    format!(
        r#"<?xml version="1.0"?><rss><channel><item><title>{title}</title><link>http://dl.test/{title}.torrent</link><size>1000</size></item></channel></rss>"#
    )
    .into_bytes()
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn indexers_are_searched_at_once_and_streamed_as_they_answer() {
    let Some(h) = Harness::start(setup()).await else {
        return;
    };
    let answer_after = |ms: u64, title: &str| {
        let feed = one_item_feed(title);
        Stub::start(move |_, _, _| {
            std::thread::sleep(Duration::from_millis(ms));
            (200, vec![], feed.clone())
        })
    };
    let slow = answer_after(1500, "Some.Movie.2020.1080p.WEB");
    let fast = answer_after(500, "Some.Movie.2020.720p.WEB");
    route(&h, "slow.test", "10.77.0.1", 80, &slow);
    route(&h, "fast.test", "10.77.0.3", 80, &fast);
    let mut db = plugin_db(&h).await;
    let a = add_indexer(
        &mut db,
        "slow",
        json!({ "baseUrl": "http://slow.test/api", "apiKey": "k" }),
    )
    .await;
    let b = add_indexer(
        &mut db,
        "fast",
        json!({ "baseUrl": "http://fast.test/api", "apiKey": "k" }),
    )
    .await;
    let media = add_movie(&h, "Some Movie").await;

    let mut events = h.st.hub.subscribe();
    let path = format!("/{media}/releases");
    let r = h
        .http(request("GET", &path, &[("sid", "s1")], b""))
        .await
        .unwrap();
    assert_eq!(r.status, 200, "{:?}", body(&r));
    assert_eq!(body(&r).as_array().map(Vec::len), Some(2), "{:?}", body(&r));
    let (s, f) = (slow.hits(), fast.hits());
    assert_eq!((s.len(), f.len()), (1, 1));
    assert!(
        f[0].start < s[0].end && s[0].start < f[0].end,
        "both requests in flight together: {s:?} {f:?}"
    );

    let state = |p: &Value, id: i64| {
        p["indexers"]
            .as_array()
            .and_then(|l| l.iter().find(|i| i["id"] == json!(id)))
            .map(|i| i["state"].as_str().unwrap_or("").to_owned())
            .unwrap_or_default()
    };
    let mut seen = vec![];
    while let Ok(ev) = events.try_recv() {
        if let Bus::Raw(raw) = &*ev
            && let Some(kind) = raw.r#type.strip_prefix("plugin.fliks.download.search.")
        {
            let n = raw.payload["releases"].as_array().map(Vec::len);
            seen.push((
                kind.to_owned(),
                state(&raw.payload, a),
                state(&raw.payload, b),
                n,
            ));
        }
    }
    let row =
        |k: &str, x: &str, y: &str, n: Option<usize>| (k.to_owned(), x.to_owned(), y.to_owned(), n);
    assert_eq!(
        seen,
        [
            row("state", "pending", "pending", None),
            row("partial", "pending", "done", Some(1)),
            row("partial", "done", "done", Some(2)),
        ]
    );
    h.finish().await;
}

/// The last streamed state of each indexer.
fn final_states(events: &mut tokio::sync::broadcast::Receiver<Arc<Bus>>) -> Vec<(i64, String)> {
    let mut last = vec![];
    while let Ok(ev) = events.try_recv() {
        if let Bus::Raw(raw) = &*ev
            && raw.r#type.starts_with("plugin.fliks.download.search.")
        {
            last = raw.payload["indexers"]
                .as_array()
                .into_iter()
                .flatten()
                .map(|i| {
                    (
                        i["id"].as_i64().unwrap_or(0),
                        i["state"].as_str().unwrap_or("").into(),
                    )
                })
                .collect();
        }
    }
    last.sort();
    last
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_round_out_of_time_drops_its_requests_without_pushing_the_spacing() {
    let Some(h) = Harness::start(setup()).await else {
        return;
    };
    h.set_config("search_budget_seconds", Some("5")).await;
    let waiting = Stub::start(|_, _, _| (200, vec![], one_item_feed("Never.Asked.2020")));
    let asked = Arc::new(std::sync::atomic::AtomicUsize::new(0));
    let seen = asked.clone();
    let hanging = Stub::start(move |_, _, _| {
        seen.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        std::thread::sleep(Duration::from_secs(10));
        (200, vec![], one_item_feed("Too.Late.2020"))
    });
    let fast = Stub::start(|_, _, _| (200, vec![], one_item_feed("Some.Movie.2020.1080p.WEB")));
    route(&h, "waiting.test", "10.77.0.1", 80, &waiting);
    route(&h, "hanging.test", "10.77.0.3", 80, &hanging);
    route(&h, "fast.test", "10.77.0.4", 80, &fast);
    let mut db = plugin_db(&h).await;
    let mut add = async |name: &str, delay: i64| {
        let url = format!("http://{name}.test/api");
        let id = add_indexer(&mut db, name, json!({ "baseUrl": url, "apiKey": "k" })).await;
        exec(
            &mut db,
            &format!(
                r#"UPDATE "indexers" SET "requestDelay" = {delay} WHERE "id" = {id};
            INSERT INTO "indexer_throttle" ("indexerId", "nextAllowedAt")
            VALUES ({id}, now() + make_interval(secs => {delay}))"#
            ),
        )
        .await;
        id
    };
    let (a, b, c) = (
        add("waiting", 10).await,
        add("hanging", 2).await,
        add("fast", 0).await,
    );
    let next_of = |id: i64| {
        format!(r#"SELECT "nextAllowedAt"::text FROM "indexer_throttle" WHERE "indexerId" = {id}"#)
    };
    let before: String = sqlx::query_scalar(AssertSqlSafe(next_of(a)))
        .fetch_one(&mut db)
        .await
        .unwrap();
    let media = add_movie(&h, "Some Movie").await;

    let mut events = h.st.hub.subscribe();
    let started = Instant::now();
    let path = format!("/{media}/releases");
    let r = h
        .http(request("GET", &path, &[("sid", "s1")], b""))
        .await
        .unwrap();
    assert!(
        started.elapsed() < Duration::from_secs(7),
        "{:?}",
        started.elapsed()
    );
    assert_eq!(r.status, 200, "{:?}", body(&r));
    let titles: Vec<Value> = body(&r)
        .as_array()
        .into_iter()
        .flatten()
        .map(|x| x["title"].clone())
        .collect();
    assert_eq!(
        titles,
        [json!("Some.Movie.2020.1080p.WEB")],
        "{:?}",
        body(&r)
    );

    // Never sent: no hit, the slot free and the spacing where it was.
    assert_eq!(waiting.hits().len(), 0);
    let after: String = sqlx::query_scalar(AssertSqlSafe(next_of(a)))
        .fetch_one(&mut db)
        .await
        .unwrap();
    assert_eq!(after, before);
    // Sent, then cut by the round: released as cancelled, no failure step.
    assert_eq!(asked.load(std::sync::atomic::Ordering::SeqCst), 1);
    let (busy, failures, cooling, next_s): (Option<String>, i32, Option<String>, f64) =
        sqlx::query_as(AssertSqlSafe(format!(
            r#"SELECT "busyToken", "failureCount", "cooldownUntil"::text,
                 extract(epoch FROM "nextAllowedAt" - now())::float8
               FROM "indexer_throttle" WHERE "indexerId" = {b}"#
        )))
        .fetch_one(&mut db)
        .await
        .unwrap();
    assert_eq!((busy, failures, cooling), (None, 0, None));
    assert!(
        (0.5..=2.1).contains(&next_s),
        "spacing from the cut: {next_s}"
    );
    let free: i64 = scalar_i64(
        &mut db,
        &format!(r#"SELECT count(*) FROM "indexer_throttle" WHERE "indexerId" IN ({a}, {c}) AND "busyToken" IS NULL"#),
    )
    .await;
    assert_eq!(free, 2);
    let mut want = vec![
        (a, "failed".to_owned()),
        (b, "failed".into()),
        (c, "done".into()),
    ];
    want.sort();
    assert_eq!(final_states(&mut events), want);
    h.finish().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_manual_grab_reaches_the_client_and_the_queue_actions_drive_it() {
    let Some(h) = Harness::start(setup()).await else {
        return;
    };
    let qbit = fake_qbit();
    let client = add_qbit(&h, &qbit).await;
    let media = add_movie(&h, "Some Movie").await;
    let magnet = format!("magnet:?xt=urn:btih:{HASH}&dn=Some.Movie.2020.1080p");
    let r = h
        .proxy(
            "POST",
            &format!("/{media}/grab"),
            Some(&json!({ "downloadUrl": magnet, "force": true, "infoUrl": "javascript:x" })),
        )
        .await;
    assert_eq!(
        (r.status, body(&r)),
        (200, json!({ "torrentHash": HASH })),
        "{:?}",
        body(&r)
    );
    let add = qbit.hits_to("/api/v2/torrents/add");
    assert_eq!(add.len(), 1);
    assert!(add[0].body.contains("category=movies"), "{}", add[0].body);
    until("the grab-started call", MAX, async || {
        logged(&h, "grabbed \"Some.Movie.2020.1080p\"") && h.scheduled_calls().await.is_empty()
    })
    .await;

    let mut db = plugin_db(&h).await;
    let (row, info): (i32, Option<String>) = sqlx::query_as(
        r#"SELECT "id", "infoUrl" FROM "download_history" WHERE "torrentHash" = $1 AND "grabSource" = 'manual' AND "downloadClientId" = $2"#,
    )
    .bind(HASH)
    .bind(client as i32)
    .fetch_one(&mut db)
    .await
    .unwrap();
    assert_eq!(info, None, "only an http(s) info URL is kept");
    let row = i64::from(row);

    h.job("ImportCompleted", None).await.unwrap();
    assert_eq!(
        queue_rows(&h).await,
        [(ref_of(&h, row).await, "active".into(), 0.5)]
    );

    assert_eq!(act(&h, "pause", row).await.status, 200);
    assert_eq!(qbit.hits_to("/api/v2/torrents/stop").len(), 1);
    assert_eq!(queue_rows(&h).await[0].1, "paused");
    assert_eq!(act(&h, "resume", row).await.status, 200);
    assert_eq!(qbit.hits_to("/api/v2/torrents/start").len(), 1);
    assert_eq!(queue_rows(&h).await[0].1, "active");

    assert_eq!(act(&h, "remove", row).await.status, 200);
    let del = qbit.hits_to("/api/v2/torrents/delete");
    assert!(del[0].body.contains("deleteFiles=true"), "{}", del[0].body);
    assert_eq!(
        status_of(&mut db, row).await,
        (
            "failed".into(),
            Some("download.queue.removed_by_user".into())
        )
    );
    assert_eq!(queue_rows(&h).await[0].1, "removed");
    let r = act(&h, "pause", row).await;
    assert_eq!(
        (r.status, body(&r)),
        (
            409,
            json!({ "error": { "key": "download.queue.errors.not_controllable", "detail": "failed" } })
        )
    );
    h.finish().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn an_unmatched_row_is_held_and_an_importing_one_removed() {
    let Some(h) = Harness::start(setup()).await else {
        return;
    };
    let qbit = fake_qbit();
    add_qbit(&h, &qbit).await;
    let media = add_movie(&h, "Some Movie").await;
    let magnet = format!("magnet:?xt=urn:btih:{HASH}&dn=Some.Movie.2020.1080p");
    let r = h
        .proxy(
            "POST",
            &format!("/{media}/grab"),
            Some(&json!({ "downloadUrl": magnet, "force": true })),
        )
        .await;
    assert_eq!(r.status, 200, "{:?}", body(&r));
    let mut db = plugin_db(&h).await;
    let row: i32 = sqlx::query_scalar(r#"SELECT "id" FROM "download_history" LIMIT 1"#)
        .fetch_one(&mut db)
        .await
        .unwrap();
    let row = i64::from(row);

    // The client holds it but the row never learnt its hash: a pause matches it first.
    exec(
        &mut db,
        format!(r#"UPDATE "download_history" SET "torrentHash" = NULL WHERE "id" = {row}"#),
    )
    .await;
    assert_eq!(act(&h, "pause", row).await.status, 200);
    assert_eq!(qbit.hits_to("/api/v2/torrents/stop").len(), 1);

    // An importing row is retired and its torrent stays.
    exec(&mut db, format!(r#"UPDATE "download_history" SET "status" = 'importing', "importLeaseUntil" = now() + interval '1 hour' WHERE "id" = {row}"#)).await;
    assert_eq!(act(&h, "remove", row).await.status, 200);
    assert!(qbit.hits_to("/api/v2/torrents/delete").is_empty());
    assert_eq!(
        status_of(&mut db, row).await,
        (
            "failed".into(),
            Some("download.queue.removed_by_user".into())
        )
    );
    sqlx::query_scalar::<_, i32>(r#"SELECT 1"#)
        .fetch_one(&mut db)
        .await
        .unwrap();
    exec(&mut db, format!(r#"UPDATE "download_history" SET "updatedAt" = now() WHERE "id" = {row} AND "status" = 'importing'"#)).await;
    h.job("ImportCompleted", None).await.unwrap();
    assert_eq!(queue_rows(&h).await[0].1, "removed");
    h.finish().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_grab_reaches_transmission_and_the_queue_actions_drive_it() {
    let Some(h) = Harness::start(setup()).await else {
        return;
    };
    let tr = fake_transmission();
    route(&h, "transmission.test", "10.77.0.3", 9091, &tr);
    let settings = json!({ "host": "transmission.test", "port": 9091, "username": "u", "password": "pw", "category": "fliks", "movieCategory": "movies" });
    let r = h
        .proxy(
            "POST",
            "/download-clients/test-connection",
            Some(&json!({ "implementation": "transmission", "settings": settings })),
        )
        .await;
    assert_eq!(body(&r)["ok"], json!(true), "{:?}", body(&r));
    let r = h
        .proxy(
            "POST",
            "/download-clients",
            Some(&json!({ "name": "tr", "implementation": "transmission", "settings": settings })),
        )
        .await;
    assert_eq!(r.status, 201, "{:?}", body(&r));
    let media = add_movie(&h, "Some Movie").await;
    let magnet = format!("magnet:?xt=urn:btih:{HASH}&dn=Some.Movie.2020.1080p");
    let r = h
        .proxy(
            "POST",
            &format!("/{media}/grab"),
            Some(&json!({ "downloadUrl": magnet, "force": true })),
        )
        .await;
    assert_eq!(
        (r.status, body(&r)),
        (200, json!({ "torrentHash": HASH })),
        "{:?}",
        body(&r)
    );
    assert_eq!(
        rpc_calls(&tr, "torrent-add")[0]["arguments"]["filename"],
        json!(magnet)
    );
    assert_eq!(
        rpc_calls(&tr, "torrent-set")[0]["arguments"],
        json!({ "ids": [HASH], "labels": ["movies"] })
    );

    let row: i32 = sqlx::query_scalar(r#"SELECT "id" FROM "download_history""#)
        .fetch_one(&mut plugin_db(&h).await)
        .await
        .unwrap();
    let row = i64::from(row);
    h.job("ImportCompleted", None).await.unwrap();
    assert_eq!(
        queue_rows(&h).await,
        [(ref_of(&h, row).await, "active".into(), 0.5)]
    );
    assert_eq!(act(&h, "pause", row).await.status, 200);
    assert_eq!(queue_rows(&h).await[0].1, "paused");
    assert_eq!(act(&h, "resume", row).await.status, 200);
    assert_eq!(rpc_calls(&tr, "torrent-start").len(), 1);
    assert_eq!(act(&h, "remove", row).await.status, 200);
    assert_eq!(
        rpc_calls(&tr, "torrent-remove")[0]["arguments"],
        json!({ "ids": [HASH], "delete-local-data": true })
    );
    assert_eq!(queue_rows(&h).await[0].1, "removed");
    h.finish().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn the_failure_ladder_follows_the_recorded_one() {
    let Some(h) = Harness::start(setup()).await else {
        return;
    };
    let g: Value = serde_json::from_str(GOLDEN).unwrap();
    let steps: Vec<i64> = g["throttleLadder"]["steps"]
        .as_array()
        .unwrap()
        .iter()
        .map(|v| v.as_i64().unwrap() / 1000)
        .collect();
    let mono: Vec<i64> = g["throttleLadder"]["mono"]
        .as_array()
        .unwrap()
        .iter()
        .map(|v| v.as_i64().unwrap() / 1000)
        .collect();
    let mut db = plugin_db(&h).await;
    let remaining = async |c: &mut PgConnection, ix: i64| {
        scalar_i64(
            c,
            &format!(r#"SELECT round(extract(epoch FROM "cooldownUntil" - now()))::bigint FROM "indexer_throttle" WHERE "indexerId" = {ix}"#),
        )
        .await
    };
    // The first step answers; a second failure inside the open window answers nothing.
    let ix = add_indexer(&mut db, "ladder", json!({})).await;
    assert!(claim(&mut db, ix, "a", 30.0, 0).await.is_some());
    release(&mut db, ix, "a", 0.0, "cancelled").await;
    for i in 0..7 {
        assert!(fail(&mut db, ix).await.is_some(), "step {i}");
        assert!((remaining(&mut db, ix).await - steps[2 * i]).abs() <= 2);
        assert_eq!(fail(&mut db, ix).await, None, "inside the window");
        assert!((remaining(&mut db, ix).await - steps[2 * i + 1]).abs() <= 2);
        exec(
            &mut db,
            &format!(
                r#"UPDATE "indexer_throttle" SET "cooldownUntil" = now() WHERE "indexerId" = {ix}"#
            ),
        )
        .await;
    }
    assert!(claim(&mut db, ix, "b", 30.0, 0).await.is_some());
    release(&mut db, ix, "b", 0.0, "resolved").await;
    assert!(fail(&mut db, ix).await.is_some());
    assert!((remaining(&mut db, ix).await - steps[14]).abs() <= 2);

    // A shorter `Retry-After` never narrows an open window.
    let iy = add_indexer(&mut db, "mono", json!({})).await;
    assert!(claim(&mut db, iy, "c", 30.0, 0).await.is_some());
    release(&mut db, iy, "c", 0.0, "cancelled").await;
    assert!(fail(&mut db, iy).await.is_some());
    for (header, expected) in ["10", "120", "60"].iter().zip(&mono) {
        sqlx::query(sql::RETRY_AFTER)
            .bind(iy as i32)
            .bind(header.parse::<f64>().unwrap() * 1000.0)
            .bind(*header)
            .execute(&mut db)
            .await
            .unwrap();
        assert!(
            (remaining(&mut db, iy).await - expected).abs() <= 2,
            "{header}"
        );
    }
    h.finish().await;
}

/// One request and the answer the TypeScript plugin's tests froze for it.
struct Case {
    method: &'static str,
    route: &'static str,
    path: String,
    body: Option<Value>,
    status: u16,
    /// `Some((key, detail))` for an `{error: {key, detail}}` body; `None` checks only `ok`.
    error: Option<(&'static str, Option<String>)>,
    ok: Option<Value>,
}

fn err(
    method: &'static str,
    route: &'static str,
    path: &str,
    body: Option<Value>,
    status: u16,
    key: &'static str,
    detail: Option<&str>,
) -> Case {
    Case {
        method,
        route,
        path: path.into(),
        body,
        status,
        error: Some((key, detail.map(str::to_owned))),
        ok: None,
    }
}

fn okc(method: &'static str, route: &'static str, path: &str, ok: Option<Value>) -> Case {
    Case {
        method,
        route,
        path: path.into(),
        body: None,
        status: 200,
        error: None,
        ok,
    }
}

/// A `POST` to a queue action with `body`, answered by an `{error}`.
fn action(
    path: &'static str,
    body: Value,
    status: u16,
    key: &'static str,
    detail: Option<&str>,
) -> Case {
    err("POST", path, path, Some(body), status, key, detail)
}

const BAD_PARAM: &str = "download.http.errors.bad_param";
const BAD_BODY: &str = "download.http.errors.bad_body";
const NOT_FOUND: &str = "download.http.errors.not_found";

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn all_35_routes_answer_the_frozen_statuses() {
    let Some(h) = Harness::start(setup()).await else {
        return;
    };
    let mut db = plugin_db(&h).await;
    let mut row = async |status: &str, extra: &str| -> i64 {
        scalar_i64(
            &mut db,
            &format!(
                r#"INSERT INTO "download_history" ("sourceTitle", "quality", "status"{}) VALUES ('x', 'HD', '{status}'{}) RETURNING "id"::bigint"#,
                if extra.is_empty() { "" } else { ", \"statusMessage\"" },
                if extra.is_empty() { String::new() } else { format!(", '{extra}'") },
            ),
        )
        .await
    };
    let completed = row("completed", "").await;
    let failed = row("failed", "boom").await;
    let grabbed = row("grabbed", "").await;
    let refs = (
        ref_of(&h, completed).await,
        ref_of(&h, failed).await,
        ref_of(&h, grabbed).await,
    );
    let (completed_ref, failed_ref, grabbed_ref) = (&refs.0, &refs.1, &refs.2);
    let (completed, failed, grabbed) = (
        completed.to_string(),
        failed.to_string(),
        grabbed.to_string(),
    );
    let (completed, grabbed) = (&completed, &grabbed);

    let not_controllable = "download.queue.errors.not_controllable";
    let cases = vec![
        err(
            "GET",
            "/:id/releases",
            "/abc/releases",
            None,
            400,
            BAD_PARAM,
            Some("id"),
        ),
        err(
            "GET",
            "/:id/releases",
            "/999/releases",
            None,
            404,
            "download.grab.errors.media_not_found",
            Some("999"),
        ),
        err(
            "POST",
            "/:id/grab",
            "/abc/grab",
            None,
            400,
            BAD_PARAM,
            Some("id"),
        ),
        err(
            "POST",
            "/:id/grab",
            "/999/grab",
            Some(json!({ "downloadUrl": "magnet:?xt=urn:btih:abc" })),
            404,
            "download.grab.errors.media_not_found",
            Some("999"),
        ),
        err(
            "POST",
            "/:id/grab",
            "/999/grab",
            None,
            400,
            BAD_PARAM,
            Some("downloadUrl"),
        ),
        err(
            "GET",
            "/:id/groups/:groupId/releases",
            "/1/groups/x/releases",
            None,
            400,
            BAD_PARAM,
            Some("groupId"),
        ),
        err(
            "POST",
            "/:id/groups/:groupId/grab",
            "/1/groups/0/grab",
            None,
            400,
            BAD_PARAM,
            Some("groupId"),
        ),
        err(
            "POST",
            "/:id/grab",
            "/1/grab",
            Some(json!({ "family": "video3" })),
            400,
            BAD_PARAM,
            Some("family"),
        ),
        err(
            "GET",
            "/:id/items/:itemId/releases",
            "/1/items/-1/releases",
            None,
            400,
            BAD_PARAM,
            Some("itemId"),
        ),
        err(
            "POST",
            "/:id/items/:itemId/grab",
            "/1/items/01/grab",
            None,
            400,
            BAD_PARAM,
            Some("itemId"),
        ),
        okc("GET", "/indexers", "/indexers", Some(json!([]))),
        err(
            "POST",
            "/indexers",
            "/indexers",
            Some(json!({})),
            400,
            BAD_BODY,
            Some("name"),
        ),
        err(
            "POST",
            "/indexers",
            "/indexers",
            Some(json!({ "name": "x" })),
            400,
            BAD_BODY,
            Some("implementation"),
        ),
        err(
            "POST",
            "/indexers",
            "/indexers",
            Some(json!({ "name": "x", "implementation": "torznab", "useFor": ["bogus"] })),
            400,
            BAD_BODY,
            Some("useFor"),
        ),
        err(
            "POST",
            "/indexers",
            "/indexers",
            Some(json!({ "name": "x", "implementation": "torznab", "useFor": [] })),
            400,
            "download.config.indexers.errors.no_usage",
            None,
        ),
        err(
            "POST",
            "/indexers",
            "/indexers",
            Some(json!({ "name": "x", "implementation": "nope" })),
            400,
            BAD_BODY,
            Some("implementation"),
        ),
        err(
            "POST",
            "/indexers/test-connection",
            "/indexers/test-connection",
            Some(json!({})),
            400,
            BAD_BODY,
            Some("implementation"),
        ),
        okc(
            "DELETE",
            "/indexers/cooldowns",
            "/indexers/cooldowns",
            Some(json!({ "cleared": 0 })),
        ),
        okc(
            "GET",
            "/indexers/implementations",
            "/indexers/implementations",
            None,
        ),
        err(
            "PUT",
            "/indexers/:id",
            "/indexers/abc",
            Some(json!({})),
            400,
            BAD_PARAM,
            Some("id"),
        ),
        err(
            "PUT",
            "/indexers/:id",
            "/indexers/999",
            Some(json!({})),
            404,
            NOT_FOUND,
            Some("Indexer #999 not found"),
        ),
        err(
            "PUT",
            "/indexers/:id",
            "/indexers/999",
            Some(json!({ "useFor": [] })),
            400,
            "download.config.indexers.errors.no_usage",
            None,
        ),
        err(
            "DELETE",
            "/indexers/:id",
            "/indexers/abc",
            None,
            400,
            BAD_PARAM,
            Some("id"),
        ),
        err(
            "DELETE",
            "/indexers/:id",
            "/indexers/999",
            None,
            404,
            NOT_FOUND,
            Some("Indexer #999 not found"),
        ),
        err(
            "DELETE",
            "/indexers/:id/cooldown",
            "/indexers/abc/cooldown",
            None,
            400,
            BAD_PARAM,
            Some("id"),
        ),
        err(
            "DELETE",
            "/indexers/:id/cooldown",
            "/indexers/999/cooldown",
            None,
            404,
            NOT_FOUND,
            Some("Indexer #999 not found"),
        ),
        err(
            "GET",
            "/indexers/:id/stats",
            "/indexers/abc/stats",
            None,
            400,
            BAD_PARAM,
            Some("id"),
        ),
        okc(
            "GET",
            "/indexers/:id/stats",
            "/indexers/999/stats",
            Some(json!([])),
        ),
        okc(
            "GET",
            "/indexer-sources",
            "/indexer-sources",
            Some(json!([])),
        ),
        err(
            "POST",
            "/indexer-sources",
            "/indexer-sources",
            Some(json!({})),
            400,
            BAD_BODY,
            Some("name"),
        ),
        err(
            "POST",
            "/indexer-sources",
            "/indexer-sources",
            Some(json!({ "name": "s", "implementation": "nope" })),
            400,
            BAD_BODY,
            Some("implementation"),
        ),
        err(
            "POST",
            "/indexer-sources/test-connection",
            "/indexer-sources/test-connection",
            Some(json!({})),
            400,
            BAD_BODY,
            Some("implementation"),
        ),
        okc(
            "GET",
            "/indexer-sources/implementations",
            "/indexer-sources/implementations",
            None,
        ),
        err(
            "PUT",
            "/indexer-sources/:id",
            "/indexer-sources/abc",
            Some(json!({})),
            400,
            BAD_PARAM,
            Some("id"),
        ),
        err(
            "PUT",
            "/indexer-sources/:id",
            "/indexer-sources/999",
            Some(json!({})),
            404,
            NOT_FOUND,
            Some("Indexer source #999 not found"),
        ),
        err(
            "DELETE",
            "/indexer-sources/:id",
            "/indexer-sources/abc",
            None,
            400,
            BAD_PARAM,
            Some("id"),
        ),
        err(
            "DELETE",
            "/indexer-sources/:id",
            "/indexer-sources/999",
            None,
            404,
            NOT_FOUND,
            Some("Indexer source #999 not found"),
        ),
        err(
            "POST",
            "/indexer-sources/:id/import",
            "/indexer-sources/abc/import",
            None,
            400,
            BAD_PARAM,
            Some("id"),
        ),
        err(
            "POST",
            "/indexer-sources/:id/import",
            "/indexer-sources/999/import",
            None,
            404,
            NOT_FOUND,
            Some("Indexer source #999 not found"),
        ),
        okc(
            "GET",
            "/download-clients",
            "/download-clients",
            Some(json!([])),
        ),
        err(
            "POST",
            "/download-clients",
            "/download-clients",
            Some(json!({})),
            400,
            BAD_BODY,
            Some("name"),
        ),
        err(
            "POST",
            "/download-clients",
            "/download-clients",
            Some(json!({ "name": "d", "implementation": "deluge" })),
            400,
            BAD_BODY,
            Some("implementation"),
        ),
        err(
            "POST",
            "/download-clients/test-connection",
            "/download-clients/test-connection",
            Some(json!({})),
            400,
            BAD_BODY,
            Some("implementation"),
        ),
        okc(
            "GET",
            "/download-clients/implementations",
            "/download-clients/implementations",
            None,
        ),
        err(
            "PUT",
            "/download-clients/:id",
            "/download-clients/abc",
            Some(json!({})),
            400,
            BAD_PARAM,
            Some("id"),
        ),
        err(
            "PUT",
            "/download-clients/:id",
            "/download-clients/999",
            Some(json!({})),
            404,
            NOT_FOUND,
            Some("Download client #999 not found"),
        ),
        err(
            "DELETE",
            "/download-clients/:id",
            "/download-clients/abc",
            None,
            400,
            BAD_PARAM,
            Some("id"),
        ),
        err(
            "DELETE",
            "/download-clients/:id",
            "/download-clients/999",
            None,
            404,
            NOT_FOUND,
            Some("Download client #999 not found"),
        ),
        // `{ref}` is missing, not a number, unknown, or names a row the action cannot drive.
        action("/queue/pause", json!({}), 400, BAD_PARAM, Some("ref")),
        action(
            "/queue/pause",
            json!({ "ref": "abc" }),
            400,
            BAD_PARAM,
            Some("ref"),
        ),
        action(
            "/queue/pause",
            json!({ "ref": 7 }),
            400,
            BAD_PARAM,
            Some("ref"),
        ),
        action(
            "/queue/pause",
            json!({ "ref": "999" }),
            404,
            NOT_FOUND,
            Some("999"),
        ),
        action(
            "/queue/pause",
            json!({ "ref": format!("{completed}-1") }),
            404,
            NOT_FOUND,
            Some(&format!("{completed}-1")),
        ),
        action(
            "/queue/pause",
            json!({ "ref": completed_ref }),
            409,
            not_controllable,
            Some("completed"),
        ),
        action(
            "/queue/pause",
            json!({ "ref": grabbed_ref }),
            409,
            "download.queue.errors.no_torrent",
            Some(grabbed),
        ),
        action(
            "/queue/resume",
            json!({ "ref": "999" }),
            404,
            NOT_FOUND,
            Some("999"),
        ),
        action(
            "/queue/resume",
            json!({ "ref": completed_ref }),
            409,
            not_controllable,
            Some("completed"),
        ),
        action(
            "/queue/remove",
            json!({ "ref": "999" }),
            404,
            NOT_FOUND,
            Some("999"),
        ),
        action(
            "/queue/remove",
            json!({ "ref": completed_ref }),
            409,
            not_controllable,
            Some("completed"),
        ),
        action(
            "/queue/retry-import",
            json!({ "ref": "abc" }),
            400,
            BAD_PARAM,
            Some("ref"),
        ),
        action(
            "/queue/retry-import",
            json!({ "ref": "999" }),
            404,
            NOT_FOUND,
            Some("999"),
        ),
        action(
            "/queue/retry-import",
            json!({ "ref": failed_ref }),
            409,
            "download.queue.errors.not_retryable",
            Some("failed"),
        ),
        okc(
            "GET",
            "/blocklist",
            "/blocklist",
            Some(json!({ "data": [], "total": 0, "page": 1, "pageSize": 25 })),
        ),
        okc(
            "DELETE",
            "/blocklist/all",
            "/blocklist/all",
            Some(json!({})),
        ),
        err(
            "DELETE",
            "/blocklist/:id",
            "/blocklist/abc",
            None,
            400,
            BAD_PARAM,
            Some("id"),
        ),
        err(
            "DELETE",
            "/blocklist/:id",
            "/blocklist/999",
            None,
            404,
            NOT_FOUND,
            Some("999"),
        ),
    ];
    for c in &cases {
        // Straight to the plugin: the host's own guards (`:id` must be a media) are not under test.
        let payload = c.body.as_ref().map(Value::to_string).unwrap_or_default();
        let r = h
            .http(request(c.method, &c.path, &[], payload.as_bytes()))
            .await
            .unwrap();
        let b = body(&r);
        let at = format!("{} {} {:?}", c.method, c.path, b);
        assert_eq!(r.status, c.status, "{at}");
        match &c.error {
            Some((key, detail)) => {
                let mut e = json!({ "key": key });
                if let Some(d) = detail {
                    e["detail"] = json!(d);
                }
                assert_eq!(b, json!({ "error": e }), "{at}");
            }
            None => {
                if let Some(ok) = &c.ok {
                    assert_eq!(&b, ok, "{at}");
                }
            }
        }
    }

    let table: Vec<(String, String)> = fk_plugin_download::routes::table()
        .into_iter()
        .map(|(m, p, _)| (m.to_owned(), p.to_owned()))
        .collect();
    assert_eq!(table.len(), 35);
    for (m, p) in &table {
        assert!(
            cases.iter().any(|c| c.method == m && c.route == p),
            "{m} {p} has no case"
        );
    }
    h.finish().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_client_that_refuses_or_is_missing_answers_502() {
    let Some(h) = Harness::start(setup()).await else {
        return;
    };
    let refusing = Stub::start(
        |_, target, _| match target.split('?').next().unwrap_or("") {
            "/api/v2/auth/login" => (
                200,
                vec![("set-cookie", "SID=s; path=/".into())],
                b"Ok.".to_vec(),
            ),
            "/api/v2/torrents/info" => (200, vec![], b"[]".to_vec()),
            _ => (403, vec![], vec![]),
        },
    );
    let client = add_qbit(&h, &refusing).await;
    let media = add_movie(&h, "Some Movie").await;
    let grab = json!({ "downloadUrl": format!("magnet:?xt=urn:btih:{HASH}&dn=Some.Movie.2020.1080p"), "force": true });
    let r = h
        .proxy("POST", &format!("/{media}/grab"), Some(&grab))
        .await;
    assert_eq!(
        (r.status, body(&r)),
        (
            502,
            json!({ "error": { "key": "download.http.errors.download_client", "detail": "the download client refused the torrent (HTTP 403)" } })
        )
    );

    let r = h
        .proxy(
            "PUT",
            &format!("/download-clients/{client}"),
            Some(&json!({ "settings": { "host": "" } })),
        )
        .await;
    assert_eq!(r.status, 200, "{:?}", body(&r));
    let r = h
        .proxy("POST", &format!("/{media}/grab"), Some(&grab))
        .await;
    assert_eq!(
        (r.status, body(&r)),
        (
            502,
            json!({ "error": { "key": "download.http.errors.download_client", "detail": "download client has no host configured" } })
        )
    );
    h.finish().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_torrent_list_with_one_unreadable_entry_keeps_its_rows_listed_as_stalled() {
    let Some(h) = Harness::start(setup()).await else {
        return;
    };
    let bad = Stub::start(
        |_, target, _| match target.split('?').next().unwrap_or("") {
            "/api/v2/auth/login" => (
                200,
                vec![("set-cookie", "SID=s; path=/".into())],
                b"Ok.".to_vec(),
            ),
            "/api/v2/torrents/info" => (
                200,
                vec![],
                json!([{ "hash": HASH, "name": "ok", "state": "downloading" }, { "hash": 7 }])
                    .to_string()
                    .into_bytes(),
            ),
            _ => (404, vec![], vec![]),
        },
    );
    add_qbit(&h, &bad).await;
    let mut db = plugin_db(&h).await;
    let media = add_movie(&h, "Some Movie").await;
    exec(&mut db, &format!(r#"INSERT INTO "download_history" ("sourceTitle", "quality", "status", "torrentHash", "downloadClientId", "mediaId") SELECT 'x', 'HD', 'grabbed', 'abc', "id", {media} FROM "download_clients""#)).await;
    // A row the core already holds open must not be closed by a tick where its client is unreadable.
    let held = ref_of(&h, 1).await;
    sqlx::query(
        r#"INSERT INTO acquisition_queue ("pluginId", "ref", "mediaId", "sourceId", "sourceName", title, quality, method, state)
           VALUES ($1, $3, $2, '0', 'x', 't', 'HD', 'auto', 'active')"#,
    )
    .bind(&h.id)
    .bind(media as i32)
    .bind(&held)
    .execute(&h.pool)
    .await
    .unwrap();
    h.job("ImportCompleted", None).await.unwrap();
    assert_eq!(queue_rows(&h).await, [(held, "stalled".into(), 0.0)]);
    h.finish().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn retry_import_rearms_only_a_failed_import_and_only_once() {
    let Some(h) = Harness::start(setup()).await else {
        return;
    };
    let mut db = plugin_db(&h).await;
    let mut rx = h.st.hub.subscribe();
    let failed = async |db: &mut PgConnection| -> i64 {
        scalar_i64(
            db,
            r#"INSERT INTO "download_history" ("sourceTitle", "quality", "status", "statusMessage") VALUES ('x', 'HD', 'import_failed', 'EACCES') RETURNING "id"::bigint"#,
        )
        .await
    };
    let id = failed(&mut db).await;
    let r = act(&h, "retry-import", id).await;
    assert_eq!((r.status, body(&r)), (200, json!({})));
    assert_eq!(status_of(&mut db, id).await, ("grabbed".into(), None));
    until("the queue change", MAX, async || {
        std::iter::from_fn(|| rx.try_recv().ok()).any(|b| format!("{b:?}").contains("QueueUpdated"))
    })
    .await;
    let r = act(&h, "retry-import", id).await;
    assert_eq!(
        (r.status, body(&r)),
        (
            409,
            json!({ "error": { "key": "download.queue.errors.not_retryable", "detail": "grabbed" } })
        )
    );

    // Two retries racing for one row: exactly one wins.
    let id = failed(&mut db).await;
    let payload = json!({ "ref": ref_of(&h, id).await }).to_string();
    let (a, b) = tokio::join!(
        h.http(request(
            "POST",
            "/queue/retry-import",
            &[],
            payload.as_bytes()
        )),
        h.http(request(
            "POST",
            "/queue/retry-import",
            &[],
            payload.as_bytes()
        ))
    );
    let (a, b) = (a.unwrap(), b.unwrap());
    let seen = format!(
        "{:?} {:?}",
        String::from_utf8_lossy(&a.body),
        String::from_utf8_lossy(&b.body)
    );
    let mut statuses = [a.status, b.status];
    statuses.sort();
    assert_eq!(statuses, [200, 409], "{seen}");
    h.finish().await;
}

/// A row of the core's queue for this plugin's `ref`: (state, progress, source, method, error).
type Entry = (String, f64, String, String, Option<String>);

async fn entries(h: &Harness, row: i64) -> Vec<Entry> {
    sqlx::query_as(
        r#"SELECT state, progress, "sourceName", method, error FROM acquisition_queue
           WHERE "pluginId" = $1 AND "ref" = $2 ORDER BY id"#,
    )
    .bind(&h.id)
    .bind(ref_of(h, row).await)
    .fetch_all(&h.pool)
    .await
    .unwrap()
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn with_no_enabled_client_the_queue_is_still_stated_and_clamped() {
    let Some(h) = Harness::start(setup()).await else {
        return;
    };
    let media = add_movie(&h, "Some Movie").await;
    let mut db = plugin_db(&h).await;
    let title = "x".repeat(600);
    exec(&mut db, format!(r#"INSERT INTO "download_history" ("sourceTitle", "quality", "status", "torrentHash", "mediaId") VALUES ('{title}', ' ', 'grabbed', 'abc', {media})"#)).await;
    h.job("ImportCompleted", None).await.unwrap();
    let rows: Vec<(String, String, String)> = sqlx::query_as(
        r#"SELECT state, title, quality FROM acquisition_queue WHERE "pluginId" = $1 AND "finishedAt" IS NULL"#,
    )
    .bind(&h.id)
    .fetch_all(&h.pool)
    .await
    .unwrap();
    assert_eq!(
        rows,
        [("stalled".into(), "x".repeat(500), "Unknown".into())],
        "a clamped entry, listed although no client is enabled"
    );
    h.finish().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn the_poller_states_every_transfer_to_the_core() {
    let Some(h) = Harness::start(setup()).await else {
        return;
    };
    let qbit = fake_qbit();
    add_qbit(&h, &qbit).await;
    let media = add_movie(&h, "Some Movie").await;
    let mut db = plugin_db(&h).await;
    let tracker = add_indexer(&mut db, "Tracker", json!({})).await;
    let magnet = format!("magnet:?xt=urn:btih:{HASH}&dn=Some.Movie.2020.1080p");
    let r = h
        .proxy(
            "POST",
            &format!("/{media}/grab"),
            Some(&json!({ "downloadUrl": magnet, "force": true })),
        )
        .await;
    assert_eq!(r.status, 200, "{:?}", body(&r));
    let live = scalar_i64(&mut db, r#"SELECT "id"::bigint FROM "download_history""#).await;
    exec(
        &mut db,
        format!(r#"UPDATE "download_history" SET "indexerId" = {tracker} WHERE "id" = {live}"#),
    )
    .await;
    let mut seed = async |status: &str, message: &str| -> i64 {
        scalar_i64(
            &mut db,
            &format!(
                r#"INSERT INTO "download_history" ("sourceTitle", "quality", "status", "statusMessage", "mediaId", "grabSource")
                   VALUES ('x', 'HD', '{status}', NULLIF('{message}', ''), {media}, 'rss') RETURNING "id"::bigint"#
            ),
        )
        .await
    };
    let done = seed("completed", "").await;
    let failed = seed("failed", "boom").await;
    let withdrawn = seed("failed", "download.queue.removed_by_user").await;

    h.job("ImportCompleted", None).await.unwrap();
    let active: Entry = (
        "active".into(),
        0.5,
        "Tracker".into(),
        "manual".into(),
        None,
    );
    assert_eq!(entries(&h, live).await, [active]);
    let imported: Entry = (
        "imported".into(),
        1.0,
        "Download".into(),
        "rss".into(),
        None,
    );
    assert_eq!(entries(&h, done).await, [imported]);
    let boom = Some("boom".to_owned());
    let failed_entry: Entry = ("failed".into(), 1.0, "Download".into(), "rss".into(), boom);
    assert_eq!(entries(&h, failed).await, [failed_entry]);
    assert_eq!(entries(&h, withdrawn).await, []);

    // A finished row is reported once, however many ticks follow.
    h.job("ImportCompleted", None).await.unwrap();
    assert_eq!(entries(&h, done).await.len(), 1);
    assert_eq!(entries(&h, failed).await.len(), 1);

    assert_eq!(act(&h, "remove", live).await.status, 200);
    assert_eq!(entries(&h, live).await[0].0, "removed");
    h.job("ImportCompleted", None).await.unwrap();
    assert_eq!(entries(&h, live).await.len(), 1);
    assert_eq!(entries(&h, withdrawn).await, []);
    h.finish().await;
}

/// A qBittorrent whose one finished torrent `name` (holding `files`) is there from the start, or
/// from the moment it is added when `after_add`; `delete` empties it.
fn finished_qbit(save: &std::path::Path, name: &str, files: &[&str], after_add: bool) -> Stub {
    let (save, name) = (save.to_string_lossy().into_owned(), name.to_owned());
    let files: Vec<Value> = files
        .iter()
        .map(|f| json!({ "name": f, "progress": 1 }))
        .collect();
    let held = Mutex::new(!after_add);
    Stub::start(move |_, target, _| {
        let ok = |b: String| (200, vec![], b.into_bytes());
        match target.split('?').next().unwrap_or("") {
            "/api/v2/auth/login" => (
                200,
                vec![("set-cookie", "SID=s; path=/".into())],
                b"Ok.".to_vec(),
            ),
            "/api/v2/torrents/add" => {
                *held.lock().unwrap() = true;
                ok("Ok.".into())
            }
            "/api/v2/torrents/info" if !*held.lock().unwrap() => ok("[]".into()),
            "/api/v2/torrents/info" => ok(json!([{
                "hash": HASH, "name": name, "size": 1000, "downloaded": 1000,
                "progress": 1, "dlspeed": 0, "ratio": 0, "eta": 0, "state": "stalledUP",
                "save_path": save, "added_on": 1
            }])
            .to_string()),
            "/api/v2/torrents/files" => ok(json!(files).to_string()),
            "/api/v2/torrents/delete" => {
                *held.lock().unwrap() = false;
                ok(String::new())
            }
            _ => (404, vec![], vec![]),
        }
    })
}

/// A harness whose ingest root is a scratch folder, with a finished torrent in it and a history
/// row for it; `library` anchors the movie (none: core refuses the ingest).
async fn import_fixture(
    name: &str,
    library: bool,
) -> Option<(Harness, std::path::PathBuf, i64, Stub)> {
    let base = std::path::PathBuf::from("/tmp/fliks-parity/download-port")
        .join(format!("{name}-{}", std::process::id()));
    let (dl, lib) = (base.join("dl"), base.join("lib"));
    std::fs::create_dir_all(&dl).unwrap();
    std::fs::create_dir_all(&lib).unwrap();
    std::fs::write(dl.join("Some.Movie.2020.1080p.mkv"), b"not really a video").unwrap();
    let mut s = setup();
    s.template["ingestRoots"] = json!([dl.to_string_lossy()]);
    let h = Harness::start(s).await?;
    let qbit = finished_qbit(
        &dl,
        "Some.Movie.2020.1080p",
        &["Some.Movie.2020.1080p.mkv"],
        false,
    );
    let client = add_qbit(&h, &qbit).await;
    let media = add_movie_in(&h, "Some Movie", library.then_some(lib.as_path())).await;
    let mut db = plugin_db(&h).await;
    let id = scalar_i64(
        &mut db,
        &format!(
            r#"INSERT INTO "download_history" ("sourceTitle", "quality", "status", "torrentHash", "downloadClientId", "mediaId", "grabSource")
               VALUES ('Some.Movie.2020.1080p', 'HD', 'grabbed', '{HASH}', {client}, {media}, 'manual') RETURNING "id"::bigint"#
        ),
    )
    .await;
    Some((h, base, id, qbit))
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn an_import_core_refuses_is_failed_and_announced() {
    let Some((h, base, id, _qbit)) = import_fixture("refused", false).await else {
        return;
    };
    let mut rx = h.st.hub.subscribe();
    h.job("ImportCompleted", None).await.unwrap();
    let mut db = plugin_db(&h).await;
    let (status, message) = status_of(&mut db, id).await;
    assert_eq!(status, "import_failed");
    assert!(
        message
            .as_deref()
            .is_some_and(|m| m.contains("not anchored to a library folder")),
        "{message:?}"
    );
    assert!(
        std::iter::from_fn(|| rx.try_recv().ok())
            .any(|b| format!("{b:?}").contains("ImportFailed")),
        "no acquisition.failed reached the hub"
    );
    h.finish().await;
    let _ = std::fs::remove_dir_all(base);
}

/// The second query of a split season runs out of time: the first one's releases are kept.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_later_query_that_lapses_keeps_the_earlier_results() {
    let Some(h) = Harness::start(setup()).await else {
        return;
    };
    h.set_config("search_budget_seconds", Some("5")).await;
    let tz = Stub::start(|_, target, _| {
        if target.contains("Show%20S02") {
            std::thread::sleep(Duration::from_secs(10));
        }
        (200, vec![], one_item_feed("Some.Show.S01.A"))
    });
    route(&h, "torznab.test", "10.77.0.1", 80, &tz);
    let mut db = plugin_db(&h).await;
    add_indexer(
        &mut db,
        "tz",
        json!({ "baseUrl": "http://torznab.test/api", "apiKey": "k" }),
    )
    .await;
    let (media, season) = split_season(&h).await;
    let r = h
        .proxy("GET", &format!("/{media}/groups/{season}/releases"), None)
        .await;
    assert_eq!(r.status, 200, "{:?}", body(&r));
    assert_eq!(body(&r).as_array().map(Vec::len), Some(1), "{:?}", body(&r));
    h.finish().await;
}

/// A series whose scene mapping TheXEM lists but the host does not hold: its imports are refused
/// for now, retried, and failed once the first refusal is a day old.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn an_import_the_host_refuses_for_now_is_retried_then_failed() {
    let base = std::path::PathBuf::from("/tmp/fliks-parity/download-port")
        .join(format!("scene-retry-{}", std::process::id()));
    let (dl, lib) = (base.join("dl"), base.join("lib"));
    std::fs::create_dir_all(&dl).unwrap();
    std::fs::create_dir_all(&lib).unwrap();
    std::fs::write(dl.join("Some.Show.S04E01.1080p.mkv"), b"not really a video").unwrap();
    let mut s = setup();
    s.template["ingestRoots"] = json!([dl.to_string_lossy()]);
    let Some(h) = Harness::start(s).await else {
        return;
    };
    // A scene provider that lists the series but cannot answer: its mapping stays unknown.
    let down = Arc::new(
        fliks_plugin_sdk::harness::fliks_server::metadata::scene_stub::SceneStub::default(),
    );
    down.down.store(true, std::sync::atomic::Ordering::SeqCst);
    h.st.metadata.add_plugin(down);
    let qbit = finished_qbit(
        &dl,
        "Some.Show.S04E01.1080p",
        &["Some.Show.S04E01.1080p.mkv"],
        false,
    );
    let client = add_qbit(&h, &qbit).await;
    let media = add_work(&h, "Some Show", "series", Some(lib.as_path())).await;
    let season: i32 = sqlx::query_scalar(
        r#"INSERT INTO item_groups ("mediaId", "number", monitored) VALUES ($1, 3, true) RETURNING id"#,
    )
    .bind(media as i32)
    .fetch_one(&h.pool)
    .await
    .unwrap();
    sqlx::raw_sql(AssertSqlSafe(format!(
        r#"INSERT INTO items ("mediaId", "editionId", "groupId", "number", monitored)
             SELECT {media}, (SELECT id FROM editions WHERE "mediaId" = {media}), {season}, n, true
             FROM generate_series(1, 8) n;
           INSERT INTO media_external_ids ("mediaId", kind, provider, value) VALUES ({media}, 'series', 'tvdb', '9');
           INSERT INTO scene_mappings ("tvdbId") VALUES (9)"#
    )))
    .execute(&h.pool)
    .await
    .unwrap();
    let mut db = plugin_db(&h).await;
    let id = scalar_i64(
        &mut db,
        &format!(
            r#"INSERT INTO "download_history" ("sourceTitle", "quality", "status", "torrentHash", "downloadClientId", "mediaId", "grabSource")
               VALUES ('Some.Show.S04E01.1080p', 'HD', 'grabbed', '{HASH}', {client}, {media}, 'manual') RETURNING "id"::bigint"#
        ),
    )
    .await;
    h.job("ImportCompleted", None).await.unwrap();
    let (status, message) = status_of(&mut db, id).await;
    assert_eq!(status, "grabbed", "{message:?}");
    assert!(
        message
            .as_deref()
            .is_some_and(|m| m.contains("scene mapping")),
        "{message:?}"
    );
    exec(
        &mut db,
        format!(r#"UPDATE "download_history" SET "retryLaterSince" = now() - interval '25 hours' WHERE "id" = {id}"#),
    )
    .await;
    h.job("ImportCompleted", None).await.unwrap();
    let (status, message) = status_of(&mut db, id).await;
    assert_eq!(status, "import_failed", "{message:?}");
    assert!(
        message.as_deref().is_some_and(|m| m.contains("24 hours")),
        "{message:?}"
    );
    h.finish().await;
    let _ = std::fs::remove_dir_all(base);
}

// --- books ---------------------------------------------------------------------------------

const BOOK_RELEASE: &str = "Mirelune.2020.EPUB-GRP";

/// A monitored book with a want per family, anchored to `library`; the host makes the edition on
/// the first import.
async fn add_book(h: &Harness, title: &str, library: &std::path::Path, families: &[&str]) -> i64 {
    let lib: i32 = sqlx::query_scalar(
        r#"INSERT INTO libraries (name, "mediaTypes") VALUES ($1, '["book"]') RETURNING id"#,
    )
    .bind(title)
    .fetch_one(&h.pool)
    .await
    .unwrap();
    sqlx::query(r#"INSERT INTO library_folders ("libraryId", path) VALUES ($1, $2)"#)
        .bind(lib)
        .bind(library.to_string_lossy().into_owned())
        .execute(&h.pool)
        .await
        .unwrap();
    let media: i32 = sqlx::query_scalar(
        r#"INSERT INTO media (title, type, year, monitored, "libraryId", "folderName")
           VALUES ($1, 'book', 2020, true, $2, $1) RETURNING id"#,
    )
    .bind(title)
    .bind(lib)
    .fetch_one(&h.pool)
    .await
    .unwrap();
    for family in families {
        sqlx::query(
            r#"INSERT INTO wants ("mediaId", family, "folderId")
               VALUES ($1, $2, (SELECT df."folderId" FROM library_default_folders df WHERE df."libraryId" = $3))"#,
        )
        .bind(media)
        .bind(family)
        .bind(lib)
        .execute(&h.pool)
        .await
        .unwrap();
    }
    media.into()
}

/// A tracker whose every answer, feed or search, is one epub release.
fn book_tracker() -> Stub {
    let feed = format!(
        r#"<?xml version="1.0"?><rss xmlns:torznab="http://torznab.com/schemas/2015/feed"><channel><item><title>{BOOK_RELEASE}</title><link>magnet:?xt=urn:btih:{HASH}&amp;dn={BOOK_RELEASE}</link><size>2000000</size><torznab:attr name="seeders" value="5"/></item></channel></rss>"#
    )
    .into_bytes();
    Stub::start(move |_, _, _| (200, vec![], feed.clone()))
}

/// The tracker as an enabled indexer advertising both book categories.
async fn add_book_indexer(h: &Harness, tracker: &Stub) {
    route(h, "books.test", "10.77.0.1", 80, tracker);
    let mut db = plugin_db(h).await;
    let id = add_indexer(
        &mut db,
        "books",
        json!({ "baseUrl": "http://books.test/api", "apiKey": "k" }),
    )
    .await;
    exec(
        &mut db,
        format!(r#"UPDATE "indexers" SET "capsCategories" = '3030,7000,7020' WHERE "id" = {id}"#),
    )
    .await;
}

/// `held`: the torrent is in the client from the start, else only once added.
async fn book_fixture(
    name: &str,
    families: &[&str],
    held: bool,
    files: &[&str],
) -> Option<(Harness, std::path::PathBuf, i64, Stub)> {
    let qbit = |dl: &std::path::Path| finished_qbit(dl, BOOK_RELEASE, files, !held);
    book_fixture_with(name, families, files, qbit).await
}

/// `qbit` builds the client stub over the download folder, which holds `files`.
async fn book_fixture_with(
    name: &str,
    families: &[&str],
    files: &[&str],
    qbit: impl FnOnce(&std::path::Path) -> Stub,
) -> Option<(Harness, std::path::PathBuf, i64, Stub)> {
    let base = std::path::PathBuf::from("/tmp/fliks-parity/download-port")
        .join(format!("{name}-{}", std::process::id()));
    let (dl, lib) = (base.join("dl"), base.join("lib"));
    std::fs::create_dir_all(&dl).unwrap();
    std::fs::create_dir_all(&lib).unwrap();
    for f in files {
        std::fs::write(dl.join(f), b"not really a book").unwrap();
    }
    let mut s = setup();
    s.template["ingestRoots"] = json!([dl.to_string_lossy()]);
    let h = Harness::start(s).await?;
    let qbit = qbit(&dl);
    let client = add_qbit(&h, &qbit).await;
    let mut db = plugin_db(&h).await;
    exec(
        &mut db,
        format!(
            r#"UPDATE "download_clients" SET "settings" = "settings" || '{{"ebookCategory": "ebooks"}}'::jsonb WHERE "id" = {client}"#
        ),
    )
    .await;
    let media = add_book(&h, "Mirelune", &lib, families).await;
    Some((h, base, media, qbit))
}

/// What the core's sweep does for `media`: for each of its wanted candidates, the System search of
/// this provider, then a grab of the first release it offers.
async fn core_sweep(h: &Harness, media: i64) {
    use fliks_plugin_sdk::harness::fliks_server::plugins::Principal;
    use fliks_plugin_sdk::harness::fliks_server::plugins::acquisition::candidates;
    use fliks_plugin_sdk::harness::fliks_server::plugins::runtime::fliks::plugin::types::{
        CandidatesQuery, Family, WantDecision,
    };
    let family_str = |f: Family| match f {
        Family::Video => "video",
        Family::Audio => "audio",
        Family::Text => "text",
        Family::Image => "image",
    };
    let today = chrono::Local::now().date_naive();
    let q = CandidatesQuery {
        kind: None,
        family: None,
        media_ids: Some(vec![media as u32]),
        available_on: today.to_string(),
        limit: 200,
        cursor: None,
    };
    let page = candidates(&h.st.db, q, today).await.unwrap();
    for t in page.items {
        if t.want
            .as_ref()
            .is_none_or(|w| w.decision == WantDecision::Skip)
        {
            continue;
        }
        let unit = t.scope.first();
        let at = match (unit.and_then(|u| u.group_id), unit.and_then(|u| u.item_id)) {
            (_, Some(i)) => format!("/{}/items/{i}", t.media_id),
            (Some(g), None) => format!("/{}/groups/{g}", t.media_id),
            _ => format!("/{}", t.media_id),
        };
        let mut query = vec![("family", family_str(t.family))];
        if let Some(l) = t.language.as_deref() {
            query.push(("language", l));
        }
        let mut search = request("GET", &format!("{at}/releases"), &query, b"");
        search.principal = Principal::System;
        let res = h.http(search).await.unwrap();
        assert_eq!(res.status, 200, "{}", String::from_utf8_lossy(&res.body));
        let found: Vec<Value> = serde_json::from_slice(&res.body).unwrap();
        let Some(best) = found.first() else { continue };
        let body = json!({
            "downloadUrl": best["downloadUrl"], "sourceTitle": best["title"],
            "sourceId": best["sourceId"], "infoUrl": best["infoUrl"],
            "family": family_str(t.family), "language": t.language,
        });
        let mut grab = request(
            "POST",
            &format!("{at}/grab"),
            &[],
            body.to_string().as_bytes(),
        );
        grab.principal = Principal::System;
        let res = h.http(grab).await.unwrap();
        assert_eq!(res.status, 200, "{}", String::from_utf8_lossy(&res.body));
    }
}

fn hub_saw(events: &mut tokio::sync::broadcast::Receiver<Arc<Bus>>, what: &[&str]) -> bool {
    std::iter::from_fn(|| events.try_recv().ok()).any(|b| {
        let d = format!("{b:?}");
        what.iter().all(|w| d.contains(w))
    })
}

/// A wanted book is found by the sweep per family, grabbed under its ebook category, imported
/// from its book files and announced with its family and no unit.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_book_is_acquired_from_want_to_import() {
    let Some((h, base, media, qbit)) =
        book_fixture("book", &["text", "audio"], false, &["Mirelune.epub"]).await
    else {
        return;
    };
    let tracker = book_tracker();
    add_book_indexer(&h, &tracker).await;
    let mut events = h.st.hub.subscribe();

    core_sweep(&h, media).await;
    let searched: Vec<String> = tracker.hits().into_iter().map(|x| x.target).collect();
    for cat in ["cat=7020", "cat=3030"] {
        assert!(
            searched.iter().any(|t| t.contains(cat)),
            "a search per family: {searched:?}"
        );
    }
    let add = qbit.hits_to("/api/v2/torrents/add");
    assert_eq!(add.len(), 1, "only the text target takes the epub");
    assert!(add[0].body.contains("category=ebooks"), "{}", add[0].body);
    assert!(hub_saw(&mut events, &["AcquisitionGrabbed", "Text"]));

    let mut db = plugin_db(&h).await;
    let (family, language, source): (Option<String>, Option<String>, String) = sqlx::query_as(
        r#"SELECT "family", "language", "grabSource" FROM "download_history" WHERE "torrentHash" = $1 AND "mediaId" = $2"#,
    )
    .bind(HASH)
    .bind(media as i32)
    .fetch_one(&mut db)
    .await
    .unwrap();
    assert_eq!((family.as_deref(), language), (Some("text"), None));
    assert_eq!(source, "auto", "the core's grab is an automatic one");

    h.job("ImportCompleted", None).await.unwrap();
    let done: i64 = scalar_i64(
        &mut db,
        format!(r#"SELECT count(*) FROM "download_history" WHERE "mediaId" = {media} AND "status" = 'completed'"#),
    )
    .await;
    assert_eq!(done, 1);
    assert!(hub_saw(&mut events, &["ImportComplete"]));
    let editions: Vec<String> =
        sqlx::query_scalar(r#"SELECT family FROM editions WHERE "mediaId" = $1 ORDER BY family"#)
            .bind(media as i32)
            .fetch_all(&h.pool)
            .await
            .unwrap();
    assert_eq!(editions, ["text"]);

    // The core's queue saw the import start, then holds it as imported, once.
    assert_eq!(queue_rows(&h).await[0].1, "importing");
    for _ in 0..2 {
        h.job("ImportCompleted", None).await.unwrap();
        let rows = queue_rows(&h).await;
        assert_eq!((rows.len(), rows[0].1.as_str()), (1, "imported"));
    }
    h.finish().await;
    let _ = std::fs::remove_dir_all(base);
}

/// The feed names no family: the book's RSS reads its categories from the caps and tries each
/// target the host lists.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn the_rss_sweep_grabs_a_book_through_its_targets() {
    let Some((h, base, media, qbit)) =
        book_fixture("book-rss", &["text", "audio"], false, &["Mirelune.epub"]).await
    else {
        return;
    };
    let tracker = book_tracker();
    add_book_indexer(&h, &tracker).await;

    h.job("RssSync", None).await.unwrap();
    let hits = tracker.hits();
    assert!(
        hits[0].target.contains("t=search&q=&cat=2000,7020,3030"),
        "{:?}",
        hits[0].target
    );
    assert_eq!(qbit.hits_to("/api/v2/torrents/add").len(), 1);
    let mut db = plugin_db(&h).await;
    let family: Option<String> =
        sqlx::query_scalar(r#"SELECT "family" FROM "download_history" WHERE "mediaId" = $1"#)
            .bind(media as i32)
            .fetch_one(&mut db)
            .await
            .unwrap();
    assert_eq!(family.as_deref(), Some("text"));
    h.finish().await;
    let _ = std::fs::remove_dir_all(base);
}

/// Another provider's download of the media stops the RSS auto-grab; once it is gone the sync grabs.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_download_in_flight_at_another_provider_blocks_the_auto_grab() {
    let Some((h, base, media, qbit)) =
        book_fixture("book-in-flight", &["text"], false, &["Mirelune.epub"]).await
    else {
        return;
    };
    let feed = format!(
        r#"<?xml version="1.0"?><rss xmlns:torznab="http://torznab.com/schemas/2015/feed"><channel>
<item><title>{BOOK_RELEASE}</title><link>magnet:?xt=urn:btih:{HASH}&amp;dn={BOOK_RELEASE}</link><size>2000000</size><torznab:attr name="seeders" value="5"/></item>
</channel></rss>"#
    )
    .into_bytes();
    let tracker = Stub::start(move |_, _, _| (200, vec![], feed.clone()));
    add_book_indexer(&h, &tracker).await;
    sqlx::query(
        r#"INSERT INTO acquisition_queue ("pluginId", "ref", "mediaId", "sourceId", "sourceName", title, quality, method, state, progress)
           VALUES ('other.provider', 'x', $1, '0', 'other', 't', 'HD', 'auto', 'active', 0.1)"#,
    )
    .bind(media as i32)
    .execute(&h.pool)
    .await
    .unwrap();

    h.job("RssSync", None).await.unwrap();
    assert!(logged(&h, "a grab is in flight at other.provider"));
    assert!(qbit.hits_to("/api/v2/torrents/add").is_empty());

    sqlx::query(r#"UPDATE acquisition_queue SET state = 'removed', "finishedAt" = now() WHERE "pluginId" = 'other.provider'"#)
        .execute(&h.pool)
        .await
        .unwrap();
    h.job("RssSync", None).await.unwrap();
    assert_eq!(qbit.hits_to("/api/v2/torrents/add").len(), 1);
    h.finish().await;
    let _ = std::fs::remove_dir_all(base);
}

/// A tracker that refuses `t=book` is asked with `t=search` from then on.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_refused_book_search_is_remembered() {
    let Some((h, base, media, _qbit)) =
        book_fixture("book-refused", &["text"], false, &["Mirelune.epub"]).await
    else {
        return;
    };
    let tracker = Stub::start(|_, target, _| {
        let body = if target.contains("t=book") {
            r#"<error code="203" description="Function Not Available"/>"#
        } else {
            r#"<?xml version="1.0"?><rss><channel></channel></rss>"#
        };
        (200, vec![], body.into())
    });
    add_book_indexer(&h, &tracker).await;
    let mut db = plugin_db(&h).await;
    exec(
        &mut db,
        r#"UPDATE "indexers" SET "capsBookSearch" = true WHERE "name" = 'books'"#,
    )
    .await;

    core_sweep(&h, media).await;
    core_sweep(&h, media).await;
    let book = tracker
        .hits()
        .into_iter()
        .filter(|x| x.target.contains("t=book"))
        .count();
    assert_eq!(book, 1, "{:?}", tracker.hits());
    let caps: bool =
        sqlx::query_scalar(r#"SELECT "capsBookSearch" FROM "indexers" WHERE "name" = 'books'"#)
            .fetch_one(&mut db)
            .await
            .unwrap();
    assert!(!caps);
    h.finish().await;
    let _ = std::fs::remove_dir_all(base);
}

/// A tracker that fails `t=book` without refusing it keeps its book search and gets no fallback.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_failed_book_search_is_not_a_refusal() {
    let Some((h, base, media, _qbit)) =
        book_fixture("book-failed", &["text"], false, &["Mirelune.epub"]).await
    else {
        return;
    };
    let tracker = Stub::start(|_, _, _| (500, vec![], vec![]));
    add_book_indexer(&h, &tracker).await;
    let mut db = plugin_db(&h).await;
    exec(
        &mut db,
        r#"UPDATE "indexers" SET "capsBookSearch" = true WHERE "name" = 'books'"#,
    )
    .await;

    core_sweep(&h, media).await;
    let targets: Vec<String> = tracker.hits().into_iter().map(|x| x.target).collect();
    assert!(targets.iter().any(|t| t.contains("t=book")), "{targets:?}");
    assert!(
        !targets.iter().any(|t| t.contains("t=search")),
        "{targets:?}"
    );
    let caps: bool =
        sqlx::query_scalar(r#"SELECT "capsBookSearch" FROM "indexers" WHERE "name" = 'books'"#)
            .fetch_one(&mut db)
            .await
            .unwrap();
    assert!(caps);
    h.finish().await;
    let _ = std::fs::remove_dir_all(base);
}

/// A row grabbed for a language the book has no edition in imports into a new edition of it,
/// next to the edition of another language.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_book_row_imports_into_a_new_edition_of_its_language() {
    let Some((h, base, media, _qbit)) =
        book_fixture("book-lang", &["text"], true, &["Mirelune.epub"]).await
    else {
        return;
    };
    sqlx::query(r#"UPDATE wants SET language = 'en' WHERE "mediaId" = $1"#)
        .bind(media as i32)
        .execute(&h.pool)
        .await
        .unwrap();
    sqlx::query(r#"INSERT INTO editions ("mediaId", family, language) VALUES ($1, 'text', 'fr')"#)
        .bind(media as i32)
        .execute(&h.pool)
        .await
        .unwrap();
    let mut db = plugin_db(&h).await;
    let client = scalar_i64(
        &mut db,
        r#"SELECT min("id")::bigint FROM "download_clients""#,
    )
    .await;
    let id = scalar_i64(
        &mut db,
        format!(
            r#"INSERT INTO "download_history" ("sourceTitle", "quality", "status", "torrentHash", "downloadClientId", "mediaId", "grabSource", "family", "language")
               VALUES ('{BOOK_RELEASE}', 'EPUB', 'grabbed', '{HASH}', {client}, {media}, 'auto', 'text', 'en') RETURNING "id"::bigint"#
        ),
    )
    .await;
    h.job("ImportCompleted", None).await.unwrap();
    assert_eq!(status_of(&mut db, id).await.0, "completed");
    assert_eq!(edition_languages(&h, media).await, ["en", "fr"]);
    h.finish().await;
    let _ = std::fs::remove_dir_all(base);
}

const HASH_FR: &str = "89abcdef0123456789abcdef0123456789abcdef";

/// A client holding the German epub of `HASH` and the French epub of `HASH_FR` once each is added.
fn twin_qbit(save: &std::path::Path) -> Stub {
    let save = save.to_string_lossy().into_owned();
    let added: Mutex<Vec<&'static str>> = Mutex::default();
    Stub::start(move |_, target, body| {
        let ok = |b: String| (200, vec![], b.into_bytes());
        match target.split('?').next().unwrap_or("") {
            "/api/v2/auth/login" => (
                200,
                vec![("set-cookie", "SID=s; path=/".into())],
                b"Ok.".to_vec(),
            ),
            "/api/v2/torrents/add" => {
                for h in [HASH, HASH_FR] {
                    if body.contains(h) {
                        added.lock().unwrap().push(h);
                    }
                }
                ok("Ok.".into())
            }
            "/api/v2/torrents/info" => ok(json!(
                added
                    .lock()
                    .unwrap()
                    .iter()
                    .map(|h| {
                        let name = if *h == HASH {
                            BOOK_RELEASE_DE
                        } else {
                            BOOK_RELEASE_FR
                        };
                        json!({
                            "hash": h, "name": name, "size": 1000, "downloaded": 1000,
                            "progress": 1, "dlspeed": 0, "ratio": 0, "eta": 0,
                            "state": "stalledUP", "save_path": save, "added_on": 1
                        })
                    })
                    .collect::<Vec<_>>()
            )
            .to_string()),
            "/api/v2/torrents/files" => {
                let file = if target.contains(HASH_FR) {
                    "Mirelune.FR.epub"
                } else {
                    "Mirelune.DE.epub"
                };
                ok(json!([{ "name": file, "progress": 1 }]).to_string())
            }
            _ => (404, vec![], vec![]),
        }
    })
}

const BOOK_RELEASE_FR: &str = "Mirelune.2020.FR.EPUB-GRP";
const BOOK_RELEASE_DE: &str = "Mirelune.2020.GERMAN.EPUB-GRP";

async fn edition_languages(h: &Harness, media: i64) -> Vec<String> {
    sqlx::query_scalar(
        r#"SELECT language FROM editions WHERE "mediaId" = $1 AND language IS NOT NULL ORDER BY language"#,
    )
    .bind(media as i32)
    .fetch_all(&h.pool)
    .await
    .unwrap()
}

/// A book wished in text/fr and text/de is two targets (a language-less want is only the fallback
/// of a family with no language want): the sweep grabs one release for each, a second sweep grabs
/// nothing, and each row is ingested with the language it was grabbed for.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_book_wished_in_two_languages_is_grabbed_and_ingested_per_language() {
    let files = ["Mirelune.DE.epub", "Mirelune.FR.epub"];
    let Some((h, base, media, qbit)) =
        book_fixture_with("book-two-lang", &["text"], &files, twin_qbit).await
    else {
        return;
    };
    sqlx::query(r#"UPDATE wants SET language = 'fr' WHERE "mediaId" = $1"#)
        .bind(media as i32)
        .execute(&h.pool)
        .await
        .unwrap();
    sqlx::query(
        r#"INSERT INTO wants ("mediaId", family, language, "folderId")
           SELECT "mediaId", family, 'de', "folderId" FROM wants WHERE "mediaId" = $1"#,
    )
    .bind(media as i32)
    .execute(&h.pool)
    .await
    .unwrap();
    let feed = format!(
        r#"<?xml version="1.0"?><rss xmlns:torznab="http://torznab.com/schemas/2015/feed"><channel>
<item><title>{BOOK_RELEASE_DE}</title><link>magnet:?xt=urn:btih:{HASH}&amp;dn={BOOK_RELEASE_DE}</link><size>2000000</size><torznab:attr name="seeders" value="5"/></item>
<item><title>{BOOK_RELEASE_FR}</title><link>magnet:?xt=urn:btih:{HASH_FR}&amp;dn={BOOK_RELEASE_FR}</link><size>2000000</size><torznab:attr name="seeders" value="5"/></item>
</channel></rss>"#
    )
    .into_bytes();
    let tracker = Stub::start(move |_, _, _| (200, vec![], feed.clone()));
    add_book_indexer(&h, &tracker).await;

    core_sweep(&h, media).await;
    assert_eq!(
        qbit.hits_to("/api/v2/torrents/add").len(),
        2,
        "one grab per language"
    );
    core_sweep(&h, media).await;
    assert_eq!(
        qbit.hits_to("/api/v2/torrents/add").len(),
        2,
        "each language's grab is pending on its own"
    );
    let mut db = plugin_db(&h).await;
    let rows: Vec<(String, Option<String>)> = sqlx::query_as(
        r#"SELECT "torrentHash", "language" FROM "download_history" WHERE "mediaId" = $1 ORDER BY "language""#,
    )
    .bind(media as i32)
    .fetch_all(&mut db)
    .await
    .unwrap();
    assert_eq!(
        rows,
        [
            (HASH.to_owned(), Some("de".to_owned())),
            (HASH_FR.to_owned(), Some("fr".to_owned()))
        ]
    );

    h.job("ImportCompleted", None).await.unwrap();
    let done = scalar_i64(
        &mut db,
        format!(r#"SELECT count(*) FROM "download_history" WHERE "mediaId" = {media} AND "status" = 'completed'"#),
    )
    .await;
    assert_eq!(done, 2);
    assert_eq!(edition_languages(&h, media).await, ["de", "fr"]);
    assert_eq!(editions_of(&h, media).await, ["text", "text"]);
    h.finish().await;
    let _ = std::fs::remove_dir_all(base);
}

/// A finished torrent holding `files` for a book wished in `families`, claimed by one history row
/// of `family` (none: auto-matched); the import has run once.
async fn imported_book(
    name: &str,
    families: &[&str],
    files: &[&str],
    family: Option<&str>,
) -> Option<(Harness, std::path::PathBuf, i64, i64, Stub)> {
    let (h, base, media, qbit) = book_fixture(name, families, true, files).await?;
    let mut db = plugin_db(&h).await;
    let client = scalar_i64(
        &mut db,
        r#"SELECT min("id")::bigint FROM "download_clients""#,
    )
    .await;
    let (cols, vals) = family.map_or((String::new(), String::new()), |f| {
        (r#", "family""#.into(), format!(", '{f}'"))
    });
    // The release title names the formats its files carry, as an indexer's does.
    let title = ["epub", "m4b"]
        .iter()
        .filter(|e| files.iter().any(|f| f.ends_with(*e)))
        .fold("Mirelune.2020".to_owned(), |t, e| {
            format!("{t}.{}", e.to_uppercase())
        })
        + "-GRP";
    let id = scalar_i64(
        &mut db,
        format!(
            r#"INSERT INTO "download_history" ("sourceTitle", "quality", "status", "torrentHash", "downloadClientId", "mediaId", "grabSource"{cols})
               VALUES ('{title}', 'EPUB', 'grabbed', '{HASH}', {client}, {media}, 'auto'{vals}) RETURNING "id"::bigint"#
        ),
    )
    .await;
    h.job("ImportCompleted", None).await.unwrap();
    Some((h, base, media, id, qbit))
}

async fn editions_of(h: &Harness, media: i64) -> Vec<String> {
    sqlx::query_scalar(r#"SELECT family FROM editions WHERE "mediaId" = $1 ORDER BY family"#)
        .bind(media as i32)
        .fetch_all(&h.pool)
        .await
        .unwrap()
}

/// A torrent of another family than the book wishes is never deleted nor blocklisted, whether
/// its row names a family or was auto-matched.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn an_audiobook_torrent_on_a_text_only_book_is_kept() {
    for (name, family) in [("book-kept-text", Some("text")), ("book-kept-none", None)] {
        let Some((h, base, media, id, qbit)) =
            imported_book(name, &["text"], &["Mirelune.m4b"], family).await
        else {
            return;
        };
        let mut db = plugin_db(&h).await;
        let (status, message) = status_of(&mut db, id).await;
        assert_eq!(status, "import_failed", "{name}: {message:?}");
        assert_eq!(qbit.hits_to("/api/v2/torrents/delete").len(), 0, "{name}");
        let blocked = scalar_i64(&mut db, r#"SELECT count(*) FROM "blocklist""#).await;
        assert_eq!(blocked, 0, "{name}");
        assert!(editions_of(&h, media).await.is_empty(), "{name}");
        h.finish().await;
        let _ = std::fs::remove_dir_all(base);
    }
}

/// One torrent with the book's ebook and audiobook ingests each wished family, the row learning
/// the first. The harness has no streaming engine, which an audio ingest needs: the ebook is
/// imported and the audiobook's failed ingest does not fail the row.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_torrent_with_both_families_ingests_each_wished_family() {
    let Some((h, base, media, id, qbit)) = imported_book(
        "book-both",
        &["text", "audio"],
        &["Mirelune.epub", "Mirelune.m4b"],
        None,
    )
    .await
    else {
        return;
    };
    let mut db = plugin_db(&h).await;
    let (status, message) = status_of(&mut db, id).await;
    assert_eq!(status, "completed", "{message:?}");
    assert_eq!(editions_of(&h, media).await, ["text"]);
    let family: Option<String> =
        sqlx::query_scalar(r#"SELECT "family" FROM "download_history" WHERE "id" = $1::int"#)
            .bind(id as i32)
            .fetch_one(&mut db)
            .await
            .unwrap();
    assert_eq!(family.as_deref(), Some("text"));
    assert_eq!(qbit.hits_to("/api/v2/torrents/delete").len(), 0);
    h.finish().await;
    let _ = std::fs::remove_dir_all(base);
}

/// A file of a family the release does not name (a companion pdf in an audiobook torrent) is not
/// imported as that family.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_companion_file_of_an_unnamed_family_is_not_imported() {
    let Some((h, base, media, _, qbit)) = imported_book(
        "book-companion",
        &["text", "audio"],
        &["Mirelune.m4b", "Mirelune - Companion.pdf"],
        Some("audio"),
    )
    .await
    else {
        return;
    };
    assert!(editions_of(&h, media).await.is_empty());
    assert_eq!(qbit.hits_to("/api/v2/torrents/delete").len(), 0);
    h.finish().await;
    let _ = std::fs::remove_dir_all(base);
}

/// A family the book does not wish is left alone: the wished one imports, nothing is deleted.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn an_unwished_family_in_a_torrent_is_not_imported_nor_deleted() {
    let Some((h, base, media, id, qbit)) = imported_book(
        "book-unwished",
        &["text"],
        &["Mirelune.epub", "Mirelune.m4b"],
        Some("text"),
    )
    .await
    else {
        return;
    };
    let mut db = plugin_db(&h).await;
    assert_eq!(status_of(&mut db, id).await.0, "completed");
    assert_eq!(editions_of(&h, media).await, ["text"]);
    assert_eq!(qbit.hits_to("/api/v2/torrents/delete").len(), 0);
    h.finish().await;
    let _ = std::fs::remove_dir_all(base);
}

/// A torrent with no media file of any family is still a dud: failed, blocklisted, removed.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_torrent_without_any_media_file_is_removed_and_blocklisted() {
    let Some((h, base, _media, id, qbit)) =
        imported_book("book-dud", &["text"], &["readme.nfo"], Some("text")).await
    else {
        return;
    };
    let mut db = plugin_db(&h).await;
    assert_eq!(status_of(&mut db, id).await.0, "failed");
    assert_eq!(qbit.hits_to("/api/v2/torrents/delete").len(), 1);
    let blocked = scalar_i64(&mut db, r#"SELECT count(*) FROM "blocklist""#).await;
    assert_eq!(blocked, 1);
    h.finish().await;
    let _ = std::fs::remove_dir_all(base);
}
