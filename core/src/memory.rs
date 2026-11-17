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
}
