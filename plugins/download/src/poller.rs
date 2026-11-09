//! `ImportCompleted`, `CleanStalled`, `CleanSeeded` and the queue snapshot they report to the core.

use crate::client::{self, Torrent, TorrentFile};
use crate::config;
use crate::db::{DownloadClientRow, HistoryRow, blocklist, clients, history, indexers, stalled};
use crate::error::{Error, R};
use crate::grab::{
    comic_item, coverage_columns, family_of, family_str, group_item_ids, identify, title_formats,
};
use crate::util::{decode_html_entities, js_number, now_ms, parse_date_ms};
use fliks_plugin_sdk::bindings::fliks::plugin::types::{
    AcquisitionEvent, Failed, Family, GrabMethod, Imported, IngestRequest, IngestResult,
    QueueEntry, QueueState, ResolveQuery, SkipReason, StalledRemoved, TransferMode, UnitRef,
    WantDecision,
};
use fliks_plugin_sdk::host;
use regex::Regex;
use serde_json::{Value, json};
use std::collections::{HashMap, HashSet};
use std::sync::{LazyLock, Mutex};
use std::time::{Duration, Instant};

/// The host's own formats (`domain::media`): the ingest refuses any other path.
const VIDEO_EXTS: [&str; 14] = [
    ".mkv", ".mp4", ".avi", ".mov", ".ts", ".m2ts", ".wmv", ".flv", ".webm", ".m4v", ".mpg",
    ".mpeg", ".ogm", ".mts",
];
pub const AUDIO_EXTS: [&str; 7] = [".m4b", ".m4a", ".mp3", ".flac", ".ogg", ".opus", ".aac"];
/// An album's tracks: the books' audio plus the lossless and uncompressed containers (`domain::media`).
pub const MUSIC_EXTS: [&str; 13] = [
    ".m4b", ".m4a", ".mp3", ".flac", ".ogg", ".opus", ".aac", ".wav", ".aiff", ".aif", ".ape",
    ".wv", ".dsf",
];
pub const COMIC_EXTS: [&str; 4] = [".cbz", ".cbr", ".cb7", ".pdf"];
pub const TEXT_EXTS: [&str; 6] = [".epub", ".pdf", ".azw3", ".mobi", ".fb2", ".cbz"];
/// Media the ingest refuses: a torrent holding them is never taken for a dud.
const REFUSED_MEDIA_EXTS: [&str; 8] = [
    ".oga", ".wma", ".wav", ".azw", ".djvu", ".kfx", ".cbr", ".cb7",
];
const ORPHAN_GRACE_MS: i64 = 5 * 60_000;
const ORPHAN_STATUS_MESSAGE: &str = "Torrent no longer present in download client";
const RESOLVE_BATCH: usize = 50;
/// Ingest takes up to 30 min: no new import starts with less left of the job's hour.
const IMPORT_MIN_LEFT: Duration = Duration::from_secs(32 * 60);

/// Log dedupe across ticks of a job instance; a re-created instance logs the line again.
static UNIDENTIFIED: Mutex<Option<HashSet<String>>> = Mutex::new(None);
static UNTRACKED: Mutex<Option<HashSet<String>>> = Mutex::new(None);
static STALL_SKIP: Mutex<Option<String>> = Mutex::new(None);

fn seen(set: &Mutex<Option<HashSet<String>>>, key: &str) -> bool {
    set.lock()
        .ok()
        .and_then(|s| s.as_ref().map(|s| s.contains(key)))
        .unwrap_or(false)
}

fn remember(set: &Mutex<Option<HashSet<String>>>, keys: HashSet<String>) {
    if let Ok(mut s) = set.lock() {
        *s = Some(keys);
    }
}

fn stall_skip(reason: Option<&str>) {
    let Ok(mut last) = STALL_SKIP.lock() else {
        return;
    };
    if let Some(r) = reason.filter(|r| last.as_deref() != Some(*r)) {
        info!("StalledCleanup: skipped, {r}");
    }
    *last = reason.map(str::to_owned);
}

pub struct Held {
    pub client_id: i64,
    pub t: Torrent,
}

/// Every enabled client's torrents, with whether all of them answered.
pub fn fetch_all() -> R<(Vec<Held>, bool)> {
    let mut out = vec![];
    let mut all_ok = true;
    for c in clients::list_enabled()?
        .into_iter()
        .filter(client::supports)
    {
        let (ok, ts) = client::torrents(&c);
        all_ok &= ok;
        out.extend(ts.into_iter().map(|t| Held { client_id: c.id, t }));
    }
    Ok((out, all_ok))
}

// --- matching torrents to history rows -------------------------------------------------------

static NAME_SEP: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"[._\s]+").unwrap());
static DOT_TORRENT: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"(?i)\.torrent$").unwrap());

/// Entities decoded, `.torrent` dropped, separators collapsed, lowercased.
pub fn normalise_name(raw: &str) -> String {
    if raw.is_empty() {
        return String::new();
    }
    let s = decode_html_entities(raw);
    let s = DOT_TORRENT.replace(&s, "");
    NAME_SEP.replace_all(&s, " ").trim().to_lowercase()
}

fn authority(h: &HistoryRow) -> i32 {
    (if h.media_id.is_some_and(|m| m != 0) {
        2
    } else {
        0
    }) + i32::from(h.is_live())
}

pub fn outranks(candidate: &HistoryRow, current: &HistoryRow) -> bool {
    let d = authority(candidate) - authority(current);
    d > 0 || (d == 0 && candidate.id > current.id)
}

fn best<'a>(rows: impl Iterator<Item = &'a HistoryRow>) -> Option<&'a HistoryRow> {
    rows.fold(None, |b: Option<&HistoryRow>, h| match b {
        Some(b) if !outranks(h, b) => Some(b),
        _ => Some(h),
    })
}

