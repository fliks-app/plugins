//! `fliks.musiclists`: follows the albums and tracks of ListenBrainz, Last.fm, Spotify, Deezer and
//! MusicBrainz lists. Titles are resolved by the host, never by the plugin (`music.md` rule 58).

mod db;
mod model;
mod net;
mod routes;
mod run;
mod sources;
mod spotify;
mod sync;

use fliks_plugin_sdk::{HealthReport, HttpRequest, HttpResponse, Plugin, info, warn};

const MIGRATIONS: [(&str, &str); 1] =
    [("0001_lists", include_str!("../migrations/0001_lists.sql"))];

struct Lists;

impl Plugin for Lists {
    fn start() -> Result<(), String> {
        db::migrate(&MIGRATIONS)
    }

    fn health() -> HealthReport {
        match db::list() {
            Ok(l) => HealthReport {
                ok: true,
                detail: Some(format!("lists={}", l.len())),
            },
            Err(e) => HealthReport {
                ok: false,
                detail: Some(e),
            },
        }
    }

    fn job(name: String, _job_id: String, _args: Option<String>) -> Result<(), String> {
        if name != "SyncLists" {
            return Err(format!("unknown job {name}"));
        }
        for row in db::list()?.iter().filter(|r| r.enabled) {
            match run::sync_list(row) {
                Ok(s) => info!(
                    "list \"{}\": {} of {} entries done",
                    row.name, s.done, s.total
                ),
                Err(e) => warn!("list \"{}\": {e}", row.name),
            }
        }
        Ok(())
    }

    fn http(req: HttpRequest) -> Result<HttpResponse, String> {
        routes::router().handle(req)
    }
}

fliks_plugin_sdk::export!(Lists);
