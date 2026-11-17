//! Durable memory: events, journal, plans (with their execution state), rules, mandates, objects.

use rusqlite::{params, Connection, OptionalExtension};
use serde_json::{json, Value};
use std::path::Path;
use std::sync::{Mutex, MutexGuard};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};

const SCHEMA: &str = r#"
PRAGMA journal_mode=WAL;
CREATE TABLE IF NOT EXISTS events  (id INTEGER PRIMARY KEY, ts REAL NOT NULL, kind TEXT NOT NULL, data TEXT NOT NULL);
CREATE TABLE IF NOT EXISTS journal (id INTEGER PRIMARY KEY, ts REAL NOT NULL, plan_id TEXT NOT NULL, step_key TEXT NOT NULL,
                                    verb TEXT NOT NULL, args TEXT NOT NULL, explain TEXT NOT NULL, status TEXT NOT NULL,
                                    result TEXT NOT NULL DEFAULT '');
CREATE INDEX IF NOT EXISTS journal_key ON journal(step_key);
CREATE TABLE IF NOT EXISTS plans   (id TEXT PRIMARY KEY, name TEXT NOT NULL, source TEXT NOT NULL, status TEXT NOT NULL,
                                    state TEXT NOT NULL, created REAL NOT NULL, updated REAL NOT NULL);
CREATE TABLE IF NOT EXISTS rules   (name TEXT PRIMARY KEY, body TEXT NOT NULL, enabled INTEGER NOT NULL DEFAULT 1,
                                    source TEXT NOT NULL, created REAL NOT NULL);
CREATE TABLE IF NOT EXISTS mandates(id INTEGER PRIMARY KEY, verb TEXT NOT NULL, target TEXT NOT NULL DEFAULT '*',
                                    source TEXT NOT NULL, created REAL NOT NULL, UNIQUE(verb, target));
CREATE TABLE IF NOT EXISTS objects (id TEXT PRIMARY KEY, type TEXT NOT NULL, handle TEXT, fields TEXT NOT NULL,
                                    created REAL NOT NULL, updated REAL NOT NULL);
CREATE INDEX IF NOT EXISTS objects_type ON objects(type, created);
CREATE INDEX IF NOT EXISTS plans_status ON plans(status);
CREATE TABLE IF NOT EXISTS saved   (name TEXT PRIMARY KEY, body TEXT NOT NULL, created REAL NOT NULL);
CREATE TABLE IF NOT EXISTS tasks   (id TEXT PRIMARY KEY, parent TEXT, status TEXT NOT NULL, body TEXT NOT NULL,
                                    created REAL NOT NULL, updated REAL NOT NULL);
CREATE INDEX IF NOT EXISTS tasks_parent ON tasks(parent);
CREATE INDEX IF NOT EXISTS tasks_status ON tasks(status);
CREATE TABLE IF NOT EXISTS playbook_history (id INTEGER PRIMARY KEY, category TEXT NOT NULL, text TEXT NOT NULL,
                                    reason TEXT NOT NULL, ts REAL NOT NULL);
"#;

pub fn now() -> f64 {
    SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_secs_f64()).unwrap_or(0.0)
}

static COUNTER: AtomicU64 = AtomicU64::new(0);

/// Time-sortable unique id: <millis base36><counter>.
pub fn new_id(prefix: &str) -> String {
    let ms = (now() * 1000.0) as u64;
    let c = COUNTER.fetch_add(1, Ordering::Relaxed) % 1296;
    format!("{prefix}{}{:0>2}", base36(ms), base36(c))
}

fn base36(mut n: u64) -> String {
    const D: &[u8] = b"0123456789abcdefghijklmnopqrstuvwxyz";
    if n == 0 {
        return "0".into();
    }
    let mut s = Vec::new();
    while n > 0 {
        s.push(D[(n % 36) as usize]);
        n /= 36;
    }
    s.reverse();
    String::from_utf8(s).unwrap()
}

/// Thread-safe: every method takes the connection lock only for the duration of its own query, so a slow
/// action in one plan never blocks the rest of the system.
pub struct Memory {
    conn: Mutex<Connection>,
}

impl Memory {
    pub fn open(path: &Path) -> rusqlite::Result<Self> {
        if let Some(dir) = path.parent() {
            let _ = std::fs::create_dir_all(dir);
        }
        let db = Connection::open(path)?;
        db.busy_timeout(std::time::Duration::from_secs(5))?;
        db.execute_batch(SCHEMA)?;
        Ok(Memory { conn: Mutex::new(db) })
    }