/// Hash, then exact normalised name, then a unique prefix overlap; several prefixes abort.
pub fn find_match<'a>(
    hash: &str,
    name: &str,
    rows: &'a [HistoryRow],
) -> Option<(&'a HistoryRow, bool)> {
    let hash = hash.to_lowercase();
    if !hash.is_empty()
        && let Some(h) = best(rows.iter().filter(|h| {
            h.torrent_hash
                .as_ref()
                .is_some_and(|t| t.to_lowercase() == hash)
        }))
    {
        return Some((h, true));
    }
    let name = normalise_name(name);
    let titles: Vec<String> = rows
        .iter()
        .map(|h| normalise_name(&h.source_title))
        .collect();
    if let Some(h) = best(
        rows.iter()
            .zip(&titles)
            .filter(|(_, t)| **t == name)
            .map(|(h, _)| h),
    ) {
        return Some((h, false));
    }
    let prefix: Vec<&HistoryRow> = rows
        .iter()
        .zip(&titles)
        .filter(|(_, t)| !t.is_empty() && (name.starts_with(t.as_str()) || t.starts_with(&name)))
        .map(|(h, _)| h)
        .collect();
    match prefix.len() {
        1 => Some((prefix[0], false)),
        0 => None,
        n => {
            warn!(
                "TorrentHistoryMatcher: {n} histories with prefix overlap on \"{name}\" — skipped"
            );
            None
        }
    }
}

/// Match, and persist the hash on a row matched by name.
fn match_and_heal(t: &Torrent, rows: &mut [HistoryRow]) -> R<Option<HistoryRow>> {
    let Some((h, by_hash)) = find_match(&t.hash, &t.name, rows) else {
        return Ok(None);
    };
    let id = h.id;
    let row = rows
        .iter_mut()
        .find(|r| r.id == id)
        .expect("the matched row");
    if !by_hash && !t.hash.is_empty() && row.torrent_hash.as_deref().is_none_or(str::is_empty) {
        history::update_hash(id, &t.hash.to_lowercase())?;
        row.torrent_hash = Some(t.hash.to_lowercase());
    }
    Ok(Some(row.clone()))
}

// --- progress ------------------------------------------------------------------------------

fn queue_state(s: &str) -> QueueState {
    match s {
        "queued" => QueueState::Queued,
        "stalled" => QueueState::Stalled,
        "paused" => QueueState::Paused,
        "importing" => QueueState::Importing,
        _ => QueueState::Active,
    }
}

/// The unit behind each row's ids, batched within `media.resolve`'s bound.
fn row_units(rows: &[&HistoryRow]) -> HashMap<i64, UnitRef> {
    let mut out = HashMap::new();
    let groups: Vec<u32> = dedup(
        rows.iter()
            .filter(|r| r.item_id.is_none())
            .filter_map(|r| r.season_id)
            .map(|i| i as u32),
    );
    let items: Vec<u32> = dedup(rows.iter().filter_map(|r| r.item_id).map(|i| i as u32));
    let mut i = 0;
    while i < groups.len().max(items.len()) {
        let q = ResolveQuery {
            media_ids: vec![],
            group_ids: groups.iter().skip(i).take(RESOLVE_BATCH).copied().collect(),
            item_ids: items.iter().skip(i).take(RESOLVE_BATCH).copied().collect(),
        };
        i += RESOLVE_BATCH;
        let resolved: HashMap<String, _> = match host::media_resolve(&q) {
            Ok(r) => r.into_iter().collect(),
            Err(e) => {
                warn!("Import: scope resolve failed: {}", e.message);
                continue;
            }
        };
        for r in rows {
            let key = match (r.item_id, r.season_id) {
                (Some(i), _) => format!("item:{i}"),
                (None, Some(g)) => format!("group:{g}"),
                _ => continue,
            };
            if let Some(unit) = resolved.get(&key).and_then(|hit| hit.unit.clone()) {
                out.insert(r.id, unit);
            }
        }
    }
    out
}

fn dedup(it: impl Iterator<Item = u32>) -> Vec<u32> {
    let mut seen = HashSet::new();
    it.filter(|x| seen.insert(*x)).collect()
}

/// How long a finished row stays in the snapshot; the core closes it once, however often it is listed.
const FINISHED_WINDOW_MIN: i64 = 10;
/// A row retired on purpose leaves no history entry: the core records it as removed.
const WITHDRAWN: [&str; 2] = [
    "download.queue.removed_by_user",
    "download.queue.retired_unverifiable",
];

fn method_of(s: &str) -> GrabMethod {
    match s {
        "manual" => GrabMethod::Manual,
        "rss" => GrabMethod::Rss,
        _ => GrabMethod::Auto,
    }
}

/// The core refuses a field outside 1-500 characters and then the whole snapshot.
fn clamp(s: &str, fallback: &str) -> String {
    let s: String = s.trim().chars().take(500).collect();
    if s.is_empty() { fallback.into() } else { s }
}

fn queue_entry(
    row: &HistoryRow,
    unit: Option<UnitRef>,
    names: &HashMap<i64, String>,
    state: QueueState,
    t: Option<&Torrent>,
) -> Option<QueueEntry> {
    let finished = matches!(state, QueueState::Imported | QueueState::Failed);
    Some(QueueEntry {
        ref_: row.id.to_string(),
        media_id: u32::try_from(row.media_id.filter(|m| *m != 0)?).ok()?,
        unit,
        source_id: row.indexer_id.map_or_else(|| "0".into(), |i| i.to_string()),
        source_name: clamp(
            row.indexer_id
                .and_then(|i| names.get(&i))
                .map_or("", String::as_str),
            "Download",
        ),
        title: clamp(&row.source_title, "Untitled"),
        quality: clamp(&row.quality, "Unknown"),
        method: method_of(&row.grab_source),
        info_url: row.info_url.clone(),
        state,
        progress: if finished || matches!(state, QueueState::ImportFailed | QueueState::Importing) {
            1.0
        } else {
            t.map_or(0.0, |t| t.progress)
        },
        size_bytes: t
            .map(|t| t.size)
            .or(row.size)
            .and_then(|n| u64::try_from(n).ok())
            .filter(|n| *n > 0),
        bytes_per_second: t.map(|t| t.dlspeed),
        eta_seconds: t.and_then(|t| (t.eta > 0.0 && t.eta < 8_640_000.0).then_some(t.eta)),
        error: matches!(state, QueueState::ImportFailed | QueueState::Failed)
            .then(|| row.status_message.as_deref().map(|m| clamp(m, "Failed")))
            .flatten(),
    })
}

