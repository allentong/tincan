//! `tincan self-update`, and the once-a-day background check behind the SessionStart notice.
//!
//! The `updates` setting (`$TINCAN_UPDATES`, else `"updates"` in `~/.config/tincan/config.json`):
//! `notify` (default) tells the user when a release is out, `auto` installs it, `off` never checks.

use crate::error::{Code, Result, TincanError};
use crate::harness::{config_file, home};
use crate::store::now;
use serde_json::{Value, json};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

const RELEASES: &str = "https://github.com/allentong/tincan/releases";
const REPO: &str = "allentong/tincan";
const CHECK_EVERY_SECS: f64 = 24.0 * 3600.0;
const CURRENT: &str = env!("CARGO_PKG_VERSION");

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Mode {
    Off,
    Notify,
    Auto,
}

impl Mode {
    pub fn name(self) -> &'static str {
        match self {
            Mode::Off => "off",
            Mode::Notify => "notify",
            Mode::Auto => "auto",
        }
    }
}

/// The configured mode, plus a config error if the setting was unreadable or unknown.
pub fn mode() -> (Mode, Option<String>) {
    let (raw, source) = match std::env::var("TINCAN_UPDATES") {
        Ok(v) if !v.is_empty() => (Some(v), "TINCAN_UPDATES".to_string()),
        _ => match config_file("config.json") {
            Some(path) => match std::fs::read_to_string(&path) {
                Ok(text) => match serde_json::from_str::<Value>(&text) {
                    Ok(v) => (
                        v.get("updates").and_then(Value::as_str).map(str::to_string),
                        path.display().to_string(),
                    ),
                    Err(e) => return (Mode::Notify, Some(format!("{}: {e}", path.display()))),
                },
                Err(_) => (None, String::new()),
            },
            None => (None, String::new()),
        },
    };
    match raw.as_deref() {
        None | Some("notify") => (Mode::Notify, None),
        Some("auto") => (Mode::Auto, None),
        Some("off") => (Mode::Off, None),
        Some(other) => (
            Mode::Notify,
            Some(format!(
                "{source}: updates {other:?} is not notify, auto or off"
            )),
        ),
    }
}

fn releases_url() -> String {
    std::env::var("TINCAN_RELEASES_URL")
        .ok()
        .filter(|u| !u.is_empty())
        .unwrap_or_else(|| RELEASES.to_string())
        .trim_end_matches('/')
        .to_string()
}

fn parse_version(s: &str) -> Option<(u64, u64, u64)> {
    let mut parts = s.trim().trim_start_matches('v').splitn(3, '.');
    let mut next = || parts.next()?.parse().ok();
    Some((next()?, next()?, next()?))
}

fn is_newer(latest: &str) -> bool {
    matches!((parse_version(latest), parse_version(CURRENT)), (Some(l), Some(c)) if l > c)
}

/// The release asset built for this platform, as named by the release workflow.
fn asset_name() -> Option<String> {
    let arch = match std::env::consts::ARCH {
        a @ ("aarch64" | "x86_64") => a,
        _ => return None,
    };
    let (os, ext) = match std::env::consts::OS {
        "macos" => ("apple-darwin", "tar.gz"),
        "linux" => ("unknown-linux-musl", "tar.gz"),
        "windows" => ("pc-windows-msvc", "zip"),
        _ => return None,
    };
    Some(format!("tincan-{arch}-{os}.{ext}"))
}

fn fail(msg: impl Into<String>) -> TincanError {
    TincanError::new(Code::Usage, msg.into())
}

fn curl(url: &str) -> Command {
    let mut cmd = Command::new("curl");
    cmd.args(["-fsSL", "--max-time", "120", "--proto", "=https,file"])
        .args(["--proto-redir", "=https", url]);
    cmd
}

fn download(url: &str, dest: &Path) -> Result<()> {
    let out = curl(url)
        .arg("-o")
        .arg(dest)
        .stdin(Stdio::null())
        .output()
        .map_err(|e| fail(format!("curl: {e}")))?;
    if !out.status.success() {
        return Err(fail(format!(
            "download {url} failed: {}",
            String::from_utf8_lossy(&out.stderr).trim()
        )));
    }
    Ok(())
}

