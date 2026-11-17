"""The AI's long-term memory.

SQLite (with FTS5 full-text search) holds:
  solutions  - how problems were solved: the playbooks the system learns from you
  notes      - facts about this machine and your preferences
  actions    - every command/file change, what you decided, and the result
  inbox      - things the observer noticed while you were away

Solutions and notes are mirrored to Markdown under mind/ so you can read and edit what it knows.
"""

from __future__ import annotations

import json
import re
import sqlite3
import time
from pathlib import Path

from .config import Config

SCHEMA = """
CREATE TABLE IF NOT EXISTS solutions (
    id INTEGER PRIMARY KEY,
    title TEXT NOT NULL,
    problem TEXT NOT NULL,
    solution TEXT NOT NULL,
    commands TEXT NOT NULL DEFAULT '[]',
    tags TEXT NOT NULL DEFAULT '',
    created REAL NOT NULL,
    uses INTEGER NOT NULL DEFAULT 0
);
CREATE VIRTUAL TABLE IF NOT EXISTS solutions_fts USING fts5(
    title, problem, solution, tags, content='solutions', content_rowid='id'
);
CREATE TRIGGER IF NOT EXISTS solutions_ai AFTER INSERT ON solutions BEGIN
    INSERT INTO solutions_fts(rowid, title, problem, solution, tags)
    VALUES (new.id, new.title, new.problem, new.solution, new.tags);
END;
CREATE TABLE IF NOT EXISTS notes (
    id INTEGER PRIMARY KEY,
    category TEXT NOT NULL,
    fact TEXT NOT NULL,
    created REAL NOT NULL
);
CREATE VIRTUAL TABLE IF NOT EXISTS notes_fts USING fts5(
    category, fact, content='notes', content_rowid='id'
);
CREATE TRIGGER IF NOT EXISTS notes_ai AFTER INSERT ON notes BEGIN
    INSERT INTO notes_fts(rowid, category, fact) VALUES (new.id, new.category, new.fact);
END;
CREATE TABLE IF NOT EXISTS actions (
    id INTEGER PRIMARY KEY,
    ts REAL NOT NULL,
    kind TEXT NOT NULL,
    detail TEXT NOT NULL,
    decision TEXT NOT NULL,
    exit_code INTEGER,
    output TEXT
);
CREATE TABLE IF NOT EXISTS allow_rules (
    prefix TEXT PRIMARY KEY,
    created REAL NOT NULL
);
CREATE TABLE IF NOT EXISTS inbox (
    id INTEGER PRIMARY KEY,
    ts REAL NOT NULL,
    source TEXT NOT NULL,
    key TEXT NOT NULL UNIQUE,
    message TEXT NOT NULL,
    seen INTEGER NOT NULL DEFAULT 0
);
"""


def _slug(text: str) -> str:
    return re.sub(r"[^a-z0-9]+", "-", text.lower()).strip("-")[:60] or "solution"


def fts_query(text: str) -> str:
    """Turn free text into a forgiving FTS5 OR-query."""
    words = {w for w in re.findall(r"[A-Za-z0-9_.-]{3,}", text.lower())}
    stop = {"the", "and", "for", "with", "this", "that", "how", "what", "can", "you", "fix", "please", "make"}
    words = [w for w in words if w not in stop]
    return " OR ".join(f'"{w}"' for w in sorted(words)[:24])


