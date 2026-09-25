use crate::error::{Code, Result, TincanError};
use crate::harness::{self, Profile};
use crate::store::now;
use rusqlite::{Connection, OptionalExtension, params};
use std::cell::OnceCell;

/// Peers bound to no PID (one-shot harnesses) go stale after this long without a call.
pub const HEARTBEAT_TTL_SECS: f64 = 15.0 * 60.0;

/// What this invocation can learn about the harness that launched it.
#[derive(Debug, Default)]
pub struct Caller {
    pub owner_pid: Option<i64>,
    pub harness: Option<Profile>,
    pub session_key: Option<String>,
}

/// Detects the caller on first use only: `--as`/TINCAN_ROLE calls never walk the process tree.
#[derive(Default)]
pub struct LazyCaller {
    cell: OnceCell<Caller>,
    session_override: Option<String>,
}

impl LazyCaller {
    /// A hook's stdin `session_id` is authoritative over env vars.
    pub fn with_session(session: Option<String>) -> Self {
        LazyCaller {
            cell: OnceCell::new(),
            session_override: session,
        }
    }

    pub fn get(&self) -> &Caller {
        self.cell.get_or_init(|| {
            let mut c = Caller::detect();
            if self.session_override.is_some() {
                c.session_key.clone_from(&self.session_override);
            }
            c
        })
    }
}

impl Caller {
    pub fn detect() -> Self {
        let (owner_pid, harness) = find_harness(&harness::load());
        // TINCAN_OWNER_PID=0 opts out of detection entirely (a plain shell inside an agent session).
        let plain = std::env::var("TINCAN_OWNER_PID").is_ok_and(|p| p == "0");
        let session_key = std::env::var("TINCAN_SESSION").ok().or_else(|| {
            harness
                .as_ref()
                .filter(|_| !plain)
                .and_then(|h| h.session_env.iter().find_map(|v| std::env::var(v).ok()))
        });
        Caller {
            owner_pid,
            harness,
            session_key,
        }
    }
}

/// Nearest harness ancestor. The process walk beats env vars because harnesses nest
/// (Claude launching Codex) and children inherit the parent's session vars.
fn find_harness(profiles: &[Profile]) -> (Option<i64>, Option<Profile>) {
    let marked = || {
        profiles
            .iter()
            .find(|p| p.marker_env.iter().any(|v| std::env::var_os(v).is_some()))
            .cloned()
    };
    // An explicit owner (wrappers, tests) skips the walk; the harness then comes from env markers.
    // `0` means "no owner": treat the caller as a plain shell.
    if let Some(pid) = std::env::var("TINCAN_OWNER_PID")
        .ok()
        .and_then(|p| p.parse().ok())
    {
        return ((pid > 0).then_some(pid), marked());
    }
    let mut pid = parent_pid();
    for _ in 0..20 {
        if pid <= 1 {
            break;
        }
        let Some((ppid, comm)) = proc_parent(pid) else {
            break;
        };
        if let Some(p) = profiles
            .iter()
            .find(|p| p.process_names.iter().any(|n| matches_harness(&comm, n)))
        {
            return (Some(pid), Some(p.clone()));
        }
        pid = ppid;
    }
    // Sandboxed harnesses (Codex seatbelt) hide the process tree: fall back to env markers.
    (None, marked())
}

/// (parent pid, executable path) straight from the kernel: no `ps` fork per ancestor.
#[cfg(target_os = "macos")]
fn proc_parent(pid: i64) -> Option<(i64, String)> {
    let mut info: libc::proc_bsdinfo = unsafe { std::mem::zeroed() };
    let size = std::mem::size_of::<libc::proc_bsdinfo>() as libc::c_int;
    let n = unsafe {
        libc::proc_pidinfo(
            pid as libc::c_int,
            libc::PROC_PIDTBSDINFO,
            0,
            (&raw mut info).cast(),
            size,
        )
    };
    if n != size {
        return None;
    }
    let mut buf = vec![0u8; libc::PROC_PIDPATHINFO_MAXSIZE as usize];
    let len = unsafe {
        libc::proc_pidpath(
            pid as libc::c_int,
            buf.as_mut_ptr().cast(),
            buf.len() as u32,
        )
    };
    let path = if len > 0 {
        String::from_utf8_lossy(&buf[..len as usize]).into_owned()
    } else {
        let comm = unsafe { std::ffi::CStr::from_ptr(info.pbi_comm.as_ptr()) };
        comm.to_string_lossy().into_owned()
    };
    Some((info.pbi_ppid as i64, path))
}