/// The plugin's transfers: every open row with what its client reports (stalled when the client does
/// not list it), and the rows that finished lately.
fn build_queue(all: &[Held]) -> R<Vec<QueueEntry>> {
    let mut rows = history::by_statuses(&["grabbed", "importing", "import_failed"])?;
    let mut live: HashMap<i64, &Torrent> = HashMap::new();
    for h in all {
        if let Some(row) = match_and_heal(&h.t, &mut rows)? {
            live.insert(row.id, &h.t);
        }
    }
    let finished = history::recent_terminal(FINISHED_WINDOW_MIN)?;
    let listed: Vec<&HistoryRow> = rows
        .iter()
        .chain(finished.iter())
        .filter(|r| r.media_id.is_some_and(|m| m != 0))
        .collect();
    let units = row_units(&listed);
    let names: HashMap<i64, String> = indexers::list_all()?
        .into_iter()
        .map(|ix| (ix.id, ix.name))
        .collect();
    let mut out = vec![];
    for row in &rows {
        let t = live.get(&row.id).copied();
        let state = match row.status.as_str() {
            "importing" => QueueState::Importing,
            "import_failed" => QueueState::ImportFailed,
            _ if t.is_some() => queue_state(client::progress_state(&t.expect("live").state)),
            // Never reached a client: that is what queued means.
            _ if row.torrent_hash.as_deref().is_none_or(str::is_empty) => QueueState::Queued,
            // Its client does not list it (down, or gone): shown until the reconcile retires the row.
            _ => QueueState::Stalled,
        };
        out.extend(queue_entry(
            row,
            units.get(&row.id).cloned(),
            &names,
            state,
            t,
        ));
    }
    for row in &finished {
        let state = if row.status == "completed" {
            QueueState::Imported
        } else if row
            .status_message
            .as_deref()
            .is_some_and(|m| WITHDRAWN.contains(&m))
        {
            continue;
        } else {
            QueueState::Failed
        };
        out.extend(queue_entry(
            row,
            units.get(&row.id).cloned(),
            &names,
            state,
            None,
        ));
    }
    Ok(out)
}

/// States the whole queue to the core; the rows of a client that did not answer stay listed as stalled.
fn publish_queue(all: &[Held]) -> R<()> {
    if let Err(e) = host::queue_set(&build_queue(all)?) {
        warn!("Import: queue publish failed: {}", e.message);
    }
    Ok(())
}

pub type Outcome = &'static str;

fn reached(all: &[Held], hash: &str, expect: Outcome) -> bool {
    let Some(h) = all.iter().find(|h| h.t.hash.eq_ignore_ascii_case(hash)) else {
        return true;
    };
    if expect == "absent" {
        return false;
    }
    let paused = client::progress_state(&h.t.state) == "paused";
    if expect == "paused" { paused } else { !paused }
}

/// Waits (up to 4 s) for the client to reflect a control, then states the queue.
pub fn settle_and_publish(row: &HistoryRow, expect: Outcome) -> R<()> {
    let Some(hash) = row.torrent_hash.as_deref() else {
        return Ok(());
    };
    let deadline = Instant::now() + Duration::from_secs(4);
    loop {
        let (all, _) = fetch_all()?;
        if reached(&all, hash, expect) || Instant::now() >= deadline {
            return publish_queue(&all);
        }
        std::thread::sleep(Duration::from_millis(300));
    }
}

// --- ImportCompleted ---------------------------------------------------------------------------

fn publish(ev: AcquisitionEvent, what: &str, title: &str) {
    if let Err(e) = host::events_publish(&[ev]) {
        warn!("Import[{title}]: failed to publish {what}: {}", e.message);
    }
}

fn publish_failed(h: &HistoryRow, reason: &str) {
    if let Some(m) = h.media_id {
        let ev = AcquisitionEvent::Failed(Failed {
            media_id: m as u32,
            title: h.source_title.clone(),
            reason: reason.into(),
        });
        publish(ev, "acquisition.failed", &h.source_title);
    }
}

fn publish_queue_changed(context: &str) {
    if let Err(e) = host::events_publish(&[AcquisitionEvent::QueueChanged]) {
        warn!("{context}: queue-changed publish failed: {}", e.message);
    }
}

pub fn poll() -> R<()> {
    let swept = history::sweep_import_leases()?;
    if swept > 0 {
        warn!("Import: {swept} importing entries outlived their lease — re-armed as grabbed");
    }
    let enabled: Vec<DownloadClientRow> = clients::list_enabled()?
        .into_iter()
        .filter(client::supports)
        .collect();
    if enabled.is_empty() {
        warn!("Import: no enabled download client found");
        return publish_queue(&[]);
    }
    let (all, all_ok) = fetch_all()?;
    auto_match(&all)?;
    let mut grabbed = history::by_statuses(&["grabbed", "failed", "import_failed", "warning"])?;
    let importing = history::by_statuses(&["importing"])?;
    if all_ok {
        reconcile(&all, &grabbed, &importing)?;
    }
    publish_queue(&all)?;

    let completed: Vec<&Held> = all
        .iter()
        .filter(|h| {
            h.t.progress >= 1.0
                || ["seeding", "stalledUP", "stoppedUP"].contains(&h.t.state.as_str())
        })
        .collect();
    if completed.is_empty() {
        return Ok(());
    }
    let mut imported = 0;
    for held in &completed {
        let Some(row) = match_and_heal(&held.t, &mut grabbed)? else {
            continue;
        };
        // A failed row is re-armed only by the orphan sweep or a manual retry.
        if row.status != "grabbed" && row.status != "warning" {
            continue;
        }
        if Duration::from_millis(host::deadline_remaining_ms()) < IMPORT_MIN_LEFT {
            info!(
                "Import: too little of the job left to start another ingest, the next tick continues"
            );
            break;
        }
        info!(
            "Import: torrent \"{}\" -> history #{} (mediaId={:?}, status={})",
            held.t.name, row.id, row.media_id, row.status
        );
        let Some(client) = enabled.iter().find(|c| c.id == held.client_id) else {
            let message = "download client for this torrent is no longer enabled";
            warn!("Import: \"{}\" — {message}; will retry", row.source_title);
            history::set_status(&[row.id], "grabbed", Some(message))?;
            continue;
        };
        let run = || {
            history::mark_importing(row.id)?;
            publish_queue(&all)?;
            process_one(&row, &held.t, client)
        };
        match run() {
            Ok(()) => imported += 1,
            Err(e) if e.is_unknown_outcome() => {
                warn!(
                    "Import: \"{}\" — core did not confirm ({e}); will retry",
                    row.source_title
                );
                history::set_status(&[row.id], "grabbed", Some(&e.to_string()))?;
            }
            Err(e) if e.is_retry_later() => {
                if history::retry_later_expired(row.id)? {
                    let msg = format!(
                        "Import failed: still refused after {} hours: {e}",
                        history::RETRY_LATER_CAP_HOURS
                    );
                    error!("Import: FAILED for \"{}\": {msg}", row.source_title);
                    history::set_status(&[row.id], "import_failed", Some(&msg))?;
                    publish_failed(&row, &msg);
                } else {
                    warn!("Import: \"{}\": {e}; will retry", row.source_title);
                    history::set_status(&[row.id], "grabbed", Some(&e.to_string()))?;
                }
            }
            Err(e) => {
                // Core's own state (a full disk, a missing root), never a verdict on the release.
                error!("Import: FAILED for \"{}\": {e}", row.source_title);
                history::set_status(&[row.id], "import_failed", Some(&e.to_string()))?;
                publish_failed(&row, &e.to_string());
            }
        }
    }
    if imported > 0 {
        info!(
            "Import: processed {imported}/{} completed torrent(s)",
            completed.len()
        );
    }
    Ok(())
}

