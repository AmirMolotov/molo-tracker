import hashlib
import hmac
import json
import os
import re
import secrets
import sqlite3
import threading
import time
from contextlib import closing
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer

DATABASE = os.environ.get("MOLO_DB", "sync.sqlite3")
PREFIX = os.environ.get("MOLO_PREFIX", "/tracker-sync").rstrip("/")
LIMIT = 100
AUTH_SLOTS = threading.BoundedSemaphore(2)


def connect():
    db = sqlite3.connect(DATABASE, timeout=10)
    db.execute("PRAGMA foreign_keys = ON")
    return db


def initialize():
    with closing(connect()) as db:
        db.executescript("""
            PRAGMA journal_mode = WAL;
            CREATE TABLE IF NOT EXISTS accounts (
                username TEXT PRIMARY KEY, salt BLOB NOT NULL, password BLOB NOT NULL
            );
            CREATE TABLE IF NOT EXISTS tokens (
                hash BLOB PRIMARY KEY, username TEXT NOT NULL REFERENCES accounts(username),
                device TEXT NOT NULL, expires INTEGER NOT NULL,
                UNIQUE(username, device)
            );
            CREATE TABLE IF NOT EXISTS devices (
                username TEXT NOT NULL REFERENCES accounts(username), device TEXT NOT NULL,
                name TEXT NOT NULL, PRIMARY KEY(username, device)
            );
            CREATE TABLE IF NOT EXISTS preferences (
                username TEXT NOT NULL REFERENCES accounts(username), key TEXT NOT NULL,
                value TEXT NOT NULL, PRIMARY KEY(username, key)
            );
            CREATE TABLE IF NOT EXISTS sessions (
                revision INTEGER PRIMARY KEY AUTOINCREMENT,
                username TEXT NOT NULL REFERENCES accounts(username), device TEXT NOT NULL,
                session_id INTEGER NOT NULL, end_ms INTEGER NOT NULL, payload TEXT NOT NULL,
                UNIQUE (username, device, session_id)
            );
            CREATE INDEX IF NOT EXISTS sessions_pull ON sessions(username, revision);
        """)


def password_hash(password, salt):
    return hashlib.scrypt(password.encode(), salt=salt, n=2**15, r=8, p=1, maxmem=64*1024**2)


def login(db, request, create):
    username = request.get("username", "")
    password = request.get("password", "")
    device = request.get("device", "")
    if (not isinstance(username, str) or not re.fullmatch(r"[a-z0-9][a-z0-9_.@+-]{2,127}", username)
            or not isinstance(password, str) or not 12 <= len(password) <= 256
            or not isinstance(device, str) or not re.fullmatch(r"[a-f0-9]{32}", device)):
        return 400, {"error": "Use a valid username and a password of at least 12 characters."}
    if not AUTH_SLOTS.acquire(blocking=False):
        return 429, {"error": "The server is busy. Try again shortly."}
    try:
        row = db.execute("SELECT salt, password FROM accounts WHERE username = ?", (username,)).fetchone()
        salt = row[0] if row else secrets.token_bytes(16)
        digest = password_hash(password, salt)
        if create and row:
            return 409, {"error": "That username is already taken."}
        if not create and (not row or not hmac.compare_digest(digest, row[1])):
            return 401, {"error": "Incorrect username or password."}
        token = secrets.token_urlsafe(32)
        with db:
            if create:
                try:
                    db.execute("INSERT INTO accounts VALUES (?, ?, ?)", (username, salt, digest))
                except sqlite3.IntegrityError:
                    return 409, {"error": "That username is already taken."}
            db.execute("DELETE FROM tokens WHERE expires <= ?", (int(time.time()),))
            db.execute("""INSERT INTO tokens VALUES (?, ?, ?, ?)
                ON CONFLICT(username, device) DO UPDATE SET hash = excluded.hash, expires = excluded.expires""",
                (hashlib.sha256(token.encode()).digest(), username, device, int(time.time()) + 90*86400))
        return 200, {"token": token}
    finally:
        AUTH_SLOTS.release()


