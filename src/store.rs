use crate::error::{Code, Result, TincanError};
use rusqlite::{Connection, OpenFlags, TransactionBehavior};
use std::fs::{File, OpenOptions};
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

/// Store-and-forward only: a `deliveries` row exists while a message is unread by that recipient,
/// and a message body is wiped once no deliveries remain (see `commands::sweep`).
const SCHEMA: &str = "
CREATE TABLE IF NOT EXISTS peers(
  role TEXT PRIMARY KEY, harness TEXT NOT NULL, session_key TEXT, pid INTEGER,
  registered_at REAL NOT NULL, last_seen REAL NOT NULL, status TEXT NOT NULL DEFAULT 'active',
  wake TEXT);
CREATE TABLE IF NOT EXISTS messages(
  id TEXT PRIMARY KEY,
  sender TEXT NOT NULL, client_id TEXT, kind TEXT NOT NULL, body TEXT NOT NULL,
  reply_to TEXT, hop INTEGER NOT NULL DEFAULT 0, no_reply INTEGER NOT NULL DEFAULT 0,
  created_at REAL NOT NULL, recipients TEXT NOT NULL DEFAULT '[]', UNIQUE(sender, client_id));
CREATE TABLE IF NOT EXISTS deliveries(
  message_id TEXT NOT NULL, recipient TEXT NOT NULL,
  delivered_at REAL, lease_until REAL,
  -- set once the recipient has been told about it (by a nudge or a hook), so it is told only once
  told INTEGER NOT NULL DEFAULT 0,
  PRIMARY KEY(message_id, recipient));
CREATE INDEX IF NOT EXISTS ix_deliveries_recipient ON deliveries(recipient);
CREATE INDEX IF NOT EXISTS ix_messages_created ON messages(created_at);
";

pub fn now() -> f64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_secs_f64()
}

/// Team dir precedence: --team-dir, then TINCAN_TEAM_DIR, then the nearest ancestor holding `.tincan/`,
/// then (created on first use) the git repo root, or a per-user default team outside any repo.
/// `init` alone uses the cwd instead of the repo root.
pub fn resolve_team(flag: Option<&str>, init: bool) -> Result<(PathBuf, PathBuf)> {
    let dir = match flag
        .map(PathBuf::from)
        .or_else(|| std::env::var_os("TINCAN_TEAM_DIR").map(PathBuf::from))
    {
        Some(d) => {
            std::path::absolute(d).map_err(|e| TincanError::new(Code::NoTeam, e.to_string()))?
        }
        None => {
            let cwd = std::env::current_dir().unwrap_or_default();
            match find_upwards(&cwd)? {
                Some(d) => d,
                None if init => cwd,
                None => git_root(&cwd)
                    .or_else(default_team)
                    .ok_or_else(|| TincanError::new(Code::NoTeam, "no team dir and no home dir"))?,
            }
        }
    };
    let tincan_dir = dir.join(".tincan");
    match std::fs::symlink_metadata(&tincan_dir) {
        Ok(meta) if is_plain_dir(&meta) => {
            harden_dir(&tincan_dir).map_err(|e| TincanError::new(Code::NoTeam, e.to_string()))?
        }
        Ok(_) => {
            return Err(TincanError::new(
                Code::NoTeam,
                format!(
                    "refusing team store {}: .tincan must be a real directory, not a symlink, reparse point, or file",
                    tincan_dir.display()
                ),
            ));
        }
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
            create_private_dir(&tincan_dir)
                .and_then(|_| std::fs::write(tincan_dir.join(".gitignore"), "*\n"))
                .map_err(|e| TincanError::new(Code::NoTeam, e.to_string()))?;
            note(format!(
                "Created team at {} (git-ignored).",
                tincan_dir.display()
            ));
        }
        Err(e) => return Err(TincanError::new(Code::NoTeam, e.to_string())),
    }
    // SQLite's NOFOLLOW flag rejects symlinks in any path component. Canonicalize only after
    // validating that `.tincan` itself is a real directory, so ordinary platform aliases such as
    // macOS `/var` -> `/private/var` work without allowing the store to redirect elsewhere.
    let db = std::fs::canonicalize(&tincan_dir)
        .map_err(|e| TincanError::new(Code::NoTeam, e.to_string()))?
        .join("tincan.db");
    Ok((dir, db))
}

fn find_upwards(start: &Path) -> Result<Option<PathBuf>> {
    for dir in start.ancestors() {
        let candidate = dir.join(".tincan");
        match std::fs::symlink_metadata(&candidate) {
            Ok(meta) if is_plain_dir(&meta) => return Ok(Some(dir.to_path_buf())),
            Ok(_) => {
                return Err(TincanError::new(
                    Code::NoTeam,
                    format!(
                        "refusing team store {}: .tincan must be a real directory, not a symlink, reparse point, or file",
                        candidate.display()
                    ),
                ));
            }
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
            Err(e) => return Err(TincanError::new(Code::NoTeam, e.to_string())),
        }
    }
    Ok(None)
}