/// Binds torrents no row claims to the media `releases.match` names.
fn auto_match(all: &[Held]) -> R<()> {
    if all.is_empty() {
        return Ok(());
    }
    let hashes: Vec<String> = all
        .iter()
        .map(|h| h.t.hash.clone())
        .filter(|h| !h.is_empty())
        .collect();
    let mut by_hash: HashMap<String, HistoryRow> = HashMap::new();
    for h in history::find_by_hashes(&hashes)? {
        let Some(key) = h.torrent_hash.as_ref().map(|t| t.to_lowercase()) else {
            continue;
        };
        if by_hash.get(&key).is_none_or(|kept| outranks(&h, kept)) {
            by_hash.insert(key, h);
        }
    }
    let candidates: Vec<&Held> = all
        .iter()
        .filter(|h| !h.t.hash.is_empty())
        .filter(|h| {
            by_hash
                .get(&h.t.hash.to_lowercase())
                .is_none_or(|r| r.media_id.is_none_or(|m| m == 0))
        })
        .collect();
    if candidates.is_empty() {
        remember(&UNIDENTIFIED, HashSet::new());
        return Ok(());
    }
    let linked: HashSet<String> = history::linked_source_titles()?
        .iter()
        .map(|t| normalise_name(t))
        .collect();
    let to_identify: Vec<&Held> = candidates
        .into_iter()
        .filter(|h| !linked.contains(&normalise_name(&h.t.name)))
        .collect();
    if to_identify.is_empty() {
        return Ok(());
    }
    let matches = identify(
        &to_identify
            .iter()
            .map(|h| h.t.name.clone())
            .collect::<Vec<_>>(),
    )?;
    let (mut bound, mut rebound) = (0, 0);
    let mut still = HashSet::new();
    for held in to_identify {
        let hash = held.t.hash.to_lowercase();
        let Some(Some(m)) = matches.get(&held.t.name).map(|m| {
            m.as_ref().filter(|m| {
                !matches!(
                    m.skip,
                    Some(SkipReason::NotAvailable | SkipReason::SceneUnresolved)
                )
            })
        }) else {
            if !seen(&UNIDENTIFIED, &hash) {
                info!(
                    "Auto-match: \"{}\" — releases.match found no media for it",
                    held.t.name
                );
            }
            still.insert(hash);
            continue;
        };
        let comic_item = match (&m.unit, m.season_number) {
            (Some(u), None) if u.number.is_some() => comic_item(m.media_id, u)?,
            _ => None,
        };
        let (group_id, item_id) = match comic_item {
            Some(item) => (None, Some(item)),
            None => group_item_ids(m.media_id, m.season_number, m.episode_number)?,
        };
        let cov = coverage_columns(m.coverage.as_ref());
        match by_hash.get(&hash) {
            Some(row) => {
                history::heal_match(
                    row.id,
                    m.media_id.into(),
                    item_id,
                    group_id,
                    &row.quality,
                    &cov,
                )?;
                rebound += 1;
            }
            None => {
                history::insert_grab(&history::Grab {
                    source_title: decode_html_entities(&held.t.name),
                    quality: "unknown".into(),
                    family: comic_item.map(|_| "image"),
                    language: None,
                    torrent_hash: Some(held.t.hash.clone()).filter(|h| !h.is_empty()),
                    size: Some(held.t.size).filter(|s| *s != 0),
                    info_url: None,
                    grab_source: "manual",
                    media_id: m.media_id.into(),
                    item_id,
                    season_id: group_id,
                    indexer_id: None,
                    download_client_id: Some(held.client_id),
                    coverage: cov,
                })?;
                bound += 1;
            }
        }
    }
    remember(&UNIDENTIFIED, still);
    if bound + rebound > 0 {
        info!("Auto-match: done — {bound} created, {rebound} healed");
    }
    Ok(())
}

