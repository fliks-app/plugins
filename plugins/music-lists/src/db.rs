//! The SQL of `lists`, `list_entries`, `list_secrets` and `oauth_states`.
use crate::model::Settings;
use crate::sync::{Known, Store};
use fliks_plugin_sdk::db::{self, Rows, Value as V};
use serde_json::Value;
use std::collections::HashMap;

pub struct Row {
    pub id: i64,
    pub name: String,
    pub implementation: String,
    pub settings: Settings,
    pub enabled: bool,
    pub owner: Option<u32>,
    pub last_sync_at: Option<String>,
    pub last_error: Option<String>,
    pub last_summary: Option<Value>,
}

type R<T> = Result<T, String>;

fn e(err: db::DbError) -> String {
    format!("{}: {}", err.code, err.message)
}

const COLUMNS: &str = r#""id", "name", "implementation", "settings"::text AS "settings", "enabled",
    "ownerId", "lastSyncAt", "lastError", "lastSummary"::text AS "lastSummary""#;

fn rows(r: &Rows) -> Vec<Row> {
    (0..r.len())
        .map(|i| Row {
            id: r.int(i, "id").unwrap_or(0),
            name: r.text(i, "name").unwrap_or_default(),
            implementation: r.text(i, "implementation").unwrap_or_default(),
            settings: r
                .text(i, "settings")
                .and_then(|t| serde_json::from_str(&t).ok())
                .unwrap_or_default(),
            enabled: matches!(r.get(i, "enabled"), Some(V::Boolean(true))),
            owner: r.int(i, "ownerId").and_then(|v| u32::try_from(v).ok()),
            last_sync_at: r.text(i, "lastSyncAt"),
            last_error: r.text(i, "lastError"),
            last_summary: r
                .text(i, "lastSummary")
                .and_then(|t| serde_json::from_str(&t).ok()),
        })
        .collect()
}

pub fn migrate(m: &[(&str, &str)]) -> R<()> {
    db::migrate(m).map(|_| ()).map_err(e)
}

pub fn list() -> R<Vec<Row>> {
    let sql = format!(r#"SELECT {COLUMNS} FROM "lists" ORDER BY "id""#);
    Ok(rows(&db::query(&sql, &[]).map_err(e)?))
}

pub fn find(id: i64) -> R<Option<Row>> {
    let sql = format!(r#"SELECT {COLUMNS} FROM "lists" WHERE "id" = $1"#);
    Ok(rows(&db::query(&sql, &[id.into()]).map_err(e)?).pop())
}

pub fn insert(
    name: &str,
    imp: &str,
    settings: &Settings,
    enabled: bool,
    owner: Option<u32>,
) -> R<i64> {
    let r = db::query(
        r#"INSERT INTO "lists" ("name", "implementation", "settings", "enabled", "ownerId")
           VALUES ($1, $2, $3::jsonb, $4, $5) RETURNING "id""#,
        &[
            name.into(),
            imp.into(),
            Value::Object(settings.clone()).to_string().into(),
            enabled.into(),
            owner.map(i64::from).into(),
        ],
    )
    .map_err(e)?;
    r.int(0, "id").ok_or_else(|| "no id".into())
}

pub fn update(
    id: i64,
    name: &str,
    settings: &Settings,
    enabled: bool,
    owner: Option<u32>,
) -> R<bool> {
    let n = db::execute(
        r#"UPDATE "lists" SET "name" = $2, "settings" = $3::jsonb, "enabled" = $4,
           "ownerId" = COALESCE($5, "ownerId") WHERE "id" = $1"#,
        &[
            id.into(),
            name.into(),
            Value::Object(settings.clone()).to_string().into(),
            enabled.into(),
            owner.map(i64::from).into(),
        ],
    )
    .map_err(e)?;
    Ok(n > 0)
}

pub fn remove(id: i64) -> R<bool> {
    Ok(db::execute(r#"DELETE FROM "lists" WHERE "id" = $1"#, &[id.into()]).map_err(e)? > 0)
}

pub fn set_result(id: i64, error: Option<&str>, summary: &Value) -> R<()> {
    db::execute(
        r#"UPDATE "lists" SET "lastSyncAt" = now(), "lastError" = $2, "lastSummary" = $3::jsonb WHERE "id" = $1"#,
        &[id.into(), error.map(str::to_owned).into(), summary.to_string().into()],
    )
    .map_err(e)?;
    Ok(())
}

pub fn secrets(id: i64) -> R<Settings> {
    let r = db::query(
        r#"SELECT "data"::text AS "data" FROM "list_secrets" WHERE "listId" = $1"#,
        &[id.into()],
    )
    .map_err(e)?;
    Ok(r.text(0, "data")
        .and_then(|t| serde_json::from_str(&t).ok())
        .unwrap_or_default())
}

pub fn set_secrets(id: i64, data: &Settings) -> R<()> {
    db::execute(
        r#"INSERT INTO "list_secrets" ("listId", "data") VALUES ($1, $2::jsonb)
           ON CONFLICT ("listId") DO UPDATE SET "data" = EXCLUDED."data""#,
        &[id.into(), Value::Object(data.clone()).to_string().into()],
    )
    .map_err(e)?;
    Ok(())
}

pub fn new_state(list: i64, state: &str) -> R<()> {
    db::execute(
        r#"DELETE FROM "oauth_states" WHERE "createdAt" < now() - interval '1 hour'"#,
        &[],
    )
    .map_err(e)?;
    db::execute(
        r#"INSERT INTO "oauth_states" ("state", "listId") VALUES ($1, $2)"#,
        &[state.into(), list.into()],
    )
    .map_err(e)?;
    Ok(())
}

/// The list a state was issued for; a state is used once and lives an hour.
pub fn take_state(state: &str) -> R<Option<i64>> {
    let r = db::query(
        r#"DELETE FROM "oauth_states" WHERE "state" = $1 AND "createdAt" > now() - interval '1 hour' RETURNING "listId""#,
        &[state.into()],
    )
    .map_err(e)?;
    Ok(r.int(0, "listId"))
}

pub struct Live;

impl Store for Live {
    fn known(&self, list: i64, actions: &str) -> R<HashMap<String, Known>> {
        let r = db::query(
            r#"SELECT "key", "itemId" FROM "list_entries"
               WHERE "listId" = $1 AND "actions" = $2
                 AND ("mediaId" IS NOT NULL OR "checkedAt" > now() - interval '7 days')"#,
            &[list.into(), actions.into()],
        )
        .map_err(e)?;
        Ok((0..r.len())
            .filter_map(|i| {
                let item = r.int(i, "itemId").and_then(|v| u32::try_from(v).ok());
                Some((r.text(i, "key")?, Known { item }))
            })
            .collect())
    }

    fn save(
        &self,
        list: i64,
        key: &str,
        actions: &str,
        media: Option<u32>,
        item: Option<u32>,
    ) -> R<()> {
        db::execute(
            r#"INSERT INTO "list_entries" ("listId", "key", "actions", "mediaId", "itemId")
               VALUES ($1, $2, $3, $4, $5)
               ON CONFLICT ("listId", "key") DO UPDATE SET "actions" = EXCLUDED."actions",
                 "mediaId" = EXCLUDED."mediaId", "itemId" = EXCLUDED."itemId", "checkedAt" = now()"#,
            &[list.into(), key.into(), actions.into(), media.map(i64::from).into(), item.map(i64::from).into()],
        )
        .map_err(e)?;
        Ok(())
    }
}