def valid_session(item):
    if not isinstance(item, dict):
        return False
    integers = [item.get(key) for key in ("id", "start", "end")]
    if any(type(value) is not int or not 0 < value < 2**53 for value in integers):
        return False
    if item["start"] >= item["end"] or not isinstance(item.get("app"), str) or not 0 < len(item["app"]) <= 260:
        return False
    for field in ("pages", "domains"):
        spans = item.get(field)
        if not isinstance(spans, list) or len(spans) > 10000:
            return False
        for span in spans:
            if (not isinstance(span, dict) or not isinstance(span.get("value"), str)
                    or len(span["value"]) > 8192
                    or type(span.get("start")) is not int or type(span.get("end")) is not int
                    or not item["start"] <= span["start"] < span["end"] <= item["end"]):
                return False
    return True


def valid_preference(key, value):
    if not isinstance(key, str) or not 0 < len(key) <= 512:
        return False
    if key in ("cutoff", "idle-seconds"):
        limit = (2**32-1) // (1000 if key == "idle-seconds" else 1)
        return value is None or type(value) is int and 0 <= value <= limit
    kind, _, name = key.partition("/")
    if not name or kind not in ("group", "assignment", "alias", "idle-exception"):
        return False
    if kind == "idle-exception":
        return value is None or value is True
    return value is None or (isinstance(value, str) and 0 < len(value) <= 64
                             and all(ord(c) >= 32 and not 127 <= ord(c) <= 159 for c in value))


def merge_preferences(db, username, patch):
    rows = dict(db.execute("SELECT key, value FROM preferences WHERE username=?", (username,)).fetchall())
    prefs = {key: json.loads(value) for key, value in rows.items()}
    for change in patch["changes"]:
        key = change["key"]
        if (patch["seed"] and key not in prefs) or (not patch["seed"] and prefs.get(key) == change["expected"]):
            prefs[key] = change["value"]
    for key, value in prefs.items():
        if key.startswith("assignment/") and value is not None:
            group = "group/" + value.lower()
            if group in prefs:
                prefs[key] = prefs[group]
        serialized = json.dumps(prefs[key], ensure_ascii=False, separators=(",", ":"))
        if rows.get(key) != serialized:
            db.execute("INSERT INTO preferences VALUES (?,?,?) ON CONFLICT(username,key) DO UPDATE SET value=excluded.value",
                       (username, key, serialized))
    return prefs


def sync(db, username, device, request):
    cursor = request.get("cursor")
    uploads = request.get("sessions")
    patch = request.get("preferences")
    if (type(cursor) is not int or not 0 <= cursor < 2**53 or not isinstance(uploads, list)
            or len(uploads) > LIMIT or not all(map(valid_session, uploads))):
        return 400, {"error": "Invalid session batch."}
    if patch is not None and (not isinstance(patch, dict) or type(patch.get("seed")) is not bool
            or not isinstance(patch.get("changes"), list) or len(patch["changes"]) > 6000
            or not all(isinstance(change, dict) and "expected" in change and "value" in change
                       and valid_preference(change.get("key"), change["value"])
                       for change in patch["changes"])):
        return 400, {"error": "Invalid settings changes."}
    with db:
        db.execute("BEGIN IMMEDIATE")
        preferences = merge_preferences(db, username, patch) if patch is not None else None
        name = request.get("device_name")
        if isinstance(name, str) and 0 < len(name) <= 80 and name.isprintable() and name.strip():
            db.execute("""INSERT INTO devices VALUES (?, ?, ?)
                          ON CONFLICT(username,device) DO UPDATE SET name=excluded.name
                          WHERE devices.name<>excluded.name""", (username, device, name.strip()))
        for item in uploads:
            previous = db.execute("SELECT revision, end_ms FROM sessions WHERE username=? AND device=? AND session_id=?",
                                  (username, device, item["id"])).fetchone()
            if previous and previous[1] >= item["end"]:
                continue
            if previous:
                db.execute("DELETE FROM sessions WHERE revision=?", (previous[0],))
            item["device"] = device
            db.execute("INSERT INTO sessions(username,device,session_id,end_ms,payload) VALUES (?,?,?,?,?)",
                       (username, device, item["id"], item["end"], json.dumps(item, ensure_ascii=False, separators=(",", ":"))))
        rows = db.execute("""SELECT revision, payload FROM sessions
                WHERE username=? AND device<>? AND revision>? ORDER BY revision LIMIT ?""",
                          (username, device, cursor, LIMIT+1)).fetchall()
        names = dict(db.execute("SELECT device, name FROM devices WHERE username=? AND device<>? LIMIT 1000",
                                (username, device)).fetchall())
        sessions = []
        response_size = 128
        for revision, payload in rows[:LIMIT]:
            payload_size = len(payload.encode("utf-8"))
            if sessions and response_size + payload_size > 2_000_000:
                break
            cursor = revision
            response_size += payload_size + 1
            sessions.append(json.loads(payload))
    return 200, {"cursor": cursor, "sessions": sessions, "more": len(rows) > len(sessions),
                 "devices": names, "preferences": preferences}