fn reconcile(all: &[Held], grabbed: &[HistoryRow], importing: &[HistoryRow]) -> R<()> {
    if grabbed.is_empty() && importing.is_empty() {
        return Ok(());
    }
    let candidates: Vec<HistoryRow> = grabbed.iter().chain(importing).cloned().collect();
    let mut torrent_of: HashMap<i64, &Torrent> = HashMap::new();
    for h in all {
        if let Some((row, _)) = find_match(&h.t.hash, &h.t.name, &candidates) {
            torrent_of.insert(row.id, &h.t);
        }
    }
    let mut changed = false;
    let restarted: Vec<i64> = importing
        .iter()
        .filter(|h| torrent_of.get(&h.id).is_some_and(|t| t.progress < 1.0))
        .map(|h| h.id)
        .collect();
    if !restarted.is_empty() {
        history::set_status(&restarted, "grabbed", None)?;
        changed = true;
        warn!(
            "Import: {} importing entries whose torrent is no longer complete — re-armed as grabbed",
            restarted.len()
        );
    }
    let revived: Vec<i64> = grabbed
        .iter()
        .filter(|h| {
            h.status == "failed"
                && h.status_message.as_deref() == Some(ORPHAN_STATUS_MESSAGE)
                && torrent_of.contains_key(&h.id)
        })
        .map(|h| h.id)
        .collect();
    if !revived.is_empty() {
        history::set_status(&revived, "grabbed", None)?;
        changed = true;
        info!(
            "Import: {} entries reappeared in the download client — cleared the orphan stamp",
            revived.len()
        );
    }
    let cutoff = now_ms() - ORPHAN_GRACE_MS;
    let expired: Vec<i64> = candidates
        .iter()
        .filter(|h| ["grabbed", "importing", "import_failed"].contains(&h.status.as_str()))
        .filter(|h| {
            !torrent_of.contains_key(&h.id)
                && parse_date_ms(&h.updated_at).is_some_and(|u| u < cutoff)
        })
        .map(|h| h.id)
        .collect();
    if !expired.is_empty() {
        history::set_status(&expired, "failed", Some(ORPHAN_STATUS_MESSAGE))?;
        changed = true;
        warn!(
            "Import: {} grabbed/importing entries lost their torrent for > 5min — marked failed",
            expired.len()
        );
    }
    if changed {
        publish_queue_changed("Import");
    }
    Ok(())
}

/// `path.join` with the separator of `save_path` (a drive letter or `\` means Windows).
pub fn join_path(save_path: &str, name: &str) -> String {
    let windows = save_path.contains('\\') || save_path.as_bytes().get(1) == Some(&b':');
    let sep = if windows { '\\' } else { '/' };
    let name = if windows {
        name.replace('/', "\\")
    } else {
        name.to_owned()
    };
    if save_path.is_empty() {
        return name;
    }
    format!(
        "{}{sep}{}",
        save_path.trim_end_matches(sep),
        name.trim_start_matches(sep)
    )
}

fn ext_of(name: &str) -> String {
    let base = name.rsplit(['/', '\\']).next().unwrap_or(name);
    match base.rfind('.') {
        Some(i) if i > 0 => base[i..].to_lowercase(),
        _ => String::new(),
    }
}

pub fn auto_blocklist(h: &HistoryRow, note: &str) {
    let name = h
        .indexer_id
        .and_then(|id| indexers::find(id).ok().flatten())
        .map(|ix| ix.name);
    let _ = blocklist::insert(&blocklist::Entry {
        source_title: &h.source_title,
        quality: Some(&h.quality),
        media_id: h.media_id,
        indexer_id: h.indexer_id,
        indexer_name: name.as_deref(),
        note: Some(note),
    });
}

#[derive(Debug, PartialEq)]
enum Ingested {
    Imported,
    AlreadyPresent,
    NothingPlaced,
}

fn classify(r: &IngestResult) -> Ingested {
    match (r.imported.is_empty(), r.already_present.is_empty()) {
        (false, _) => Ingested::Imported,
        (true, false) => Ingested::AlreadyPresent,
        (true, true) => Ingested::NothingPlaced,
    }
}

fn exts_of(f: Family) -> &'static [&'static str] {
    match f {
        Family::Audio => &AUDIO_EXTS,
        Family::Text => &TEXT_EXTS,
        Family::Image => &COMIC_EXTS,
        Family::Video => &VIDEO_EXTS,
    }
}

/// The finished files of `family` the ingest accepts.
fn paths_of(files: &[TorrentFile], save: &str, family: Family) -> Vec<String> {
    files
        .iter()
        .filter(|f| f.progress >= 1.0 && exts_of(family).contains(&ext_of(&f.name).as_str()))
        .map(|f| join_path(save, &f.name))
        .collect()
}

/// Any media file of any family, finished or not, accepted by the ingest or not.
fn holds_media(files: &[TorrentFile]) -> bool {
    files.iter().any(|f| {
        let e = ext_of(&f.name);
        [Family::Video, Family::Audio, Family::Text, Family::Image]
            .into_iter()
            .any(|fam| exts_of(fam).contains(&e.as_str()))
            || MUSIC_EXTS.contains(&e.as_str())
            || REFUSED_MEDIA_EXTS.contains(&e.as_str())
    })
}

/// The language a row was grabbed for, when `family` is the family it was grabbed for.
fn row_language(h: &HistoryRow, family: Family) -> Option<&str> {
    (h.family.as_deref().and_then(family_of) == Some(family))
        .then_some(h.language.as_deref())
        .flatten()
}

/// A family of a torrent to ingest, with the language its want has now.
struct Intake {
    family: Family,
    language: Option<String>,
    paths: Vec<String>,
}

/// Whether a row is an album: its family says audio, then the host says album or book. A row with
/// video ids is never one.
fn is_album(h: &HistoryRow, media: u32) -> R<bool> {
    let family = h.family.as_deref().and_then(family_of);
    if family.is_some_and(|f| f != Family::Audio)
        || (family.is_none() && (h.season_id.is_some() || h.item_id.is_some()))
    {
        return Ok(false);
    }
    Ok(
        host::media_acquisition_context(media, family, None, None, None)?
            .is_some_and(|t| t.kind == "album"),
    )
}

/// Series rows carry their ids; a movie or a book row without a family asks the host once.
fn is_book(h: &HistoryRow, media: u32) -> R<bool> {
    match h.family.as_deref().and_then(family_of) {
        Some(f) => Ok(matches!(f, Family::Text | Family::Audio)),
        None if h.season_id.is_some() || h.item_id.is_some() => Ok(false),
        None => Ok(
            host::media_acquisition_context(media, None, None, None, None)?
                .ok_or_else(|| Error::Other("the host no longer knows this media".into()))?
                .kind
                == "book",
        ),
    }
}