#[cfg(target_os = "linux")]
fn proc_parent(pid: i64) -> Option<(i64, String)> {
    // /proc/PID/stat is "pid (comm) state ppid ..."; comm may itself contain spaces or parens.
    let stat = std::fs::read_to_string(format!("/proc/{pid}/stat")).ok()?;
    let (head, tail) = stat.rsplit_once(')')?;
    let ppid = tail.split_whitespace().nth(1)?.parse().ok()?;
    let path = std::fs::read_link(format!("/proc/{pid}/exe"))
        .map(|p| p.to_string_lossy().into_owned())
        .unwrap_or_else(|_| head.split_once('(').map(|x| x.1).unwrap_or("").to_string());
    Some((ppid, path))
}

#[cfg(windows)]
fn proc_parent(pid: i64) -> Option<(i64, String)> {
    use windows_sys::Win32::Foundation::{CloseHandle, INVALID_HANDLE_VALUE};
    use windows_sys::Win32::System::Diagnostics::ToolHelp::{
        CreateToolhelp32Snapshot, PROCESSENTRY32W, Process32FirstW, Process32NextW,
        TH32CS_SNAPPROCESS,
    };
    let snap = unsafe { CreateToolhelp32Snapshot(TH32CS_SNAPPROCESS, 0) };
    if snap == INVALID_HANDLE_VALUE {
        return None;
    }
    let mut e: PROCESSENTRY32W = unsafe { std::mem::zeroed() };
    e.dwSize = std::mem::size_of::<PROCESSENTRY32W>() as u32;
    let mut found = None;
    let mut ok = unsafe { Process32FirstW(snap, &mut e) } != 0;
    while ok {
        if i64::from(e.th32ProcessID) == pid {
            let len = e
                .szExeFile
                .iter()
                .position(|&c| c == 0)
                .unwrap_or(e.szExeFile.len());
            let name = String::from_utf16_lossy(&e.szExeFile[..len]);
            found = Some((
                i64::from(e.th32ParentProcessID),
                image_path(pid).unwrap_or(name),
            ));
            break;
        }
        ok = unsafe { Process32NextW(snap, &mut e) } != 0;
    }
    unsafe { CloseHandle(snap) };
    found
}

/// Full executable path; the snapshot only has the file name.
#[cfg(windows)]
fn image_path(pid: i64) -> Option<String> {
    use windows_sys::Win32::Foundation::CloseHandle;
    use windows_sys::Win32::System::Threading::{
        OpenProcess, PROCESS_NAME_WIN32, PROCESS_QUERY_LIMITED_INFORMATION,
        QueryFullProcessImageNameW,
    };
    let h = unsafe { OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, 0, pid as u32) };
    if h.is_null() {
        return None;
    }
    let mut buf = [0u16; 1024];
    let mut len = buf.len() as u32;
    let ok =
        unsafe { QueryFullProcessImageNameW(h, PROCESS_NAME_WIN32, buf.as_mut_ptr(), &mut len) }
            != 0;
    unsafe { CloseHandle(h) };
    ok.then(|| String::from_utf16_lossy(&buf[..len as usize]))
}

#[cfg(unix)]
fn parent_pid() -> i64 {
    i64::from(std::os::unix::process::parent_id())
}

#[cfg(windows)]
fn parent_pid() -> i64 {
    proc_parent(i64::from(std::process::id())).map_or(0, |(ppid, _)| ppid)
}

/// A harness binary is named after the harness (`codex`, `grok-cli`), or is a versioned
/// build inside a directory named after it (`~/.local/share/claude/versions/2.1.282`).
fn matches_harness(path: &str, name: &str) -> bool {
    let path = path.to_lowercase().replace('\\', "/");
    let mut parts = path.rsplit('/');
    let base = parts.next().unwrap_or("");
    if base.starts_with(name) {
        return true;
    }
    base.starts_with(|c: char| c.is_ascii_digit()) && parts.any(|dir| dir == name)
}

#[cfg(unix)]
pub fn pid_alive(pid: i64) -> bool {
    // kill(pid, 0) probes without signalling; EPERM still means the process exists.
    let rc = unsafe { libc::kill(pid as libc::pid_t, 0) };
    rc == 0 || std::io::Error::last_os_error().raw_os_error() == Some(libc::EPERM)
}