/// The repo's main checkout, so sessions in any of its worktrees share one team.
fn git_root(start: &Path) -> Option<PathBuf> {
    let root = start.ancestors().find(|d| d.join(".git").exists())?;
    let dotgit = root.join(".git");
    if dotgit.is_file() {
        // A worktree: `.git` holds `gitdir: <main>/.git/worktrees/<name>`.
        let text = std::fs::read_to_string(&dotgit).ok()?;
        let gitdir = PathBuf::from(text.trim().strip_prefix("gitdir:")?.trim());
        let gitdir = if gitdir.is_absolute() {
            gitdir
        } else {
            root.join(gitdir)
        };
        if let Some(main) = gitdir
            .ancestors()
            .find(|a| a.file_name().is_some_and(|n| n == ".git"))
            .and_then(Path::parent)
        {
            return Some(main.to_path_buf());
        }
    }
    Some(root.to_path_buf())
}

/// Outside any repo: `~/.local/share/tincan/default` (Windows: `%LOCALAPPDATA%\tincan\default`).
fn default_team() -> Option<PathBuf> {
    if let Some(d) = std::env::var_os("LOCALAPPDATA").filter(|d| !d.is_empty()) {
        return Some(Path::new(&d).join("tincan").join("default"));
    }
    std::env::var_os("HOME")
        .filter(|h| !h.is_empty())
        .or_else(|| std::env::var_os("USERPROFILE"))
        .map(|h| Path::new(&h).join(".local/share/tincan/default"))
}

static NOTES: std::sync::Mutex<Vec<String>> = std::sync::Mutex::new(Vec::new());

/// Record something tincan set up on its own, so the command's output can confirm it.
pub fn note(s: String) {
    NOTES.lock().unwrap().push(s);
}

pub fn take_notes() -> Option<String> {
    let notes = std::mem::take(&mut *NOTES.lock().unwrap());
    (!notes.is_empty()).then(|| notes.join(" "))
}

/// Bump when SCHEMA changes. The store only holds in-flight mail, so an old one is rebuilt, not migrated.
const SCHEMA_VERSION: i64 = 4;

pub fn connect(db: &Path) -> Result<Connection> {
    let flags = OpenFlags::SQLITE_OPEN_READ_WRITE
        | OpenFlags::SQLITE_OPEN_CREATE
        | OpenFlags::SQLITE_OPEN_NO_MUTEX
        | OpenFlags::SQLITE_OPEN_NOFOLLOW;
    let mut conn = Connection::open_with_flags(db, flags)?;
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
    for path in [
        db.to_path_buf(),
        db.with_extension("db-wal"),
        db.with_extension("db-shm"),
    ] {
        harden_file(&path).map_err(|e| TincanError::new(Code::Store, e.to_string()))?;
    }
    Ok(conn)
}

fn user_version(conn: &Connection) -> Result<i64> {
    Ok(conn.query_row("PRAGMA user_version", [], |r| r.get(0))?)
}

/// Mail bodies live here: owner-only on Unix, and git-ignored so they never get committed.
fn create_private_dir(dir: &Path) -> std::io::Result<()> {
    let mut b = std::fs::DirBuilder::new();
    b.recursive(true);
    #[cfg(unix)]
    std::os::unix::fs::DirBuilderExt::mode(&mut b, 0o700);
    b.create(dir)
}

fn is_plain_dir(meta: &std::fs::Metadata) -> bool {
    if !meta.file_type().is_dir() || meta.file_type().is_symlink() {
        return false;
    }
    #[cfg(windows)]
    {
        use std::os::windows::fs::MetadataExt;
        const FILE_ATTRIBUTE_REPARSE_POINT: u32 = 0x400;
        if meta.file_attributes() & FILE_ATTRIBUTE_REPARSE_POINT != 0 {
            return false;
        }
    }
    true
}

fn harden_dir(dir: &Path) -> std::io::Result<()> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(dir, std::fs::Permissions::from_mode(0o700))?;
    }
    #[cfg(not(unix))]
    let _ = dir;
    Ok(())
}

fn harden_file(path: &Path) -> std::io::Result<()> {
    let meta = match std::fs::symlink_metadata(path) {
        Ok(meta) => meta,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(()),
        Err(e) => return Err(e),
    };
    if !meta.file_type().is_file() || meta.file_type().is_symlink() {
        return Err(std::io::Error::other(format!(
            "refusing non-regular private file {}",
            path.display()
        )));
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600))?;
    }
    Ok(())
}

/// Open a private, replaceable output file without following a final symlink.
pub fn create_private_file(path: &Path) -> std::io::Result<File> {
    let mut options = OpenOptions::new();
    options.write(true).create(true).truncate(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600).custom_flags(libc::O_NOFOLLOW);
    }
    #[cfg(windows)]
    if let Ok(meta) = std::fs::symlink_metadata(path) {
        use std::os::windows::fs::MetadataExt;
        const FILE_ATTRIBUTE_REPARSE_POINT: u32 = 0x400;
        if meta.file_type().is_symlink()
            || meta.file_attributes() & FILE_ATTRIBUTE_REPARSE_POINT != 0
        {
            return Err(std::io::Error::other(format!(
                "refusing reparse point {}",
                path.display()
            )));
        }
    }
    let file = options.open(path)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        file.set_permissions(std::fs::Permissions::from_mode(0o600))?;
    }
    Ok(file)
}