/// The families to ingest from a torrent: a row's own, then every other wished book family the
/// files carry; a row without one takes the first family found. Empty when nothing is ingestable.
fn intake(h: &HistoryRow, media: u32, files: &[TorrentFile], save: &str) -> R<Vec<Intake>> {
    let own = h.family.as_deref().and_then(family_of);
    if own == Some(Family::Image) {
        let paths = paths_of(files, save, Family::Image);
        return Ok(Some(Intake {
            family: Family::Image,
            language: None,
            paths,
        })
        .filter(|i| !i.paths.is_empty())
        .into_iter()
        .collect());
    }
    if is_album(h, media)? {
        if own.is_none() {
            history::set_family(h.id, family_str(Family::Audio))?;
        }
        let paths: Vec<String> = files
            .iter()
            .filter(|f| f.progress >= 1.0 && MUSIC_EXTS.contains(&ext_of(&f.name).as_str()))
            .map(|f| join_path(save, &f.name))
            .collect();
        return Ok(Some(Intake {
            family: Family::Audio,
            language: None,
            paths,
        })
        .filter(|i| !i.paths.is_empty())
        .into_iter()
        .collect());
    }
    let video = !is_book(h, media)?;
    if video {
        let paths = paths_of(files, save, Family::Video);
        let found = Intake {
            family: Family::Video,
            language: None,
            paths,
        };
        return Ok(Some(found)
            .filter(|i| !i.paths.is_empty())
            .into_iter()
            .collect());
    }
    let mut out: Vec<Intake> = vec![];
    for family in own.into_iter().chain([Family::Text, Family::Audio]) {
        let paths = paths_of(files, save, family);
        if paths.is_empty() || out.iter().any(|i| i.family == family) {
            continue;
        }
        let Some(target) = host::media_acquisition_context(
            media,
            Some(family),
            row_language(h, family),
            None,
            None,
        )?
        else {
            continue;
        };
        let missing = target
            .want
            .as_ref()
            .is_none_or(|w| w.decision != WantDecision::Skip);
        let named = title_formats(&h.source_title)
            .iter()
            .any(|(f, _)| *f == family);
        if own == Some(family) || (missing && named) || (own.is_none() && out.is_empty()) {
            out.push(Intake {
                family,
                language: target.language,
                paths,
            });
        }
    }
    if own.is_none()
        && let Some(first) = out.first()
    {
        history::set_family(h.id, family_str(first.family))?;
    }
    Ok(out)
}

fn process_one(h: &HistoryRow, t: &Torrent, client: &DownloadClientRow) -> R<()> {
    let Some(files) = client::files(client, &t.hash) else {
        let message = format!("could not list files for \"{}\"", t.name);
        warn!("Import[{}]: {message}; will retry", h.source_title);
        return history::set_status(&[h.id], "grabbed", Some(&message));
    };
    let Some(media) = h.media_id else {
        return history::set_status(
            &[h.id],
            "import_failed",
            Some("Import failed: no media linked to this download"),
        );
    };
    let save = t.save_path.clone().unwrap_or_default();
    let plan = intake(h, media as u32, &files, &save)?;
    let what = h.family.as_deref().unwrap_or("media");
    if plan.is_empty() {
        if holds_media(&files) {
            let msg = format!(
                "Import failed: no {what} file this media wishes in the download \"{}\", which holds other media files",
                t.name
            );
            warn!("Import[{}]: {msg}", h.source_title);
            history::set_status(&[h.id], "import_failed", Some(&msg))?;
            publish_failed(h, &msg);
            return Ok(());
        }
        let msg = format!(
            "Import failed: no media file in the download \"{}\"",
            t.name
        );
        warn!("Import[{}]: {msg}", h.source_title);
        history::mark_failed(h.id, &msg)?;
        auto_blocklist(h, "Auto-blocklist: no media file in the download");
        if let Err(e) = client::delete(client, &t.hash, true) {
            warn!(
                "Import[{}]: failed to remove dud torrent: {e}",
                h.source_title
            );
        }
        publish_failed(h, &msg);
        return Ok(());
    }
    let base = h
        .family
        .as_deref()
        .and_then(family_of)
        .unwrap_or(plan[0].family);
    let (mut files_placed, mut own_placed) = (0, false);
    for it in plan {
        let (family, key) = (
            it.family,
            if it.family == base {
                format!("download-history:{}", h.id)
            } else {
                format!("download-history:{}:{}", h.id, family_str(it.family))
            },
        );
        let result = match host::library_ingest(&IngestRequest {
            idempotency_key: key,
            media_id: media as u32,
            family,
            language: it.language,
            paths: it.paths,
            transfer: TransferMode::Copy,
            fallback_quality: None,
            source_label: h.source_title.clone(),
        }) {
            Ok(r) => r,
            Err(e) if family != base && !Error::from(e.clone()).is_unknown_outcome() => {
                warn!(
                    "Import[{}]: the {} ingest failed: {e}",
                    h.source_title,
                    family_str(family)
                );
                continue;
            }
            Err(e) => return Err(e.into()),
        };
        let ingested = classify(&result);
        if family == Family::Image
            && let Some(item) = h.item_id
            && !result
                .imported
                .iter()
                .any(|f| f.item_ids.contains(&(item as u32)))
            && !result.imported.is_empty()
        {
            let msg = format!(
                "Import failed: \"{}\" did not land in the issue it was grabbed for",
                h.source_title
            );
            warn!("Import[{}]: {msg}", h.source_title);
            history::mark_failed(h.id, &msg)?;
            auto_blocklist(
                h,
                "Auto-blocklist: the file names another issue than the one wanted",
            );
            publish_failed(h, &msg);
            return Ok(());
        }
        if ingested == Ingested::NothingPlaced {
            warn!(
                "Import[{}]: no {} file could be placed under the library root",
                h.source_title,
                family_str(family)
            );
            continue;
        }
        if family == base {
            own_placed = true;
        }
        files_placed += result.imported.len();
        let quality = match (family == base, result.imported.first()) {
            (false, Some(f)) => f.quality.clone(),
            _ => h.quality.clone(),
        };
        publish(
            AcquisitionEvent::Imported(Imported {
                media_id: media as u32,
                family,
                unit: result.unit,
                quality,
                source_title: h.source_title.clone(),
            }),
            "acquisition.imported",
            &h.source_title,
        );
    }
    if !own_placed {
        let msg = format!(
            "Import failed: no file could be placed under the library root for \"{}\"",
            t.name
        );
        error!("Import[{}]: {msg}", h.source_title);
        history::set_status(&[h.id], "import_failed", Some(&msg))?;
        publish_failed(h, &msg);
        return Ok(());
    }
    history::complete_import(h.id)?;
    info!(
        "Import[{}]: completed successfully ({files_placed} file(s))",
        h.source_title
    );
    Ok(())
}