pub fn latest_version() -> Result<String> {
    let url = format!("{}/latest/download/version.txt", releases_url());
    let out = curl(&url)
        .stdin(Stdio::null())
        .output()
        .map_err(|e| fail(format!("curl: {e}")))?;
    let text = String::from_utf8_lossy(&out.stdout).trim().to_string();
    if !out.status.success() || parse_version(&text).is_none() {
        return Err(fail(format!(
            "no release found at {url}: {}",
            String::from_utf8_lossy(&out.stderr).trim()
        )));
    }
    Ok(text.trim_start_matches('v').to_string())
}

fn sha256(path: &Path) -> Result<String> {
    let tools: &[(&str, &[&str])] = if cfg!(windows) {
        &[("certutil", &["-hashfile"])]
    } else {
        &[("sha256sum", &[]), ("shasum", &["-a", "256"])]
    };
    for (tool, args) in tools {
        let mut cmd = Command::new(tool);
        cmd.args(*args).arg(path);
        if cfg!(windows) {
            cmd.arg("SHA256");
        }
        let Ok(out) = cmd.stdin(Stdio::null()).output() else {
            continue;
        };
        if !out.status.success() {
            continue;
        }
        let text = String::from_utf8_lossy(&out.stdout).to_lowercase();
        if let Some(hash) = text
            .split_whitespace()
            .find(|w| w.len() == 64 && w.chars().all(|c| c.is_ascii_hexdigit()))
        {
            return Ok(hash.to_string());
        }
    }
    Err(fail("no sha256 tool found (sha256sum, shasum or certutil)"))
}

/// Build provenance, when it can be checked: only for the real GitHub releases, and only with
/// an authenticated `gh` (as install.sh does). A failed verification aborts the update.
fn verify_provenance(asset: &Path) -> Result<&'static str> {
    if releases_url() != RELEASES {
        return Ok("checksum");
    }
    let authed = Command::new("gh")
        .args(["auth", "status", "--hostname", "github.com"])
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .is_ok_and(|s| s.success());
    if !authed {
        return Ok("checksum");
    }
    let out = Command::new("gh")
        .args(["attestation", "verify"])
        .arg(asset)
        .args(["--repo", REPO, "--signer-workflow"])
        .arg(format!("{REPO}/.github/workflows/release.yml"))
        .stdin(Stdio::null())
        .output()
        .map_err(|e| fail(format!("gh: {e}")))?;
    if !out.status.success() {
        return Err(fail(format!(
            "build provenance check failed: {}",
            String::from_utf8_lossy(&out.stderr).trim()
        )));
    }
    Ok("checksum+provenance")
}

/// Removes the staging directory however the update ends.
struct Staging(PathBuf);