class Memory:
    def __init__(self, config: Config):
        self.config = config
        config.ensure()
        self.db = sqlite3.connect(config.db_path)
        self.db.row_factory = sqlite3.Row
        self.db.executescript(SCHEMA)
        self.db.commit()

    # --- solutions -------------------------------------------------------
    def add_solution(self, title: str, problem: str, solution: str,
                     commands: list[str] | None = None, tags: str = "") -> int:
        cur = self.db.execute(
            "INSERT INTO solutions(title, problem, solution, commands, tags, created) VALUES (?,?,?,?,?,?)",
            (title, problem, solution, json.dumps(commands or []), tags, time.time()),
        )
        self.db.commit()
        sid = int(cur.lastrowid)
        self._write_solution_md(sid)
        return sid

    def _write_solution_md(self, sid: int) -> Path:
        row = self.db.execute("SELECT * FROM solutions WHERE id=?", (sid,)).fetchone()
        cmds = json.loads(row["commands"])
        body = [f"# {row['title']}", "", f"tags: {row['tags']}" if row["tags"] else "",
                "", "## Problem", row["problem"], "", "## Solution", row["solution"]]
        if cmds:
            body += ["", "## Commands", "```bash", *cmds, "```"]
        path = self.config.mind_dir / "solutions" / f"{sid:04d}-{_slug(row['title'])}.md"
        path.write_text("\n".join(body).strip() + "\n")
        return path

    def search_solutions(self, query: str, limit: int = 5) -> list[sqlite3.Row]:
        q = fts_query(query)
        if not q:
            return []
        return self.db.execute(
            "SELECT s.* FROM solutions_fts f JOIN solutions s ON s.id=f.rowid "
            "WHERE solutions_fts MATCH ? ORDER BY bm25(solutions_fts) LIMIT ?", (q, limit)
        ).fetchall()

    def mark_used(self, sid: int) -> None:
        self.db.execute("UPDATE solutions SET uses=uses+1 WHERE id=?", (sid,))
        self.db.commit()

    def list_solutions(self) -> list[sqlite3.Row]:
        return self.db.execute("SELECT * FROM solutions ORDER BY id DESC").fetchall()

    # --- notes -----------------------------------------------------------
    def add_note(self, fact: str, category: str = "general") -> int:
        cur = self.db.execute("INSERT INTO notes(category, fact, created) VALUES (?,?,?)",
                              (category, fact, time.time()))
        self.db.commit()
        self._write_notes_md()
        return int(cur.lastrowid)

    def _write_notes_md(self) -> None:
        rows = self.db.execute("SELECT * FROM notes ORDER BY category, id").fetchall()
        out, cat = ["# What I know about this machine and its human", ""], None
        for r in rows:
            if r["category"] != cat:
                cat = r["category"]
                out += ["", f"## {cat}"]
            out.append(f"- {r['fact']}")
        (self.config.mind_dir / "notes.md").write_text("\n".join(out).strip() + "\n")

    def all_notes(self) -> list[sqlite3.Row]:
        return self.db.execute("SELECT * FROM notes ORDER BY category, id").fetchall()

    def search_notes(self, query: str, limit: int = 8) -> list[sqlite3.Row]:
        q = fts_query(query)
        if not q:
            return []
        return self.db.execute(
            "SELECT n.* FROM notes_fts f JOIN notes n ON n.id=f.rowid "
            "WHERE notes_fts MATCH ? ORDER BY bm25(notes_fts) LIMIT ?", (q, limit)
        ).fetchall()

    # --- action log ------------------------------------------------------
    def log_action(self, kind: str, detail: str, decision: str,
                   exit_code: int | None = None, output: str | None = None) -> None:
        self.db.execute(
            "INSERT INTO actions(ts, kind, detail, decision, exit_code, output) VALUES (?,?,?,?,?,?)",
            (time.time(), kind, detail, decision, exit_code, (output or "")[-4000:]),
        )
        self.db.commit()

    def recent_actions(self, limit: int = 30) -> list[sqlite3.Row]:
        return self.db.execute("SELECT * FROM actions ORDER BY id DESC LIMIT ?", (limit,)).fetchall()

    # --- learned trust ---------------------------------------------------
    def allow_prefixes(self) -> list[str]:
        return [r["prefix"] for r in self.db.execute("SELECT prefix FROM allow_rules")]

    def add_allow(self, prefix: str) -> None:
        self.db.execute("INSERT OR IGNORE INTO allow_rules(prefix, created) VALUES (?,?)", (prefix, time.time()))
        self.db.commit()

    # --- inbox -----------------------------------------------------------
    def post_inbox(self, source: str, key: str, message: str) -> bool:
        cur = self.db.execute("INSERT OR IGNORE INTO inbox(ts, source, key, message) VALUES (?,?,?,?)",
                              (time.time(), source, key, message))
        self.db.commit()
        return cur.rowcount > 0

    def unseen_inbox(self) -> list[sqlite3.Row]:
        return self.db.execute("SELECT * FROM inbox WHERE seen=0 ORDER BY id").fetchall()

    def mark_inbox_seen(self) -> None:
        self.db.execute("UPDATE inbox SET seen=1 WHERE seen=0")
        self.db.commit()