// --- CleanStalled ------------------------------------------------------------------------------

/// Removes stalled downloads; their re-search goes to `search:<mediaId>` calls.
pub fn clean_stalled() -> R<()> {
    let Some(cfg) = config::stall().map_err(crate::error::Error::Other)? else {
        stall_skip(Some("off, the number of checks before cleanup is not set"));
        return Ok(());
    };
    let window = (cfg.samples + 1) as f64 * cfg.interval_minutes as f64 * 60.0;
    stalled::prune_older_than_secs(window.max(86_400.0))?;
    let enabled: Vec<DownloadClientRow> = clients::list_enabled()?
        .into_iter()
        .filter(client::supports)
        .collect();
    if enabled.is_empty() {
        stall_skip(Some("no enabled download client supports it"));
        return Ok(());
    }
    stall_skip(None);
    let mut untracked = HashSet::new();
    let mut rows = history::by_statuses(&["grabbed", "failed", "warning", "importing"])?;
    let mut research: Vec<i64> = vec![];
    let now = now_ms();
    for client in &enabled {
        let (ok, torrents) = client::torrents(client);
        if !ok {
            continue;
        }
        for t in torrents.iter().filter(|t| {
            t.progress < 1.0
                && !t.hash.is_empty()
                && client::STALL_ELIGIBLE.contains(&t.state.as_str())
        }) {
            let Some(row) = match_and_heal(t, &mut rows)? else {
                if !seen(&UNTRACKED, &t.hash) {
                    info!(
                        "StalledCleanup: \"{}\" is not a download this plugin grabbed, ignored",
                        t.name
                    );
                }
                untracked.insert(t.hash.clone());
                continue;
            };
            if !is_stalled(t, &cfg, now)? {
                continue;
            }
            warn!(
                "StalledCleanup: \"{}\" stalled (under {} KiB/s for {} checks, interval={}m)",
                t.name,
                cfg.min_bytes_per_second / 1024.0,
                cfg.samples,
                cfg.interval_minutes
            );
            if let Err(e) = client::delete(client, &t.hash, true) {
                error!("StalledCleanup: failed to delete \"{}\": {e}", t.name);
                continue;
            }
            stalled::delete_by_hash(&t.hash)?;
            auto_blocklist(&row, "Auto-blocklist: stalled torrent");
            history::mark_failed(row.id, "Stalled — removed by stalled-download cleanup")?;
            let ev = AcquisitionEvent::StalledRemoved(StalledRemoved {
                media_id: row.media_id.map(|m| m as u32),
                title: row.source_title.clone(),
            });
            if let Err(e) = host::events_publish(&[ev]) {
                warn!(
                    "StalledCleanup: failed to publish acquisition.stalled.removed: {}",
                    e.message
                );
            }
            let restart =
                cfg.auto_restart && (row.grab_source == "auto" || cfg.include_manual_grabs);
            if let (true, Some(m)) = (restart, row.media_id)
                && !research.contains(&m)
            {
                research.push(m);
            }
        }
    }
    remember(&UNTRACKED, untracked);
    if !research.is_empty() {
        info!(
            "StalledCleanup: searching for a replacement for {} media(s)",
            research.len()
        );
        for m in research {
            if let Err(e) =
                fliks_plugin_sdk::scheduled::schedule(&format!("search:{m}"), 0, &json!({}))
            {
                warn!(
                    "StalledCleanup: re-search of #{m} not scheduled: {}",
                    e.message
                );
            }
        }
    }
    Ok(())
}

fn is_stalled(t: &Torrent, cfg: &config::Stall, now: i64) -> R<bool> {
    let latest = stalled::recent(&t.hash, 1)?;
    let due = latest
        .first()
        .and_then(|l| parse_date_ms(&l.checked_at))
        .is_none_or(|at| now - at >= cfg.interval_minutes * 60_000);
    if due {
        stalled::insert(&t.hash, t.downloaded)?;
    }
    let recent = stalled::recent(&t.hash, cfg.samples)?;
    if (recent.len() as i64) < cfg.samples {
        return Ok(false);
    }
    let samples: Vec<(i64, i64)> = recent
        .iter()
        .map(|r| {
            (
                r.downloaded_bytes,
                parse_date_ms(&r.checked_at).unwrap_or(0),
            )
        })
        .collect();
    Ok(client::stalled_strikes(&samples, cfg.min_bytes_per_second) >= cfg.samples)
}

// --- CleanSeeded -------------------------------------------------------------------------------

/// Empty keeps seeding; an unknown completion time is judged on ratio alone.
pub fn seed_reason(t: &Torrent, settings: &crate::util::Settings, now_s: f64) -> String {
    let retention = settings
        .get("maxRetentionDays")
        .filter(|v| !v.is_null())
        .map(|v| js_number(Some(v)));
    let done = t.completion_on.unwrap_or(0.0);
    if let Some(days) = retention.filter(|d| *d > 0.0)
        && done > 0.0
    {
        let age = (now_s - done) / 86_400.0;
        if age >= days {
            return format!("retention {}d >= {}d", age.round(), fmt_js(days));
        }
    }
    let target = settings
        .get("seedRatio")
        .filter(|v| !v.is_null())
        .map_or(1.0, |v| js_number(Some(v)));
    if t.ratio >= target {
        return format!("ratio {:.2} >= {}", t.ratio, fmt_js(target));
    }
    String::new()
}

fn fmt_js(f: f64) -> String {
    match crate::util::num(f) {
        Value::Number(n) => n.to_string(),
        _ => f.to_string(),
    }
}