impl Drop for Staging {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

/// Download, verify and swap in `version` over the running binary.
fn install(version: &str) -> Result<Value> {
    let _lock = lock(true)?.ok_or_else(|| fail("couldn't take the update lock"))?;
    let asset = asset_name().ok_or_else(|| {
        fail(format!(
            "no release build for {}-{}; build with cargo install --git https://github.com/{REPO}",
            std::env::consts::ARCH,
            std::env::consts::OS
        ))
    })?;
    let exe = std::env::current_exe()
        .and_then(|p| p.canonicalize())
        .map_err(|e| fail(format!("can't find the running tincan: {e}")))?;
    let dir = exe
        .parent()
        .ok_or_else(|| fail("tincan binary has no parent directory"))?;
    // Another updater may have finished while this one waited for the lock.
    if let Some(on_disk) = reported_version(&exe)
        && parse_version(&on_disk) >= parse_version(version)
    {
        return Ok(json!({"path": exe, "already_installed": on_disk}));
    }
    // Stage next to the binary so the final rename stays on one filesystem.
    let staging = Staging(dir.join(format!(".tincan-update-{}", std::process::id())));
    std::fs::create_dir_all(&staging.0).map_err(|e| {
        fail(format!(
            "{}: {e} (is the binary's directory writable?)",
            dir.display()
        ))
    })?;
    let archive = staging.0.join(&asset);
    let url = format!("{}/download/v{version}/{asset}", releases_url());
    download(&url, &archive)?;
    let sums = staging.0.join(format!("{asset}.sha256"));
    download(&format!("{url}.sha256"), &sums)?;
    let expected = std::fs::read_to_string(&sums)
        .ok()
        .and_then(|t| t.split_whitespace().next().map(str::to_lowercase))
        .ok_or_else(|| fail(format!("{url}.sha256 is empty")))?;
    let actual = sha256(&archive)?;
    if actual != expected {
        return Err(fail(format!(
            "checksum mismatch for {asset}: expected {expected}, got {actual}"
        )));
    }
    let verified = verify_provenance(&archive)?;
    let tar_flags = if asset.ends_with(".zip") {
        "-xf"
    } else {
        "-xzf"
    };
    let status = Command::new("tar")
        .arg(tar_flags)
        .arg(&archive)
        .arg("-C")
        .arg(&staging.0)
        .stdin(Stdio::null())
        .status()
        .map_err(|e| fail(format!("tar: {e}")))?;
    if !status.success() {
        return Err(fail(format!("couldn't unpack {asset}")));
    }
    let new = staging.0.join(if cfg!(windows) {
        "tincan.exe"
    } else {
        "tincan"
    });
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&new, std::fs::Permissions::from_mode(0o755))
            .map_err(|e| fail(format!("{}: {e}", new.display())))?;
    }
    // The download has to be the release it claims to be before it replaces anything.
    let reported = reported_version(&new).unwrap_or_default();
    if reported != version {
        return Err(fail(format!(
            "downloaded binary reports \"tincan {reported}\", expected \"tincan {version}\""
        )));
    }
    replace(&new, &exe)?;
    // The skill ships in the binary: refresh the installed copies, as install.sh does.
    let skills = Command::new(&exe)
        .arg("install-skills")
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .is_ok_and(|s| s.success());
    Ok(json!({"path": exe, "verified": verified, "skills_refreshed": skills}))
}

/// `<bin> --version` minus the `tincan ` prefix.
fn reported_version(bin: &Path) -> Option<String> {
    let out = Command::new(bin)
        .arg("--version")
        .stdin(Stdio::null())
        .output()
        .ok()?;
    let text = String::from_utf8_lossy(&out.stdout);
    text.trim().strip_prefix("tincan ").map(str::to_string)
}

fn replace(new: &Path, exe: &Path) -> Result<()> {
    let err = |e: std::io::Error| fail(format!("replace {}: {e}", exe.display()));
    // Windows can't overwrite a running executable; self_replace moves it aside and cleans
    // up after it exits.
    #[cfg(windows)]
    {
        let _ = exe;
        self_replace::self_replace(new).map_err(err)
    }
    #[cfg(not(windows))]
    std::fs::rename(new, exe).map_err(err)
}

/// The update lock (`~/.config/tincan/update.lock`), held until dropped. `wait = false`
/// returns None when another process holds it.
fn lock(wait: bool) -> Result<Option<std::fs::File>> {
    let path = config_file("update.lock").ok_or_else(|| fail("no home dir"))?;
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir).map_err(|e| fail(format!("{}: {e}", dir.display())))?;
    }
    let file = std::fs::OpenOptions::new()
        .create(true)
        .truncate(false)
        .write(true)
        .open(&path)
        .map_err(|e| fail(format!("{}: {e}", path.display())))?;
    if wait {
        file.lock()
            .map_err(|e| fail(format!("{}: {e}", path.display())))?;
        return Ok(Some(file));
    }
    match file.try_lock() {
        Ok(()) => Ok(Some(file)),
        Err(std::fs::TryLockError::WouldBlock) => Ok(None),
        Err(std::fs::TryLockError::Error(e)) => Err(fail(format!("{}: {e}", path.display()))),
    }
}

fn state_path() -> Option<PathBuf> {
    config_file("update-check.json")
}

fn read_state() -> Value {
    state_path()
        .and_then(|p| std::fs::read_to_string(p).ok())
        .and_then(|t| serde_json::from_str(&t).ok())
        .unwrap_or(Value::Null)
}