class Handler(BaseHTTPRequestHandler):
    def setup(self):
        super().setup()
        self.connection.settimeout(10)

    def respond(self, status, body):
        payload = json.dumps(body, ensure_ascii=False, separators=(",", ":")).encode()
        self.send_response(status)
        self.send_header("Content-Type", "application/json")
        self.send_header("Cache-Control", "no-store")
        self.send_header("Content-Length", str(len(payload)))
        self.end_headers()
        self.wfile.write(payload)

    def do_GET(self):
        if self.path in ("/healthz", PREFIX + "/healthz"):
            with closing(connect()) as db:
                db.execute("SELECT username FROM accounts LIMIT 1").fetchone()
            self.respond(200, {"status": "ok"})
        else:
            self.respond(404, {"error": "Unknown endpoint."})

    def do_POST(self):
        try:
            size = int(self.headers.get("Content-Length", "0"))
            if not 0 < size <= 2_000_000:
                self.respond(413, {"error": "Request exceeds the allowed size."})
                return
            body = json.loads(self.rfile.read(size))
            if not isinstance(body, dict):
                raise ValueError()
        except (ValueError, UnicodeError):
            self.respond(400, {"error": "Invalid JSON request."})
            return
        try:
            with closing(connect()) as db:
                if self.path in (PREFIX + "/login", PREFIX + "/register"):
                    status, response = login(db, body, self.path.endswith("/register"))
                elif self.path in (PREFIX + "/sync", PREFIX + "/logout"):
                    bearer = self.headers.get("Authorization", "")
                    if not bearer.startswith("Bearer "):
                        self.respond(401, {"error": "Sign in to sync."})
                        return
                    digest = hashlib.sha256(bearer[7:].encode()).digest()
                    account = db.execute("SELECT username,device FROM tokens WHERE hash=? AND expires>?",
                                         (digest, int(time.time()))).fetchone()
                    if not account:
                        status, response = 401, {"error": "Sign in to sync."}
                    elif self.path.endswith("/logout"):
                        with db:
                            db.execute("DELETE FROM tokens WHERE hash=?", (digest,))
                        status, response = 200, {}
                    else:
                        status, response = sync(db, *account, body)
                else:
                    status, response = 404, {"error": "Unknown endpoint."}
            self.respond(status, response)
        except sqlite3.Error as error:
            self.log_error("Database error: %s", error)
            self.respond(503, {"error": "Sync is temporarily unavailable."})


if __name__ == "__main__":
    initialize()
    ThreadingHTTPServer(("0.0.0.0", int(os.environ.get("PORT", "8080"))), Handler).serve_forever()