    fn c(&self) -> MutexGuard<'_, Connection> {
        self.conn.lock().unwrap_or_else(|e| e.into_inner())
    }

    pub fn in_memory() -> Self {
        let db = Connection::open_in_memory().unwrap();
        db.execute_batch(SCHEMA).unwrap();
        Memory { conn: Mutex::new(db) }
    }

    // ---- events ----
    pub fn add_event(&self, kind: &str, data: &Value) -> i64 {
        let db = self.c();
        let _ = db.execute(
            "INSERT INTO events(ts, kind, data) VALUES (?1, ?2, ?3)",
            params![now(), kind, data.to_string()],
        );
        db.last_insert_rowid()
    }

    pub fn events_since(&self, since: i64, limit: i64) -> Vec<Value> {
        let db = self.c();
        let mut st = db
            .prepare("SELECT id, ts, kind, data FROM events WHERE id > ?1 ORDER BY id LIMIT ?2")
            .unwrap();
        st.query_map(params![since, limit], |r| {
            let data: String = r.get(3)?;
            Ok(json!({"id": r.get::<_, i64>(0)?, "ts": r.get::<_, f64>(1)?, "kind": r.get::<_, String>(2)?,
                      "data": serde_json::from_str::<Value>(&data).unwrap_or(Value::Null)}))
        })
        .unwrap()
        .filter_map(Result::ok)
        .collect()
    }

    pub fn last_event_id(&self) -> i64 {
        let db = self.c();
        db.query_row("SELECT COALESCE(MAX(id),0) FROM events", [], |r| r.get(0)).unwrap_or(0)
    }

    // ---- journal ----
    pub fn journal_begin(&self, plan_id: &str, key: &str, verb: &str, args: &Value, explain: &str) -> i64 {
        let db = self.c();
        let _ = db.execute(
            "INSERT INTO journal(ts, plan_id, step_key, verb, args, explain, status) VALUES (?1,?2,?3,?4,?5,?6,'started')",
            params![now(), plan_id, key, verb, args.to_string(), explain],
        );
        db.last_insert_rowid()
    }

    pub fn journal_end(&self, id: i64, status: &str, result: &str) {
        let db = self.c();
        let _ = db.execute(
            "UPDATE journal SET status=?1, result=?2 WHERE id=?3",
            params![status, result, id],
        );
    }

    /// Status of a previously started step with this idempotency key, if any.
    pub fn journal_status(&self, key: &str) -> Option<(String, String)> {
        let db = self.c();
        db
            .query_row(
                "SELECT status, result FROM journal WHERE step_key=?1 ORDER BY id DESC LIMIT 1",
                params![key],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )
            .optional()
            .ok()
            .flatten()
    }

    pub fn journal(&self, limit: i64) -> Vec<Value> {
        let db = self.c();
        let mut st = db
            .prepare("SELECT ts, plan_id, verb, explain, status, result FROM journal ORDER BY id DESC LIMIT ?1")
            .unwrap();
        st.query_map(params![limit], |r| {
            Ok(json!({"ts": r.get::<_, f64>(0)?, "plan_id": r.get::<_, String>(1)?, "verb": r.get::<_, String>(2)?,
                      "explain": r.get::<_, String>(3)?, "status": r.get::<_, String>(4)?,
                      "result": r.get::<_, String>(5)?}))
        })
        .unwrap()
        .filter_map(Result::ok)
        .collect()
    }

    // ---- plans ----
    pub fn save_plan(&self, id: &str, name: &str, source: &str, status: &str, state: &Value) {
        let db = self.c();
        let t = now();
        let _ = db.execute(
            "INSERT INTO plans(id, name, source, status, state, created, updated) VALUES (?1,?2,?3,?4,?5,?6,?6)
             ON CONFLICT(id) DO UPDATE SET status=excluded.status, state=excluded.state, updated=excluded.updated",
            params![id, name, source, status, state.to_string(), t],
        );
    }

    pub fn load_plan(&self, id: &str) -> Option<Value> {
        let db = self.c();
        db
            .query_row("SELECT state FROM plans WHERE id=?1", params![id], |r| r.get::<_, String>(0))
            .optional()
            .ok()
            .flatten()
            .and_then(|s| serde_json::from_str(&s).ok())
    }

    pub fn plans_with_status(&self, prefix: &str) -> Vec<Value> {
        let db = self.c();
        let mut st = db
            .prepare("SELECT state FROM plans WHERE status LIKE ?1 ORDER BY created")
            .unwrap();
        st.query_map(params![format!("{prefix}%")], |r| r.get::<_, String>(0))
            .unwrap()
            .filter_map(Result::ok)
            .filter_map(|s| serde_json::from_str(&s).ok())
            .collect()
    }

    // ---- rules ----
    pub fn put_rule(&self, name: &str, body: &Value, source: &str, enabled: bool) {
        let db = self.c();
        let _ = db.execute(
            "INSERT INTO rules(name, body, enabled, source, created) VALUES (?1,?2,?3,?4,?5)
             ON CONFLICT(name) DO UPDATE SET body=excluded.body, enabled=excluded.enabled",
            params![name, body.to_string(), enabled as i64, source, now()],
        );
    }

    pub fn insert_rule_if_missing(&self, name: &str, body: &Value, source: &str) {
        let db = self.c();
        let _ = db.execute(
            "INSERT OR IGNORE INTO rules(name, body, enabled, source, created) VALUES (?1,?2,1,?3,?4)",
            params![name, body.to_string(), source, now()],
        );
    }

    pub fn set_rule_enabled(&self, name: &str, enabled: bool) -> bool {
        let db = self.c();
        db
            .execute("UPDATE rules SET enabled=?1 WHERE name=?2", params![enabled as i64, name])
            .map(|n| n > 0)
            .unwrap_or(false)
    }

    pub fn rules(&self) -> Vec<(Value, bool, String)> {
        let db = self.c();
        let mut st = db.prepare("SELECT body, enabled, source FROM rules ORDER BY created").unwrap();
        st.query_map([], |r| {
            Ok((r.get::<_, String>(0)?, r.get::<_, i64>(1)? != 0, r.get::<_, String>(2)?))
        })
        .unwrap()
        .filter_map(Result::ok)
        .filter_map(|(b, e, s)| serde_json::from_str(&b).ok().map(|v| (v, e, s)))
        .collect()
    }

    // ---- mandates (trust learned from conversation) ----
    pub fn add_mandate(&self, verb: &str, target: &str, source: &str) {
        let db = self.c();
        let _ = db.execute(
            "INSERT OR IGNORE INTO mandates(verb, target, source, created) VALUES (?1,?2,?3,?4)",
            params![verb, target, source, now()],
        );
    }

    pub fn mandates(&self) -> Vec<(String, String)> {
        let db = self.c();
        let mut st = db.prepare("SELECT verb, target FROM mandates ORDER BY id").unwrap();
        st.query_map([], |r| Ok((r.get(0)?, r.get(1)?))).unwrap().filter_map(Result::ok).collect()
    }

    // ---- objects ----
    pub fn put_object(&self, typ: &str, handle: Option<&str>, fields: &Value) -> String {
        let db = self.c();
        let id = format!("obj:{typ}/{}", new_id(""));
        let t = now();
        let _ = db.execute(
            "INSERT INTO objects(id, type, handle, fields, created, updated) VALUES (?1,?2,?3,?4,?5,?5)",
            params![id, typ, handle, fields.to_string(), t],
        );
        id
    }

    /// Objects of a type, newest first. Field filtering happens in the engine.
    pub fn objects_of(&self, typ: &str, limit: i64) -> Vec<Value> {
        let db = self.c();
        let mut st = db
            .prepare("SELECT id, handle, fields, created FROM objects WHERE type=?1 ORDER BY created DESC LIMIT ?2")
            .unwrap();
        st.query_map(params![typ, limit], |r| {
            let f: String = r.get(2)?;
            let mut v = serde_json::from_str::<Value>(&f).unwrap_or(json!({}));
            if let Value::Object(m) = &mut v {
                m.insert("id".into(), Value::String(r.get(0)?));
                m.insert("type".into(), Value::String(typ.to_string()));
                if let Some(h) = r.get::<_, Option<String>>(1)? {
                    m.insert("handle".into(), Value::String(h));
                }
                m.insert("created".into(), json!(r.get::<_, f64>(3)?));
            }
            Ok(v)
        })
        .unwrap()
        .filter_map(Result::ok)
        .collect()
    }

    // ---- saved plans (reusable by name) ----
    pub fn save_named(&self, name: &str, body: &Value) {
        let db = self.c();
        let _ = db.execute(
            "INSERT INTO saved(name, body, created) VALUES (?1,?2,?3) ON CONFLICT(name) DO UPDATE SET body=excluded.body",
            params![name, body.to_string(), now()],
        );
    }

    pub fn named(&self, name: &str) -> Option<Value> {
        let db = self.c();
        db
            .query_row("SELECT body FROM saved WHERE name=?1", params![name], |r| r.get::<_, String>(0))
            .optional()
            .ok()
            .flatten()
            .and_then(|s| serde_json::from_str(&s).ok())
    }

    // ---- tasks ----
    /// Insert or replace a task (its `id`, `parent` and `status` are read from the body).
    pub fn save_task(&self, t: &Value) {
        let db = self.c();
        let now_ = now();
        let _ = db.execute(
            "INSERT INTO tasks(id, parent, status, body, created, updated) VALUES (?1,?2,?3,?4,?5,?5)
             ON CONFLICT(id) DO UPDATE SET status=excluded.status, body=excluded.body, updated=excluded.updated",
            params![t["id"].as_str().unwrap_or(""), t["parent"].as_str(), t["status"].as_str().unwrap_or("pending"),
                    t.to_string(), now_],
        );
    }

    pub fn load_task(&self, id: &str) -> Option<Value> {
        let db = self.c();
        db.query_row("SELECT body FROM tasks WHERE id=?1", params![id], |r| r.get::<_, String>(0))
            .optional().ok().flatten().and_then(|s| serde_json::from_str(&s).ok())
    }

    /// Atomically move a task from one of `from` to `to`, merging `patch` into it. False if it wasn't in
    /// one of those states (someone else got there first).
    pub fn transition_task(&self, id: &str, from: &[&str], to: &str, patch: &Value) -> Option<Value> {
        let db = self.c();
        let body: String = db.query_row("SELECT body FROM tasks WHERE id=?1", params![id], |r| r.get(0)).ok()?;
        let mut t: Value = serde_json::from_str(&body).ok()?;
        if !from.contains(&t["status"].as_str().unwrap_or("")) {
            return None;
        }
        t["status"] = json!(to);
        if let (Value::Object(m), Value::Object(p)) = (&mut t, patch) {
            for (k, v) in p {
                m.insert(k.clone(), v.clone());
            }
        }
        db.execute("UPDATE tasks SET status=?1, body=?2, updated=?3 WHERE id=?4",
                   params![to, t.to_string(), now(), id]).ok()?;
        Some(t)
    }

    pub fn task_children(&self, parent: &str) -> Vec<Value> {
        let db = self.c();
        let mut st = db.prepare("SELECT body FROM tasks WHERE parent=?1 ORDER BY created, rowid").unwrap();
        st.query_map(params![parent], |r| r.get::<_, String>(0)).unwrap()
            .filter_map(Result::ok).filter_map(|s| serde_json::from_str(&s).ok()).collect()
    }

    pub fn tasks_with_status(&self, status: &str) -> Vec<Value> {
        let db = self.c();
        let mut st = db.prepare("SELECT body FROM tasks WHERE status=?1 ORDER BY created, rowid").unwrap();
        st.query_map(params![status], |r| r.get::<_, String>(0)).unwrap()
            .filter_map(Result::ok).filter_map(|s| serde_json::from_str(&s).ok()).collect()
    }

    pub fn root_tasks(&self, limit: i64) -> Vec<Value> {
        let db = self.c();
        let mut st = db.prepare("SELECT body FROM tasks WHERE parent IS NULL ORDER BY created DESC LIMIT ?1").unwrap();
        st.query_map(params![limit], |r| r.get::<_, String>(0)).unwrap()
            .filter_map(Result::ok).filter_map(|s| serde_json::from_str(&s).ok()).collect()
    }

    // ---- playbook history (the files themselves live on disk, as markdown) ----
    pub fn playbook_archive(&self, category: &str, old_text: &str, reason: &str) {
        let db = self.c();
        let _ = db.execute("INSERT INTO playbook_history(category, text, reason, ts) VALUES (?1,?2,?3,?4)",
                           params![category, old_text, reason, now()]);
    }

    pub fn playbook_history(&self, category: &str, limit: i64) -> Vec<Value> {
        let db = self.c();
        let mut st = db.prepare("SELECT text, reason, ts FROM playbook_history WHERE category=?1 ORDER BY id DESC LIMIT ?2").unwrap();
        st.query_map(params![category, limit], |r| Ok(json!({"previous_text": r.get::<_, String>(0)?,
            "replaced_because": r.get::<_, String>(1)?, "ts": r.get::<_, f64>(2)?}))).unwrap()
            .filter_map(Result::ok).collect()
    }

    // ---- pruning: old raw history is condensed into daily digests, then removed ----
    #[cfg(test)]
    pub fn add_event_at(&self, ts: f64, kind: &str, data: &Value) {
        let db = self.c();
        let _ = db.execute("INSERT INTO events(ts, kind, data) VALUES (?1, ?2, ?3)", params![ts, kind, data.to_string()]);
    }

    /// Remove history older than the retention windows. Whole days of events become one `digest` object
    /// each (counts by kind, plus the notable moments) before they're deleted. Nothing that is in progress
    /// or waiting is ever removed.
    pub fn prune(&self, r: &Retention, dry_run: bool) -> Value {
        let db = self.c();
        let t = now();
        let day = 86400.0;
        // Whole UTC days when keeping a day or more; exact (for testing and soak runs) when keeping less.
        let raw_cutoff = t - r.events_days * day;
        let events_cutoff = if r.events_days >= 1.0 { (raw_cutoff / day).floor() * day } else { raw_cutoff };
        let plans_cutoff = t - r.plans_days * day;
        let journal_cutoff = t - r.journal_days * day;
        let tasks_cutoff = t - r.tasks_days * day;
        // Finished task trees (root finished long enough ago), including all their subtasks.
        let old_trees = "WITH RECURSIVE old(id) AS (SELECT id FROM tasks WHERE parent IS NULL AND status IN ('done','failed','cancelled') AND updated < ?1 UNION ALL SELECT t.id FROM tasks t JOIN old ON t.parent = old.id)";
        let count = |sql: &str, p: f64| -> i64 { db.query_row(sql, params![p], |row| row.get(0)).unwrap_or(0) };
        let n_events = count("SELECT COUNT(*) FROM events WHERE ts < ?1", events_cutoff);
        let n_plans = count("SELECT COUNT(*) FROM plans WHERE status IN ('done','stopped','failed') AND updated < ?1", plans_cutoff);
        let n_journal = count("SELECT COUNT(*) FROM journal WHERE ts < ?1", journal_cutoff);
        let n_tasks = count(&format!("{old_trees} SELECT COUNT(*) FROM old"), tasks_cutoff);
        let n_versions: i64 = db.query_row(
            "SELECT COUNT(*) FROM playbook_history h WHERE (SELECT COUNT(*) FROM playbook_history n
             WHERE n.category = h.category AND n.id > h.id) >= ?1", params![r.playbook_versions], |row| row.get(0)).unwrap_or(0);
        let mut digests = 0;
        if !dry_run && n_events > 0 {
            // One digest per day: how many of each kind of event, and what stood out.
            let notable = ["ai.tell", "plan.failed", "task.failed", "task.done", "service.failed", "rule.throttled",
                           "playbook.updated", "trust.granted", "battery.critical", "temperature.high"];
            let mut st = db.prepare("SELECT CAST(ts / 86400 AS INTEGER) AS d, kind, data, ts FROM events WHERE ts < ?1 ORDER BY id").unwrap();
            let rows: Vec<(i64, String, String, f64)> = st.query_map(params![events_cutoff], |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?)))
                .unwrap().filter_map(Result::ok).collect();
            drop(st);
            let mut by_day: std::collections::BTreeMap<i64, (serde_json::Map<String, Value>, Vec<Value>)> = Default::default();
            for (d, kind, data, ts) in rows {
                let e = by_day.entry(d).or_default();
                let n = e.0.get(&kind).and_then(Value::as_i64).unwrap_or(0) + 1;
                e.0.insert(kind.clone(), json!(n));
                let same_kind = e.1.iter().filter(|x| x["kind"] == json!(kind)).count();
                if notable.contains(&kind.as_str()) && same_kind < 5 && e.1.len() < 40 {
                    let data: Value = serde_json::from_str(&data).unwrap_or(Value::Null);
                    let what = data.get("text").or(data.get("title")).or(data.get("unit")).or(data.get("plan"))
                        .or(data.get("category")).cloned().unwrap_or(Value::Null);
                    e.1.push(json!({"ts": ts, "kind": kind, "what": what, "error": data.get("error")}));
                }
            }
            for (d, (counts, notable)) in by_day {
                // One digest per day: merge into it if an earlier prune already started that day's digest.
                let existing: Option<(String, String)> = db.query_row(
                    "SELECT id, fields FROM objects WHERE type='digest' AND json_extract(fields, '$.day') = ?1",
                    params![d * 86400], |row| Ok((row.get(0)?, row.get(1)?))).optional().ok().flatten();
                match existing {
                    Some((id, old)) => {
                        let mut f: Value = serde_json::from_str(&old).unwrap_or(json!({}));
                        for (k, v) in &counts {
                            let n = f["counts"][k].as_i64().unwrap_or(0) + v.as_i64().unwrap_or(0);
                            f["counts"][k] = json!(n);
                        }
                        let mut list = f["notable"].as_array().cloned().unwrap_or_default();
                        for n in notable {
                            let same = list.iter().filter(|x| x["kind"] == n["kind"]).count();
                            if same < 5 && list.len() < 40 {
                                list.push(n);
                            }
                        }
                        f["notable"] = json!(list);
                        let _ = db.execute("UPDATE objects SET fields=?1, updated=?2 WHERE id=?3", params![f.to_string(), t, id]);
                    }
                    None => {
                        let id = format!("obj:digest/{}", new_id(""));
                        let fields = json!({"day": d * 86400, "counts": counts, "notable": notable});
                        let _ = db.execute("INSERT INTO objects(id, type, handle, fields, created, updated) VALUES (?1,'digest',NULL,?2,?3,?3)",
                                           params![id, fields.to_string(), t]);
                        digests += 1;
                    }
                }
            }
            let _ = db.execute("DELETE FROM events WHERE ts < ?1", params![events_cutoff]);
        }
        if !dry_run {
            let _ = db.execute("DELETE FROM plans WHERE status IN ('done','stopped','failed') AND updated < ?1", params![plans_cutoff]);
            let _ = db.execute("DELETE FROM journal WHERE ts < ?1", params![journal_cutoff]);
            let _ = db.execute(&format!("{old_trees} DELETE FROM tasks WHERE id IN (SELECT id FROM old)"), params![tasks_cutoff]);
            let _ = db.execute("DELETE FROM playbook_history WHERE id IN (SELECT h.id FROM playbook_history h WHERE
                (SELECT COUNT(*) FROM playbook_history n WHERE n.category = h.category AND n.id > h.id) >= ?1)", params![r.playbook_versions]);
            // Give the space back when a lot was freed.
            let pages: i64 = db.query_row("PRAGMA page_count", [], |row| row.get(0)).unwrap_or(0);
            let free: i64 = db.query_row("PRAGMA freelist_count", [], |row| row.get(0)).unwrap_or(0);
            if pages > 0 && free * 4 > pages {
                let _ = db.execute_batch("VACUUM;");
            }
            let _ = db.execute_batch("PRAGMA wal_checkpoint(TRUNCATE);");
        }
        json!({"dry_run": dry_run, "events": n_events, "digests": digests, "plans": n_plans, "journal": n_journal, "tasks": n_tasks,
               "playbook_versions": n_versions})
    }

    pub fn stats(&self) -> Value {
        let db = self.c();
        let mut out = serde_json::Map::new();
        for t in ["events", "journal", "plans", "rules", "mandates", "objects", "tasks", "playbook_history"] {
            let n: i64 = db.query_row(&format!("SELECT COUNT(*) FROM {t}"), [], |row| row.get(0)).unwrap_or(-1);
            out.insert(t.into(), json!(n));
        }
        let pages: i64 = db.query_row("PRAGMA page_count", [], |row| row.get(0)).unwrap_or(0);
        let size: i64 = db.query_row("PRAGMA page_size", [], |row| row.get(0)).unwrap_or(0);
        out.insert("db_bytes".into(), json!(pages * size));
        Value::Object(out)
    }
}

/// How long raw history is kept. Settable with LINUXAI_KEEP_* environment variables.
#[derive(Clone, Debug)]
pub struct Retention {
    pub events_days: f64,
    pub plans_days: f64,
    pub journal_days: f64,
    pub tasks_days: f64,
    pub playbook_versions: i64,
}

impl Retention {
    pub fn from_env() -> Self {
        let get = |k: &str, d: f64| std::env::var(k).ok().and_then(|v| v.parse().ok()).unwrap_or(d);
        Retention {
            events_days: get("LINUXAI_KEEP_EVENTS_DAYS", 30.0),
            plans_days: get("LINUXAI_KEEP_PLANS_DAYS", 14.0),
            journal_days: get("LINUXAI_KEEP_JOURNAL_DAYS", 365.0),
            tasks_days: get("LINUXAI_KEEP_TASKS_DAYS", 180.0),
            playbook_versions: get("LINUXAI_KEEP_PLAYBOOK_VERSIONS", 50.0) as i64,
        }
    }
}