fn write_state(state: &Value) {
    let Some(path) = state_path() else {
        return;
    };
    if let Some(dir) = path.parent() {
        let _ = std::fs::create_dir_all(dir);
    }
    // Readers never see a half-written file.
    let tmp = path.with_extension(format!("json.{}", std::process::id()));
    if std::fs::write(&tmp, state.to_string()).is_ok() && std::fs::rename(&tmp, &path).is_err() {
        let _ = std::fs::remove_file(&tmp);
    }
}

/// `tincan self-update [--check]`.
pub fn self_update(check: bool, background: bool) -> Result<Option<Value>> {
    if background {
        background_check();
        return Ok(None);
    }
    let latest = latest_version()?;
    let newer = is_newer(&latest);
    write_state(&json!({"checked_at": now(), "latest": latest}));
    let mut out =
        json!({"ok": true, "current": CURRENT, "latest": latest, "update_available": newer});
    if check || !newer {
        out["updated"] = json!(false);
        return Ok(Some(out));
    }
    let installed = install(&latest)?;
    out["updated"] = json!(true);
    out["update_available"] = json!(false);
    if let Value::Object(fields) = installed {
        out.as_object_mut().unwrap().extend(fields);
    }
    Ok(Some(out))
}

/// Runs detached from a SessionStart hook: check, and in `auto` mode install. Never prints.
fn background_check() {
    if mode().0 == Mode::Off {
        return;
    }
    let Ok(latest) = latest_version() else {
        return;
    };
    let mut state = json!({"checked_at": now(), "latest": latest});
    if mode().0 == Mode::Auto && is_newer(&latest) {
        match install(&latest) {
            Ok(_) => state["updated_from"] = json!(CURRENT),
            Err(e) => state["error"] = json!(e.message),
        }
    }
    write_state(&state);
}

fn launched() -> bool {
    std::env::var("TINCAN_LAUNCHED").is_ok_and(|v| !v.is_empty())
}

/// For SessionStart: start a background check if one is due, and say so if a release is out.
pub fn session_start_notice() -> Option<String> {
    if launched() || home().is_none() {
        return None;
    }
    let (mode, _) = mode();
    if mode == Mode::Off {
        return None;
    }
    let due = |state: &Value| {
        let checked_at = state
            .get("checked_at")
            .and_then(Value::as_f64)
            .unwrap_or(0.0);
        now() - checked_at >= CHECK_EVERY_SECS
    };
    let mut state = read_state();
    // Claim the check under the lock, so sessions starting together spawn one between them.
    // A held lock means another process is checking or installing right now.
    if due(&state)
        && let Ok(Some(_lock)) = lock(false)
    {
        state = read_state();
        if due(&state) {
            let mut claimed = if state.is_object() {
                state.clone()
            } else {
                json!({})
            };
            claimed["checked_at"] = json!(now());
            write_state(&claimed);
            spawn_background_check();
        }
    }

    let latest = state.get("latest").and_then(Value::as_str)?;
    if !is_newer(latest) {
        return None;
    }
    let why = match state.get("error").and_then(Value::as_str) {
        Some(e) if mode == Mode::Auto => format!(" (automatic update failed: {e})"),
        _ => String::new(),
    };
    Some(format!(
        "[tincan] tincan {latest} is available; this is {CURRENT}{why}. \
         Tell the user once, briefly, that `tincan self-update` installs it."
    ))
}

fn spawn_background_check() {
    let Ok(exe) = std::env::current_exe() else {
        return;
    };
    let mut cmd = Command::new(exe);
    cmd.args(["self-update", "--background"])
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null());
    #[cfg(unix)]
    std::os::unix::process::CommandExt::process_group(&mut cmd, 0);
    #[cfg(windows)]
    {
        // CREATE_NEW_PROCESS_GROUP | CREATE_NO_WINDOW
        std::os::windows::process::CommandExt::creation_flags(&mut cmd, 0x0000_0200 | 0x0800_0000);
    }
    let _ = cmd.spawn();
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn versions() {
        assert_eq!(parse_version("v1.2.3"), Some((1, 2, 3)));
        assert_eq!(parse_version("0.10.0\n"), Some((0, 10, 0)));
        assert_eq!(parse_version("1.2"), None);
        assert_eq!(parse_version("<html>"), None);
        assert!(parse_version("0.10.0") > parse_version("0.9.9"));
    }
}