#[cfg(windows)]
pub fn pid_alive(pid: i64) -> bool {
    use windows_sys::Win32::Foundation::{
        CloseHandle, ERROR_ACCESS_DENIED, GetLastError, STILL_ACTIVE,
    };
    use windows_sys::Win32::System::Threading::{
        GetExitCodeProcess, OpenProcess, PROCESS_QUERY_LIMITED_INFORMATION,
    };
    let Ok(pid) = u32::try_from(pid) else {
        return false;
    };
    if pid == 0 {
        return false;
    }
    let h = unsafe { OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, 0, pid) };
    if h.is_null() {
        // Access denied still means the process exists.
        return unsafe { GetLastError() } == ERROR_ACCESS_DENIED;
    }
    let mut code = 0u32;
    let ok = unsafe { GetExitCodeProcess(h, &mut code) } != 0;
    unsafe { CloseHandle(h) };
    ok && code == STILL_ACTIVE as u32
}

#[derive(Debug, Clone)]
pub struct Peer {
    pub role: String,
    pub harness: String,
    pub session_key: Option<String>,
    pub pid: Option<i64>,
    pub last_seen: f64,
    pub status: String,
    /// Wake driver spec (`tmux:%3`, `cmux:<surface>`, `cmd:<shell>`), None = hooks/wait only.
    pub wake: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PeerState {
    Active,
    Stale,
    Gone,
}

impl PeerState {
    pub fn as_str(self) -> &'static str {
        match self {
            PeerState::Active => "active",
            PeerState::Stale => "stale",
            PeerState::Gone => "gone",
        }
    }
}

impl Peer {
    /// Two liveness modes: PID-bound for interactive sessions, heartbeat TTL for one-shot peers.
    pub fn state(&self) -> PeerState {
        if self.status == "gone" {
            return PeerState::Gone;
        }
        match self.pid {
            Some(pid) if !pid_alive(pid) => PeerState::Stale,
            None if now() - self.last_seen > HEARTBEAT_TTL_SECS => PeerState::Stale,
            _ => PeerState::Active,
        }
    }
}

pub const PEER_COLS: &str = "role, harness, session_key, pid, last_seen, status, wake";

pub fn peer_from_row(r: &rusqlite::Row) -> rusqlite::Result<Peer> {
    Ok(Peer {
        role: r.get(0)?,
        harness: r.get(1)?,
        session_key: r.get(2)?,
        pid: r.get(3)?,
        last_seen: r.get(4)?,
        status: r.get(5)?,
        wake: r.get(6)?,
    })
}

pub fn get_peer(conn: &Connection, role: &str) -> Result<Option<Peer>> {
    Ok(conn
        .query_row(
            &format!("SELECT {PEER_COLS} FROM peers WHERE role = ?1"),
            [role],
            peer_from_row,
        )
        .optional()?)
}

/// Resolve who is calling. `--as`/TINCAN_ROLE is the contract; session and PID lookups are conveniences.
pub fn whoami(conn: &Connection, as_role: Option<&str>, caller: &LazyCaller) -> Result<Peer> {
    if let Some(role) = as_role
        .map(str::to_string)
        .or_else(|| std::env::var("TINCAN_ROLE").ok())
    {
        return match get_peer(conn, &role)? {
            Some(p) if p.status != "gone" => Ok(p),
            _ => Err(TincanError::new(
                Code::NotRegistered,
                format!("role {role:?} is not registered"),
            )),
        };
    }
    let caller = caller.get();
    if let Some(sk) = &caller.session_key {
        let found = conn
            .query_row(
                &format!("SELECT {PEER_COLS} FROM peers WHERE session_key = ?1 AND status = 'active' ORDER BY last_seen DESC"),
                [sk],
                peer_from_row,
            )
            .optional()?;
        if let Some(mut peer) = found {
            // A resumed session keeps its session id but gets a new PID: rebind instead of going stale.
            // Only when the old owner is dead, so an explicit --pid binding is never overwritten.
            if let (Some(old), Some(new)) = (peer.pid, caller.owner_pid)
                && old != new
                && !pid_alive(old)
                && pid_alive(new)
            {
                conn.execute(
                    "UPDATE peers SET pid = ?1 WHERE role = ?2",
                    params![new, peer.role],
                )?;
                peer.pid = Some(new);
            }
            return Ok(peer);
        }
    }
    if let Some(pid) = caller.owner_pid {
        let found = conn
            .query_row(
                &format!("SELECT {PEER_COLS} FROM peers WHERE pid = ?1 AND status = 'active' ORDER BY last_seen DESC"),
                [pid],
                peer_from_row,
            )
            .optional()?;
        if let Some(peer) = found {
            return Ok(peer);
        }
    }
    Err(TincanError::new(
        Code::NotRegistered,
        "cannot infer identity: pass --as ROLE or set TINCAN_ROLE",
    ))
}