/// Whether a torrent of a book still holds finished files of another wished, missing family;
/// when that cannot be told, it does.
fn holds_unimported(row: &HistoryRow, client: &DownloadClientRow, t: &Torrent) -> bool {
    let Some(media) = row.media_id else {
        return false;
    };
    if !is_book(row, media as u32).unwrap_or(true) {
        return false;
    }
    let Some(files) = client::files(client, &t.hash) else {
        return true;
    };
    let save = t.save_path.clone().unwrap_or_default();
    [Family::Text, Family::Audio]
        .into_iter()
        .filter(|f| !paths_of(&files, &save, *f).is_empty())
        .any(|f| {
            match host::media_acquisition_context(
                media as u32,
                Some(f),
                row_language(row, f),
                None,
                None,
            ) {
                Ok(target) => target
                    .and_then(|t| t.want)
                    .is_some_and(|w| w.decision != WantDecision::Skip),
                Err(_) => true,
            }
        })
}

pub fn clean_seeded() -> R<()> {
    let enabled: Vec<DownloadClientRow> = clients::list_enabled()?
        .into_iter()
        .filter(client::supports)
        .collect();
    if enabled.is_empty() {
        return Ok(());
    }
    let mut held: Vec<(&DownloadClientRow, Torrent)> = vec![];
    for c in &enabled {
        let (ok, ts) = client::torrents(c);
        if ok {
            held.extend(ts.into_iter().map(|t| (c, t)));
        }
    }
    if held.is_empty() {
        return Ok(());
    }
    let hashes: Vec<String> = held.iter().map(|(_, t)| t.hash.to_lowercase()).collect();
    let completed = history::completed_by_hashes(&hashes)?;
    if completed.is_empty() {
        return Ok(());
    }
    let all = indexers::list_all()?;
    let now_s = now_ms() as f64 / 1000.0;
    let mut deleted = false;
    for row in &completed {
        let Some(hash) = row.torrent_hash.as_ref().map(|h| h.to_lowercase()) else {
            continue;
        };
        let Some((client, t)) = held
            .iter()
            .rev()
            .find(|(_, t)| t.hash.to_lowercase() == hash)
        else {
            continue;
        };
        let settings = row
            .indexer_id
            .filter(|i| *i != 0)
            .and_then(|id| all.iter().find(|ix| ix.id == id))
            .map(|ix| ix.settings.clone())
            .unwrap_or_default();
        let reason = seed_reason(t, &settings, now_s);
        if reason.is_empty() {
            continue;
        }
        // Retention bounds the keep; only a ratio is overridden.
        if !reason.starts_with("retention") && holds_unimported(row, client, t) {
            info!(
                "SeedCleanup: keeping \"{}\" ({reason}), it holds files of a wished family not yet imported",
                t.name
            );
            continue;
        }
        info!("SeedCleanup: removing \"{}\" ({reason})", t.name);
        match client::delete(client, &t.hash, true) {
            Ok(()) => deleted = true,
            Err(e) => error!("SeedCleanup: failed to delete \"{}\": {e}", t.name),
        }
    }
    if deleted {
        host::events_publish(&[AcquisitionEvent::QueueChanged])?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::error::Error;
    use fliks_plugin_sdk::bindings::fliks::plugin::types::ImportedFile;

    #[test]
    fn an_ingest_is_classified_by_what_it_placed() {
        let file = ImportedFile {
            media_file_id: 1,
            relative_path: "a.mkv".into(),
            quality: "HD".into(),
            edition_id: 1,
            item_ids: vec![],
        };
        let result = |imported: Vec<ImportedFile>, present: Vec<&str>| IngestResult {
            imported,
            already_present: present.into_iter().map(str::to_owned).collect(),
            unit: None,
        };
        assert_eq!(
            classify(&result(vec![file.clone()], vec![])),
            Ingested::Imported
        );
        assert_eq!(
            classify(&result(vec![file], vec!["b.mkv"])),
            Ingested::Imported
        );
        assert_eq!(
            classify(&result(vec![], vec!["b.mkv"])),
            Ingested::AlreadyPresent
        );
        assert_eq!(classify(&result(vec![], vec![])), Ingested::NothingPlaced);
    }

    #[test]
    fn only_a_host_timeout_leaves_the_outcome_unknown() {
        let host = |code: &str| {
            Error::from(fliks_plugin_sdk::host::HostError {
                code: code.into(),
                message: "m".into(),
            })
        };
        assert!(host("ERR_TIMEOUT").is_unknown_outcome());
        assert!(!host("ERR").is_unknown_outcome());
        assert!(!host("ERR_DENIED").is_unknown_outcome());
        assert!(!Error::Other("ERR_TIMEOUT".into()).is_unknown_outcome());
    }

    #[test]
    fn joins_paths_with_the_save_path_flavour() {
        assert_eq!(join_path("/downloads/", "a/b.mkv"), "/downloads/a/b.mkv");
        assert_eq!(join_path("D:\\dl", "a/b.mkv"), "D:\\dl\\a\\b.mkv");
        assert_eq!(join_path("", "b.mkv"), "b.mkv");
        assert_eq!(ext_of("dir/Show.S01E01.MKV"), ".mkv");
        assert_eq!(ext_of(".hidden"), "");
    }

    #[test]
    fn seeding_rules() {
        let t = Torrent {
            ratio: 1.5,
            completion_on: Some(1_000.0),
            ..Default::default()
        };
        let s = |v: Value| v.as_object().unwrap().clone();
        assert_eq!(seed_reason(&t, &s(json!({})), 2_000.0), "ratio 1.50 >= 1");
        assert_eq!(seed_reason(&t, &s(json!({ "seedRatio": 2 })), 2_000.0), "");
        assert_eq!(
            seed_reason(
                &t,
                &s(json!({ "seedRatio": 2, "maxRetentionDays": 1 })),
                1_000.0 + 86_400.0 * 2.0
            ),
            "retention 2d >= 1d"
        );
        let unknown = Torrent {
            completion_on: None,
            ratio: 0.1,
            ..Default::default()
        };
        assert_eq!(
            seed_reason(&unknown, &s(json!({ "maxRetentionDays": 1 })), 1e12),
            ""
        );
    }
}
