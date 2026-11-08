//! A list sync against the live host: the plugin's own database, the web and the host imports.
use crate::model::{Rec, Settings};
use crate::net::{Live as Web, Net};
use crate::sync::{Ctx, HErr, Host, Summary};
use crate::{db, sources, spotify, sync};
use fliks_plugin_sdk::bindings::fliks::plugin::types::{ExternalId, HostError, RecordingRef};
use fliks_plugin_sdk::host;
use serde_json::{Value, json};

/// One resolution can take a minute: no new entry starts with less left.
const MIN_LEFT_MS: u64 = 70_000;

fn herr(e: HostError) -> HErr {
    HErr {
        code: e.code,
        message: e.message,
    }
}

fn recording(rec: &Rec) -> RecordingRef {
    match rec {
        Rec::Isrc(i) => RecordingRef::Isrc(i.clone()),
        Rec::Mbid(m) => RecordingRef::Mbid(m.clone()),
    }
}

pub struct LiveHost;

impl Host for LiveHost {
    fn album(&self, ids: &[(String, String)], library: Option<u32>) -> Result<u32, HErr> {
        let ids: Vec<ExternalId> = ids
            .iter()
            .map(|(p, v)| ExternalId {
                provider: p.clone(),
                value: v.clone(),
            })
            .collect();
        host::media_resolve_or_create("album", &ids, library, None).map_err(herr)
    }

    fn want(&self, rec: &Rec, library: Option<u32>) -> Result<(u32, u32), HErr> {
        host::music_want_recording(&recording(rec), library)
            .map(|w| (w.media_id, w.item_id))
            .map_err(herr)
    }

    fn find(&self, rec: &Rec) -> Result<Option<(u32, u32)>, HErr> {
        host::music_find_recording(&recording(rec))
            .map(|o| o.map(|w| (w.media_id, w.item_id)))
            .map_err(herr)
    }

    fn playlist(&self, user: u32, name: &str, items: &[u32]) -> Result<u32, HErr> {
        host::playlist_sync(user, name, items).map_err(herr)
    }

    fn isrc_of(&self, track: &str) -> Option<String> {
        Web.pause(150);
        sources::deezer_isrc(&Web, track)
    }

    fn out_of_time(&self) -> bool {
        host::deadline_remaining_ms() < MIN_LEFT_MS
    }
}

/// An access token for a connected Spotify list; the refresh token is kept as Spotify rotates it.
pub fn spotify_access(id: i64, st: &Settings) -> Result<String, String> {
    let mut data = db::secrets(id)?;
    let current = data
        .get("refreshToken")
        .and_then(Value::as_str)
        .ok_or("Spotify is not connected: open the connection link of the list")?
        .to_owned();
    let (access, next) = spotify::refresh(&Web, st, &current)?;
    if next != current {
        data.insert("refreshToken".into(), Value::String(next));
        db::set_secrets(id, &data)?;
    }
    Ok(access)
}

pub fn summary_json(s: &Summary) -> Value {
    json!({
        "total": s.total, "done": s.done, "unresolved": s.unresolved, "errors": s.errors,
        "playlistId": s.playlist, "complete": s.complete,
    })
}

/// Reads the list and acts on its new entries; the outcome is stored on the list either way.
pub fn sync_list(row: &db::Row) -> Result<Summary, String> {
    let run = || -> Result<Summary, String> {
        let token = (row.implementation == "spotify")
            .then(|| spotify_access(row.id, &row.settings))
            .transpose()?;
        let entries = sources::fetch(&Web, &row.implementation, &row.settings, token.as_deref())?;
        let ctx = Ctx {
            id: row.id,
            name: &row.name,
            owner: row.owner,
            settings: &row.settings,
        };
        sync::run(&ctx, &entries, &LiveHost, &db::Live)
    };
    let out = run();
    let (error, summary) = match &out {
        Ok(s) => (s.last_error.clone(), summary_json(s)),
        Err(e) => (Some(e.clone()), json!({})),
    };
    db::set_result(row.id, error.as_deref(), &summary)?;
    out
}
