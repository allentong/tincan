use crate::error::{Code, Result, TincanError};
use rusqlite::{Connection, TransactionBehavior};
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

/// Store-and-forward only: a `deliveries` row exists while a message is unread by that recipient,
/// and a message body is wiped once no deliveries remain (see `commands::sweep`).
const SCHEMA: &str = "
CREATE TABLE IF NOT EXISTS peers(
  role TEXT PRIMARY KEY, harness TEXT NOT NULL, session_key TEXT, pid INTEGER,
  registered_at REAL NOT NULL, last_seen REAL NOT NULL, status TEXT NOT NULL DEFAULT 'active',
  wake TEXT, told_seq INTEGER NOT NULL DEFAULT 0);
CREATE TABLE IF NOT EXISTS messages(
  seq INTEGER PRIMARY KEY AUTOINCREMENT, id TEXT UNIQUE NOT NULL,
  sender TEXT NOT NULL, client_id TEXT, kind TEXT NOT NULL, body TEXT NOT NULL,
  reply_to TEXT, hop INTEGER NOT NULL DEFAULT 0, no_reply INTEGER NOT NULL DEFAULT 0,
  created_at REAL NOT NULL, recipients TEXT NOT NULL DEFAULT '[]', UNIQUE(sender, client_id));
CREATE TABLE IF NOT EXISTS deliveries(
  message_id TEXT NOT NULL, recipient TEXT NOT NULL,
  delivered_at REAL, lease_until REAL,
  PRIMARY KEY(message_id, recipient));
CREATE INDEX IF NOT EXISTS ix_deliveries_recipient ON deliveries(recipient);
";

pub fn now() -> f64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_secs_f64()
}

/// Team dir precedence: --team-dir, then TINCAN_TEAM_DIR, then the nearest ancestor holding `.tincan/`.
pub fn resolve_team(flag: Option<&str>, create: bool) -> Result<(PathBuf, PathBuf)> {
    let dir = match flag
        .map(PathBuf::from)
        .or_else(|| std::env::var_os("TINCAN_TEAM_DIR").map(PathBuf::from))
    {
        Some(d) => {
            std::path::absolute(d).map_err(|e| TincanError::new(Code::NoTeam, e.to_string()))?
        }
        None => {
            let cwd = std::env::current_dir().unwrap_or_default();
            match find_upwards(&cwd) {
                Some(d) => d,
                // `init` with no team found anywhere above starts one here.
                None if create => cwd,
                None => {
                    return Err(TincanError::new(
                        Code::NoTeam,
                        "no team dir: pass --team-dir, set TINCAN_TEAM_DIR, or run `init`",
                    ));
                }
            }
        }
    };
    let tincan_dir = dir.join(".tincan");
    if !tincan_dir.is_dir() {
        if !create {
            return Err(TincanError::new(
                Code::NoTeam,
                format!(
                    "{} is not initialised (no .tincan/); run `init`",
                    dir.display()
                ),
            ));
        }
        std::fs::create_dir_all(&tincan_dir)
            .map_err(|e| TincanError::new(Code::NoTeam, e.to_string()))?;
    }
    let db = tincan_dir.join("tincan.db");
    Ok((dir, db))
}

fn find_upwards(start: &Path) -> Option<PathBuf> {
    start
        .ancestors()
        .find(|d| d.join(".tincan").is_dir())
        .map(Path::to_path_buf)
}

/// Bump when SCHEMA changes. The store only holds in-flight mail, so an old one is rebuilt, not migrated.
const SCHEMA_VERSION: i64 = 3;

pub fn connect(db: &Path) -> Result<Connection> {
    let mut conn = Connection::open(db)?;
    conn.busy_timeout(Duration::from_secs(10))?;
    conn.execute_batch("PRAGMA synchronous=NORMAL;")?;
    if user_version(&conn)? != SCHEMA_VERSION {
        conn.query_row("PRAGMA journal_mode=WAL", [], |_| Ok(()))?;
        let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
        // Re-check under the write lock: a concurrent first call may have just built it.
        if user_version(&tx)? != SCHEMA_VERSION {
            tx.execute_batch(&format!(
                "DROP TABLE IF EXISTS peers; DROP TABLE IF EXISTS messages; DROP TABLE IF EXISTS deliveries;
                 {SCHEMA}
                 PRAGMA user_version = {SCHEMA_VERSION};"
            ))?;
        }
        tx.commit()?;
    }
    Ok(conn)
}

fn user_version(conn: &Connection) -> Result<i64> {
    Ok(conn.query_row("PRAGMA user_version", [], |r| r.get(0))?)
}