pub fn touch(conn: &Connection, role: &str) -> Result<()> {
    conn.execute(
        "UPDATE peers SET last_seen = ?1 WHERE role = ?2",
        params![now(), role],
    )?;
    Ok(())
}

/// Role names: 1-64 of `[A-Za-z0-9._-]`, so they're safe in messages, shells and `*` broadcasts.
pub fn valid_role(role: &str) -> bool {
    !role.is_empty()
        && role.len() <= 64
        && role
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.'))
}

/// `whoami`, registering the caller first if it's an agent session that hasn't joined yet:
/// as its `--as`/TINCAN_ROLE if given, else as its harness name (`claude`, then `claude-2`, ...).
pub fn me(conn: &Connection, as_role: Option<&str>, caller: &LazyCaller) -> Result<Peer> {
    match whoami(conn, as_role, caller) {
        Err(e) if e.code == Code::NotRegistered => auto_register(conn, as_role, caller)?.ok_or(e),
        r => r,
    }
}

fn auto_register(
    conn: &Connection,
    as_role: Option<&str>,
    caller: &LazyCaller,
) -> Result<Option<Peer>> {
    let explicit = as_role
        .map(str::to_string)
        .or_else(|| std::env::var("TINCAN_ROLE").ok());
    if explicit.as_deref().is_some_and(|r| !valid_role(r)) {
        return Ok(None);
    }
    let c = caller.get();
    // Only an agent session auto-registers, and every harness puts its session id in the
    // session's env. A harness process alone isn't enough: a desktop app's run button or a
    // terminal runs under the app's process too, and must register explicitly.
    if c.session_key.is_none() {
        return Ok(None);
    }
    let harness = c.harness.as_ref().map_or("agent", |h| h.name.as_str());
    let tx = rusqlite::Transaction::new_unchecked(conn, rusqlite::TransactionBehavior::Immediate)?;
    let free = |role: &str| -> Result<bool> {
        Ok(get_peer(&tx, role)?.is_none_or(|p| p.state() != PeerState::Active))
    };
    let role = match explicit {
        Some(r) if free(&r)? => r,
        Some(_) => return Ok(None),
        None => {
            let mut found = None;
            for n in 1..=99 {
                let r = if n == 1 {
                    harness.to_string()
                } else {
                    format!("{harness}-{n}")
                };
                if free(&r)? {
                    found = Some(r);
                    break;
                }
            }
            let Some(r) = found else { return Ok(None) };
            r
        }
    };
    // Mail belongs to a session, not a role: taking over a stale role starts empty.
    tx.execute("DELETE FROM deliveries WHERE recipient = ?1", [&role])?;
    let t = now();
    tx.execute(
        "INSERT INTO peers(role, harness, session_key, pid, registered_at, last_seen, status, wake)
         VALUES (?1, ?2, ?3, ?4, ?5, ?5, 'active', NULL)
         ON CONFLICT(role) DO UPDATE SET harness = excluded.harness, session_key = excluded.session_key,
           pid = excluded.pid, registered_at = excluded.registered_at, last_seen = excluded.last_seen,
           status = 'active', wake = NULL",
        params![role, harness, c.session_key, c.owner_pid, t],
    )?;
    let peer = get_peer(&tx, &role)?;
    tx.commit()?;
    crate::store::note(format!("Registered this session as '{role}'."));
    Ok(peer)
}

#[cfg(test)]
mod tests {
    use super::matches_harness;

    #[test]
    fn harness_matched_by_name_or_versioned_dir() {
        assert!(matches_harness("/opt/homebrew/bin/codex", "codex"));
        assert!(matches_harness(
            "/Users/a/.local/share/claude/versions/2.1.282",
            "claude"
        ));
        assert!(!matches_harness("/Users/a/claude/bin/python3", "claude"));
        assert!(!matches_harness("/bin/zsh", "claude"));
    }

    #[test]
    fn harness_matched_in_windows_paths() {
        assert!(matches_harness(
            r"C:\Users\a\AppData\Roaming\npm\codex.exe",
            "codex"
        ));
        assert!(matches_harness(
            r"C:\Users\a\.local\share\claude\versions\2.1.282.exe",
            "claude"
        ));
        assert!(!matches_harness(
            r"C:\Program Files\nodejs\node.exe",
            "claude"
        ));
        assert!(!matches_harness(r"C:\Users\a\claude\python.exe", "claude"));
    }
}
