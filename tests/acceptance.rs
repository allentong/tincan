//! Black-box acceptance tests for the tincan CLI, mapped to spec AC numbers.

use serde_json::{Value, json};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::thread;
use std::time::{Duration, Instant};

const BIN: &str = env!("CARGO_BIN_EXE_tincan");
const STRIPPED: [&str; 7] = ["CLAUDE", "CODEX", "TINCAN", "GROK", "CMUX", "TMUX", "HERDR"];

#[derive(Default, Clone, Copy)]
struct Opts<'a> {
    env: &'a [(&'a str, &'a str)],
    stdin: Option<&'a str>,
    cwd: Option<&'a Path>,
}

/// The inherited environment minus anything that would make tincan detect a real harness or terminal.
fn clean_command() -> Command {
    let mut cmd = Command::new(BIN);
    cmd.env_clear();
    for (k, v) in std::env::vars_os() {
        let key = k.to_string_lossy();
        if !STRIPPED.iter().any(|p| key.starts_with(p)) {
            cmd.env(&k, v);
        }
    }
    cmd
}

/// Returns (exit code, parsed JSON of the first stdout line or Null).
fn exec(team: Option<&Path>, args: &[&str], o: Opts) -> (i32, Value) {
    let mut cmd = clean_command();
    if let Some(t) = team {
        cmd.arg("--team-dir").arg(t);
    }
    cmd.args(args);
    for (k, v) in o.env {
        cmd.env(k, v);
    }
    if let Some(d) = o.cwd {
        cmd.current_dir(d);
    }
    cmd.stdin(if o.stdin.is_some() {
        Stdio::piped()
    } else {
        Stdio::null()
    })
    .stdout(Stdio::piped())
    .stderr(Stdio::piped());
    let mut child = cmd.spawn().expect("spawn tincan");
    if let Some(input) = o.stdin {
        use std::io::Write;
        let mut pipe = child.stdin.take().unwrap();
        let _ = pipe.write_all(input.as_bytes());
    }
    let out = child.wait_with_output().expect("wait tincan");
    let stdout = String::from_utf8_lossy(&out.stdout);
    let json = match stdout.lines().next() {
        Some(line) if !stdout.trim().is_empty() => {
            serde_json::from_str(line).unwrap_or_else(|e| panic!("bad JSON {line:?}: {e}"))
        }
        _ => Value::Null,
    };
    (out.status.code().unwrap_or(-1), json)
}

/// A process that just stays alive for ten minutes.
#[cfg(unix)]
fn sleeper() -> Command {
    let mut c = Command::new("sleep");
    c.arg("600");
    c
}

#[cfg(windows)]
fn sleeper() -> Command {
    let mut c = Command::new("ping");
    c.args(["-n", "600", "127.0.0.1"]).stdout(Stdio::null());
    c
}

fn unique_dir(prefix: &str) -> PathBuf {
    static N: AtomicUsize = AtomicUsize::new(0);
    let d = std::env::temp_dir().join(format!(
        "{prefix}-{}-{}",
        std::process::id(),
        N.fetch_add(1, Ordering::Relaxed)
    ));
    let _ = std::fs::remove_dir_all(&d);
    std::fs::create_dir_all(&d).unwrap();
    d
}

/// A scratch dir with no team in it or above it.
struct TmpDir(PathBuf);

impl TmpDir {
    fn new() -> Self {
        TmpDir(unique_dir("tincan-empty"))
    }
}

impl Drop for TmpDir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

struct Team {
    dir: PathBuf,
    procs: Vec<Child>,
}

impl Team {
    fn new() -> Self {
        let t = Team {
            dir: unique_dir("tincan-test"),
            procs: vec![],
        };
        assert_eq!(t.run(&["init"]).0, 0);
        t
    }

    fn with(&self, args: &[&str], o: Opts) -> (i32, Value) {
        exec(Some(&self.dir), args, o)
    }

    fn run(&self, args: &[&str]) -> (i32, Value) {
        self.with(args, Opts::default())
    }

    fn env(&self, args: &[&str], env: &[(&str, &str)]) -> (i32, Value) {
        self.with(
            args,
            Opts {
                env,
                ..Opts::default()
            },
        )
    }

    fn out(&self, args: &[&str]) -> Value {
        self.run(args).1
    }

    /// Stand-in for a live harness process that a role's liveness binds to.
    fn owner(&mut self) -> u32 {
        let p = sleeper().spawn().unwrap();
        let pid = p.id();
        self.procs.push(p);
        pid
    }

    fn kill(&mut self, pid: u32) {
        if let Some(p) = self.procs.iter_mut().find(|p| p.id() == pid) {
            let _ = p.kill();
            let _ = p.wait();
        }
    }

    fn kill_all(&mut self) {
        for p in &mut self.procs {
            let _ = p.kill();
            let _ = p.wait();
        }
    }

    fn reg(&mut self, role: &str, harness: &str) -> u32 {
        let pid = self.owner();
        let (rc, o) = self.run(&[
            "register",
            role,
            "--harness",
            harness,
            "--pid",
            &pid.to_string(),
        ]);
        assert_eq!(rc, 0, "{o}");
        pid
    }

    fn path(&self, name: &str) -> PathBuf {
        self.dir.join(name)
    }

    fn db(&self) -> rusqlite::Connection {
        rusqlite::Connection::open(self.dir.join(".tincan/tincan.db")).unwrap()
    }

    fn db_strings(&self, sql: &str) -> Vec<String> {
        let c = self.db();
        let mut st = c.prepare(sql).unwrap();
        st.query_map([], |r| r.get(0))
            .unwrap()
            .map(Result::unwrap)
            .collect()
    }

    fn db_count(&self, sql: &str) -> i64 {
        self.db().query_row(sql, [], |r| r.get(0)).unwrap()
    }

    fn write(&self, name: &str, text: &str) -> String {
        let p = self.path(name);
        std::fs::write(&p, text).unwrap();
        p.to_string_lossy().into_owned()
    }
}

impl Drop for Team {
    fn drop(&mut self) {
        self.kill_all();
        let _ = std::fs::remove_dir_all(&self.dir);
    }
}

fn msgs(v: &Value) -> &Vec<Value> {
    v["messages"].as_array().expect("messages array")
}

fn bodies(v: &Value) -> Vec<&str> {
    msgs(v)
        .iter()
        .map(|m| m["body"].as_str().unwrap())
        .collect()
}

fn ids(v: &Value) -> Vec<&str> {
    msgs(v).iter().map(|m| m["id"].as_str().unwrap()).collect()
}

fn strs(v: &Value) -> Vec<String> {
    let mut out: Vec<String> = v
        .as_array()
        .unwrap()
        .iter()
        .map(|s| s.as_str().unwrap().to_string())
        .collect();
    out.sort();
    out
}

fn roles(v: &Value) -> Vec<String> {
    v["peers"]
        .as_array()
        .unwrap()
        .iter()
        .map(|p| p["role"].as_str().unwrap().to_string())
        .collect()
}

fn id(v: &Value) -> String {
    v["id"].as_str().unwrap().to_string()
}

// ---- acceptance ----

#[test]
fn ac1_ac2_dm_both_directions() {
    let mut t = Team::new();
    t.reg("claude", "claude");
    t.reg("grok", "grok");
    let (rc, o) = t.run(&["--as", "claude", "send", "grok", "please review PR 8230"]);
    assert_eq!(rc, 0, "{o}");
    let o = t.out(&["--as", "grok", "inbox"]);
    assert_eq!(bodies(&o), ["please review PR 8230"]);
    let mid = id(&msgs(&o)[0]);
    let (rc, o) = t.run(&["--as", "grok", "send", "claude", "LGTM", "--reply-to", &mid]);
    assert_eq!((rc, o["hop"].as_i64()), (0, Some(1)));
    let o = t.out(&["--as", "claude", "inbox"]);
    assert_eq!(msgs(&o)[0]["reply_to"], mid.as_str());
    // read is sticky: second inbox is empty
    assert!(msgs(&t.out(&["--as", "claude", "inbox"])).is_empty());
}

#[test]
fn ac3_stale_peer_does_not_black_hole() {
    let mut t = Team::new();
    t.reg("claude", "claude");
    let g = t.reg("grok", "grok");
    t.kill(g);
    let all = t.out(&["peers", "--all"]);
    let mut states: Vec<(String, String)> = all["peers"]
        .as_array()
        .unwrap()
        .iter()
        .map(|p| {
            (
                p["role"].as_str().unwrap().into(),
                p["state"].as_str().unwrap().into(),
            )
        })
        .collect();
    states.sort();
    assert_eq!(
        states,
        [
            ("claude".into(), "active".into()),
            ("grok".into(), "stale".into())
        ]
    );
    assert_eq!(roles(&t.out(&["peers"])), ["claude"]);
    let (rc, o) = t.run(&["--as", "claude", "send", "grok", "hello?"]);
    assert_eq!(
        (rc, o["error"].as_str(), o["state"].as_str()),
        (3, Some("peer_unavailable"), Some("stale"))
    );
}

#[test]
fn ac3_unregister_marks_gone() {
    let mut t = Team::new();
    t.reg("claude", "claude");
    t.reg("codex", "codex");
    assert_eq!(t.run(&["--as", "codex", "unregister"]).0, 0);
    let (rc, o) = t.run(&["--as", "claude", "send", "codex", "x"]);
    assert_eq!((rc, o["state"].as_str()), (3, Some("gone")));
}

#[test]
fn ac4_concurrent_sends_consistent() {
    let mut t = Team::new();
    t.reg("claude", "claude");
    t.reg("grok", "grok");
    const N: usize = 60;
    const WORKERS: usize = 16;
    let codes: Vec<i32> = thread::scope(|s| {
        let t = &t;
        let handles: Vec<_> = (0..WORKERS)
            .map(|w| {
                s.spawn(move || {
                    (w..N)
                        .step_by(WORKERS)
                        .map(|i| {
                            let (a, b) = if i % 2 == 1 {
                                ("claude", "grok")
                            } else {
                                ("grok", "claude")
                            };
                            let body = format!("{a}-{i}");
                            let cid = format!("c{i}");
                            t.run(&["--as", a, "send", b, &body, "--client-id", &cid]).0
                        })
                        .collect::<Vec<_>>()
                })
            })
            .collect();
        handles
            .into_iter()
            .flat_map(|h| h.join().unwrap())
            .collect()
    });
    assert_eq!(codes, vec![0; N]);
    // pending mail survives the senders exiting; each side gets exactly its half, no dupes
    t.kill_all();
    let g = t.out(&["--as", "grok", "inbox"]);
    let c = t.out(&["--as", "claude", "inbox"]);
    assert_eq!((msgs(&g).len(), msgs(&c).len()), (N / 2, N / 2));
    let mut all: Vec<&str> = ids(&g).into_iter().chain(ids(&c)).collect();
    all.sort();
    all.dedup();
    assert_eq!(all.len(), N);
}

#[test]
fn ac6_idempotent_resend() {
    let mut t = Team::new();
    t.reg("claude", "claude");
    t.reg("grok", "grok");
    let a = t.out(&["--as", "claude", "send", "grok", "hi", "--client-id", "abc"]);
    let b = t.out(&["--as", "claude", "send", "grok", "hi", "--client-id", "abc"]);
    assert_eq!(a["id"], b["id"]);
    assert_eq!(b["duplicate"], true);
    assert_eq!(msgs(&t.out(&["--as", "grok", "inbox"])).len(), 1);
}

#[test]
fn ac8_any_pair_and_broadcast() {
    let mut t = Team::new();
    t.reg("claude", "claude");
    t.reg("grok", "grok");
    t.reg("codex", "codex");
    assert_eq!(t.run(&["--as", "codex", "send", "grok", "c->g"]).0, 0);
    let o = t.out(&["--as", "grok", "send", "*", "all hands"]);
    assert_eq!(strs(&o["recipients"]), ["claude", "codex"]);
    assert_eq!(bodies(&t.out(&["--as", "grok", "inbox"])), ["c->g"]);
    assert_eq!(t.out(&["--as", "codex", "inbox", "--count"])["unread"], 1);
    assert_eq!(
        msgs(&t.out(&["--as", "claude", "inbox"]))[0]["kind"],
        "broadcast"
    );
}

// ---- edge cases ----

#[test]
fn role_collision_rejected() {
    let mut t = Team::new();
    t.reg("claude", "claude");
    let p = t.owner();
    let (rc, o) = t.run(&["register", "claude", "--pid", &p.to_string()]);
    assert_eq!((rc, o["error"].as_str()), (5, Some("role_taken")));
}

#[test]
fn no_team_dir_fails_loud() {
    let d = TmpDir::new();
    let (rc, o) = exec(
        None,
        &["peers"],
        Opts {
            cwd: Some(&d.0),
            ..Opts::default()
        },
    );
    assert_eq!((rc, o["error"].as_str()), (2, Some("no_team")));
}

#[test]
fn team_dir_discovered_from_subdir() {
    let mut t = Team::new();
    t.reg("claude", "claude");
    let sub = t.path("a/b");
    std::fs::create_dir_all(&sub).unwrap();
    let (_, o) = exec(
        None,
        &["peers"],
        Opts {
            cwd: Some(&sub),
            ..Opts::default()
        },
    );
    assert_eq!(roles(&o), ["claude"]);
}

#[test]
fn large_payload_rejected() {
    let mut t = Team::new();
    t.reg("claude", "claude");
    t.reg("grok", "grok");
    let big = "x".repeat(9000);
    let (rc, o) = t.with(
        &["--as", "claude", "send", "grok", "-"],
        Opts {
            stdin: Some(&big),
            ..Opts::default()
        },
    );
    assert_eq!((rc, o["error"].as_str()), (4, Some("too_large")));
}

#[test]
fn send_to_self_rejected_with_sender_hint() {
    let mut t = Team::new();
    t.reg("a", "claude");
    t.reg("b", "codex");
    let ask = id(&t.out(&["--as", "a", "send", "b", "6*7?"]));
    let (rc, o) = t.run(&["--as", "b", "send", "b", "42", "--reply-to", &ask]);
    assert_eq!((rc, o["error"].as_str()), (2, Some("usage")), "{o}");
    assert!(o["message"].as_str().unwrap().contains("send to a"), "{o}");
}

#[test]
fn reply_loop_capped() {
    let mut t = Team::new();
    t.reg("a", "claude");
    t.reg("b", "codex");
    let mut mid = id(&t.out(&["--as", "a", "send", "b", "0"]));
    for i in 0..8 {
        let (s, r) = if i % 2 == 0 { ("b", "a") } else { ("a", "b") };
        let (rc, o) = t.run(&["--as", s, "send", r, &i.to_string(), "--reply-to", &mid]);
        assert_eq!(rc, 0, "{o}");
        mid = id(&o);
    }
    let (rc, o) = t.run(&["--as", "a", "send", "b", "loop", "--reply-to", &mid]);
    assert_eq!((rc, o["error"].as_str()), (7, Some("hop_limit")));
}

#[test]
fn identity_inferred_from_session_env() {
    let mut t = Team::new();
    let p = t.owner();
    // CLAUDECODE marks the harness when no claude process is an ancestor (CI).
    let env = [("CLAUDE_CODE_SESSION_ID", "sess-1"), ("CLAUDECODE", "1")];
    t.env(&["register", "claude", "--pid", &p.to_string()], &env);
    let (rc, o) = t.env(&["whoami"], &env);
    assert_eq!((rc, o["role"].as_str()), (0, Some("claude")));
    let (rc, o) = t.run(&["whoami"]);
    assert_eq!((rc, o["error"].as_str()), (6, Some("not_registered")));
}

#[test]
fn peek_keeps_unread() {
    let mut t = Team::new();
    t.reg("claude", "claude");
    t.reg("grok", "grok");
    t.run(&["--as", "claude", "send", "grok", "hi"]);
    assert_eq!(msgs(&t.out(&["--as", "grok", "inbox", "--peek"])).len(), 1);
    assert_eq!(t.out(&["--as", "grok", "whoami"])["unread"], 1);
}

#[test]
fn hook_emits_context_only_when_unread() {
    let mut t = Team::new();
    t.reg("claude", "claude");
    t.reg("grok", "grok");
    assert!(t.out(&["--as", "claude", "hook"]).is_null());
    t.run(&["--as", "grok", "send", "claude", "ping"]);
    let o = t.out(&["--as", "claude", "hook"]);
    assert!(
        o["hookSpecificOutput"]["additionalContext"]
            .as_str()
            .unwrap()
            .contains("1 unread")
    );
    // hook never breaks the turn when misconfigured
    let d = TmpDir::new();
    let (rc, o) = exec(
        None,
        &["hook"],
        Opts {
            cwd: Some(&d.0),
            ..Opts::default()
        },
    );
    assert_eq!((rc, o), (0, Value::Null));
}

#[test]
fn wait_wakes_on_message() {
    let mut t = Team::new();
    t.reg("claude", "claude");
    t.reg("grok", "grok");
    let mut cmd = clean_command();
    cmd.arg("--team-dir")
        .arg(&t.dir)
        .args(["--as", "claude", "wait", "--timeout", "20"])
        .stdin(Stdio::null())
        .stdout(Stdio::piped());
    let w = cmd.spawn().unwrap();
    thread::sleep(Duration::from_secs(1));
    let t0 = Instant::now();
    t.run(&["--as", "grok", "send", "claude", "wake"]);
    let out = w.wait_with_output().unwrap();
    let o: Value = serde_json::from_slice(&out.stdout).unwrap();
    assert_eq!(
        (o["unread"].as_i64(), o["timed_out"].as_bool()),
        (Some(1), Some(false))
    );
    assert!(t0.elapsed() < Duration::from_secs(2));
}

// ---- v2 behaviour ----

#[test]
fn require_ack_redelivers_after_lease() {
    let mut t = Team::new();
    t.reg("claude", "claude");
    t.reg("grok", "grok");
    let mid = id(&t.out(&["--as", "claude", "send", "grok", "important"]));
    let env = [("TINCAN_LEASE_SECS", "1")];
    let got = t.env(&["--as", "grok", "inbox", "--require-ack"], &env).1;
    assert_eq!(ids(&got), [mid.as_str()]);
    // leased: hidden from a second reader, still unread
    assert!(msgs(&t.env(&["--as", "grok", "inbox"], &env).1).is_empty());
    assert_eq!(t.out(&["--as", "grok", "whoami"])["unread"], 1);
    thread::sleep(Duration::from_millis(1300));
    let again = t.env(&["--as", "grok", "inbox", "--require-ack"], &env).1;
    assert_eq!(ids(&again), [mid.as_str()]);
    assert_eq!(t.out(&["--as", "grok", "ack", &mid])["acked"], 1);
    thread::sleep(Duration::from_millis(1300));
    assert!(msgs(&t.env(&["--as", "grok", "inbox"], &env).1).is_empty());
    assert_eq!(t.out(&["--as", "grok", "whoami"])["unread"], 0);
}

#[test]
fn read_message_is_erased() {
    let mut t = Team::new();
    t.reg("claude", "claude");
    t.reg("grok", "grok");
    t.run(&["--as", "claude", "send", "grok", "secret diff"]);
    t.run(&["--as", "grok", "inbox"]);
    assert_eq!(t.db_count("SELECT COUNT(*) FROM deliveries"), 0);
    assert_eq!(t.db_strings("SELECT body FROM messages"), [""]);
}

#[test]
fn broadcast_body_kept_until_last_reader() {
    let mut t = Team::new();
    t.reg("a", "claude");
    t.reg("b", "codex");
    t.reg("c", "grok");
    t.run(&["--as", "a", "send", "*", "all hands"]);
    t.run(&["--as", "b", "inbox"]);
    assert_eq!(t.db_strings("SELECT body FROM messages"), ["all hands"]);
    t.run(&["--as", "c", "inbox"]);
    assert_eq!(t.db_strings("SELECT body FROM messages"), [""]);
}

#[test]
fn dedupe_survives_read() {
    let mut t = Team::new();
    t.reg("claude", "claude");
    t.reg("grok", "grok");
    let a = t.out(&["--as", "claude", "send", "grok", "hi", "--client-id", "k1"]);
    t.run(&["--as", "grok", "inbox"]);
    let b = t.out(&["--as", "claude", "send", "grok", "hi", "--client-id", "k1"]);
    assert_eq!((&b["id"], &b["duplicate"]), (&a["id"], &json!(true)));
    assert!(msgs(&t.out(&["--as", "grok", "inbox"])).is_empty());
}

#[test]
fn stub_expires_after_window() {
    let mut t = Team::new();
    t.reg("claude", "claude");
    t.reg("grok", "grok");
    let mid = id(&t.out(&["--as", "claude", "send", "grok", "x"]));
    t.run(&["--as", "grok", "inbox"]);
    // the next send sweeps; with a zero window the wiped stub is gone
    t.env(
        &["--as", "claude", "send", "grok", "y"],
        &[("TINCAN_STUB_SECS", "0")],
    );
    let (rc, o) = t.run(&["--as", "grok", "send", "claude", "late", "--reply-to", &mid]);
    assert_eq!((rc, o["error"].as_str()), (2, Some("usage")));
}

#[test]
fn new_session_does_not_inherit_mail() {
    let mut t = Team::new();
    t.reg("claude", "claude");
    let g = t.reg("grok", "grok");
    t.run(&["--as", "claude", "send", "grok", "for the old session"]);
    t.kill(g);
    let p = t.owner();
    let (rc, o) = t.run(&["register", "grok", "--pid", &p.to_string()]);
    assert_eq!(
        (rc, o["reclaimed"].as_bool(), o["dropped_pending"].as_i64()),
        (0, Some(true), Some(1))
    );
    assert!(msgs(&t.out(&["--as", "grok", "inbox"])).is_empty());
}

#[test]
fn unregister_drops_pending() {
    let mut t = Team::new();
    t.reg("claude", "claude");
    t.reg("grok", "grok");
    t.run(&["--as", "claude", "send", "grok", "never read"]);
    assert_eq!(t.out(&["--as", "grok", "unregister"])["dropped_pending"], 1);
    assert_eq!(t.db_strings("SELECT body FROM messages"), [""]);
}

#[test]
fn sweep_removes_ended_peers() {
    let mut t = Team::new();
    t.reg("claude", "claude");
    let g = t.reg("grok", "grok");
    t.run(&["--as", "claude", "send", "grok", "x"]);
    t.kill(g);
    assert_eq!(roles(&t.out(&["peers", "--all"])).len(), 2);
    // any register/send sweeps; zero grace removes the dead peer and its pending mail now
    t.reg("codex", "codex");
    assert_eq!(roles(&t.out(&["peers", "--all"])).len(), 3);
    t.env(
        &["--as", "claude", "send", "codex", "y"],
        &[("TINCAN_PEER_GRACE_SECS", "0")],
    );
    let mut left = roles(&t.out(&["peers", "--all"]));
    left.sort();
    assert_eq!(left, ["claude", "codex"]);
    assert_eq!(
        t.db_count("SELECT COUNT(*) FROM deliveries WHERE recipient = 'grok'"),
        0
    );
}

/// stdout closed before inbox can write: the lease expires and the message comes back.
#[test]
fn inbox_killed_before_output_redelivers() {
    let mut t = Team::new();
    t.reg("claude", "claude");
    t.reg("grok", "grok");
    t.run(&["--as", "claude", "send", "grok", "survives"]);
    // reader gone: the write fails with a broken pipe
    let (r, w) = std::io::pipe().unwrap();
    drop(r);
    let mut cmd = clean_command();
    cmd.arg("--team-dir")
        .arg(&t.dir)
        .args(["--as", "grok", "inbox"])
        .env("TINCAN_LEASE_SECS", "1")
        .stdin(Stdio::null())
        .stdout(Stdio::from(w));
    let mut p = cmd.spawn().unwrap();
    drop(cmd); // closes the parent's copy of the write end
    p.wait().unwrap();
    thread::sleep(Duration::from_millis(1300));
    let got = t
        .env(&["--as", "grok", "inbox"], &[("TINCAN_LEASE_SECS", "1")])
        .1;
    assert_eq!(bodies(&got), ["survives"]);
}

#[test]
fn no_reply_rejects_answers() {
    let mut t = Team::new();
    t.reg("a", "claude");
    t.reg("b", "codex");
    let mid = id(&t.out(&["--as", "a", "send", "b", "thanks, done", "--no-reply"]));
    assert_eq!(msgs(&t.out(&["--as", "b", "inbox"]))[0]["no_reply"], true);
    let (rc, o) = t.run(&[
        "--as",
        "b",
        "send",
        "a",
        "you're welcome",
        "--reply-to",
        &mid,
    ]);
    assert_eq!((rc, o["error"].as_str()), (8, Some("reply_not_wanted")));
}

#[test]
fn resumed_session_rebinds_pid() {
    let mut t = Team::new();
    let (old, new) = (t.owner(), t.owner());
    let (old_s, new_s) = (old.to_string(), new.to_string());
    t.env(
        &["register", "claude"],
        &[("TINCAN_OWNER_PID", &old_s), ("TINCAN_SESSION", "s1")],
    );
    t.kill(old);
    assert_eq!(t.out(&["peers", "--all"])["peers"][0]["state"], "stale");
    // same session id, new process: first call rebinds instead of failing
    let (rc, o) = t.env(
        &["whoami"],
        &[("TINCAN_OWNER_PID", &new_s), ("TINCAN_SESSION", "s1")],
    );
    assert_eq!(
        (rc, o["role"].as_str(), o["pid"].as_u64()),
        (0, Some("claude"), Some(new as u64))
    );
    assert_eq!(t.out(&["peers"])["peers"][0]["state"], "active");
}

#[test]
fn heartbeat_peer_stays_active_without_pid() {
    let t = Team::new();
    t.run(&["register", "codex", "--pid", "0"]);
    let p = &t.out(&["peers"])["peers"][0];
    assert_eq!(
        (&p["pid"], p["state"].as_str()),
        (&Value::Null, Some("active"))
    );
}

#[test]
fn usage_errors_are_json() {
    let t = Team::new();
    let (rc, o) = t.run(&["send"]);
    assert_eq!(
        (rc, o["ok"].as_bool(), o["error"].as_str()),
        (2, Some(false), Some("usage"))
    );
}

// ---- hooks: terminal-free delivery (Claude Code and Codex share the contract) ----

fn hook(t: &Team, event: &str, linger: Option<u32>) -> Value {
    let linger_s = linger.map(|l| l.to_string());
    let mut args = vec!["--as", "b", "hook", "--event", event];
    if let Some(l) = &linger_s {
        args.extend(["--linger", l]);
    }
    let payload = json!({"hook_event_name": event}).to_string();
    t.with(
        &args,
        Opts {
            stdin: Some(&payload),
            ..Opts::default()
        },
    )
    .1
}

#[test]
fn stop_blocks_once_per_new_message() {
    let mut t = Team::new();
    t.reg("a", "claude");
    t.reg("b", "codex");
    assert!(hook(&t, "Stop", None).is_null());
    t.run(&["--as", "a", "send", "b", "one"]);
    let o = hook(&t, "Stop", None);
    assert_eq!(o["decision"], "block");
    assert!(o["reason"].as_str().unwrap().contains("1 unread"));
    // agent ignored it: the same message never blocks a second stop (no stop loop)
    assert!(hook(&t, "Stop", None).is_null());
    t.run(&["--as", "a", "send", "b", "two"]);
    assert_eq!(hook(&t, "Stop", None)["decision"], "block");
}

#[test]
fn stop_lingers_for_reply_to_open_request() {
    let mut t = Team::new();
    t.reg("a", "claude");
    t.reg("b", "codex");
    let mid = id(&t.out(&["--as", "b", "send", "a", "question?"]));
    let (o, elapsed) = thread::scope(|s| {
        let t = &t;
        let mid = &mid;
        s.spawn(move || {
            thread::sleep(Duration::from_secs(1));
            t.run(&["--as", "a", "send", "b", "answer", "--reply-to", mid]);
        });
        let start = Instant::now();
        (hook(t, "Stop", Some(10)), start.elapsed())
    });
    assert_eq!(o["decision"], "block");
    assert!(elapsed < Duration::from_secs(5));
}

#[test]
fn stop_does_not_linger_without_open_request() {
    let mut t = Team::new();
    t.reg("a", "claude");
    t.reg("b", "codex");
    t.run(&["--as", "b", "send", "a", "fyi", "--no-reply"]);
    let start = Instant::now();
    assert!(hook(&t, "Stop", Some(10)).is_null());
    assert!(start.elapsed() < Duration::from_secs(2));
}

#[test]
fn post_tool_use_tells_once() {
    let mut t = Team::new();
    t.reg("a", "claude");
    t.reg("b", "codex");
    t.run(&["--as", "a", "send", "b", "mid-turn news"]);
    let o = hook(&t, "PostToolUse", None);
    assert!(
        o["hookSpecificOutput"]["additionalContext"]
            .as_str()
            .unwrap()
            .contains("1 unread")
    );
    assert!(hook(&t, "PostToolUse", None).is_null());
}

#[test]
fn session_end_releases_role() {
    let mut t = Team::new();
    t.reg("b", "codex");
    hook(&t, "SessionEnd", None);
    assert_eq!(t.out(&["peers", "--all"])["peers"][0]["state"], "gone");
}

#[test]
fn hook_identity_from_stdin_session_id() {
    let mut t = Team::new();
    t.env(
        &["register", "b", "--pid", "0"],
        &[("TINCAN_SESSION", "thread-9")],
    );
    t.reg("a", "claude");
    t.run(&["--as", "a", "send", "b", "hi"]);
    let payload = json!({"hook_event_name": "Stop", "session_id": "thread-9"}).to_string();
    let (_, o) = t.with(
        &["hook", "--event", "Stop"],
        Opts {
            stdin: Some(&payload),
            ..Opts::default()
        },
    );
    assert_eq!(o["decision"], "block");
}

#[test]
fn hooks_config_per_harness() {
    let (_, o) = exec(None, &["hooks", "--harness", "codex"], Opts::default());
    assert_eq!(o["file"], ".codex/hooks.json");
    assert!(o["config"]["hooks"].get("Stop").is_some());
    let (rc, o) = exec(None, &["hooks", "--harness", "grok"], Opts::default());
    assert_eq!((rc, o["error"].as_str()), (2, Some("usage")));
}

// ---- wake drivers: opt-in per peer; the default touches no terminal ----

fn reg_wake(t: &mut Team, role: &str, spec: &str, env: &[(&str, &str)]) -> (i32, Value) {
    let p = t.owner().to_string();
    t.env(
        &[
            "register",
            role,
            "--harness",
            "codex",
            "--pid",
            &p,
            "--wake",
            spec,
        ],
        env,
    )
}

#[test]
fn default_is_no_wake() {
    let mut t = Team::new();
    t.reg("a", "claude");
    t.reg("b", "codex");
    let o = t.out(&["--as", "a", "send", "b", "x"]);
    assert_eq!(o["wake"], json!({}));
}

#[test]
fn cmd_driver_nudges_once_per_new_message() {
    let mut t = Team::new();
    let log = t.path("wake.log");
    t.reg("a", "claude");
    // Redirect first on Windows: `echo b 1>> f` parses `1>>` as a stdout redirect.
    let spec = if cfg!(windows) {
        format!(
            "cmd:>> \"{}\" echo %TINCAN_WAKE_ROLE% %TINCAN_WAKE_UNREAD%",
            log.display()
        )
    } else {
        format!(
            "cmd:echo \"$TINCAN_WAKE_ROLE $TINCAN_WAKE_UNREAD\" >> \"{}\"",
            log.display()
        )
    };
    let (rc, o) = reg_wake(&mut t, "b", &spec, &[]);
    assert_eq!(rc, 0, "{o}");
    assert_eq!(
        t.out(&["--as", "a", "send", "b", "1"])["wake"],
        json!({"b": "nudged"})
    );
    // recipient hasn't read yet; a Stop hook or earlier nudge already told it about seq<=1
    assert_eq!(
        t.out(&["--as", "a", "send", "b", "2"])["wake"],
        json!({"b": "nudged"})
    );
    t.run(&["--as", "b", "inbox"]);
    let text = std::fs::read_to_string(&log).unwrap();
    let lines: Vec<&str> = text.lines().map(str::trim_end).take(2).collect();
    assert_eq!(lines, ["b 1", "b 2"]);
}

#[test]
fn nudge_counts_as_told_for_stop_hook() {
    let mut t = Team::new();
    let log = t.path("wake.log");
    t.reg("a", "claude");
    reg_wake(
        &mut t,
        "b",
        &format!("cmd:echo x >> \"{}\"", log.display()),
        &[],
    );
    t.run(&["--as", "b", "hook", "--event", "Stop"]);
    let o = t.out(&["--as", "a", "send", "b", "1"]);
    assert_eq!(o["wake"], json!({"b": "nudged"}));
    let (_, o) = t.with(
        &["--as", "b", "hook", "--event", "Stop"],
        Opts {
            stdin: Some(r#"{"hook_event_name":"Stop"}"#),
            ..Opts::default()
        },
    );
    assert!(o.is_null());
}

#[test]
fn failed_nudge_never_fails_send() {
    let mut t = Team::new();
    t.reg("a", "claude");
    reg_wake(&mut t, "b", "cmd:exit 3", &[]);
    let (rc, o) = t.run(&["--as", "a", "send", "b", "x"]);
    assert_eq!(rc, 0);
    assert!(o["wake"]["b"].as_str().unwrap().starts_with("failed"));
}

#[test]
fn bad_spec_rejected() {
    let mut t = Team::new();
    let (rc, o) = reg_wake(&mut t, "b", "zellij:1", &[]);
    assert_eq!((rc, o["error"].as_str()), (2, Some("usage")));
}

#[test]
fn auto_picks_current_terminal() {
    let mut t = Team::new();
    let (_, o) = reg_wake(&mut t, "b", "auto", &[("TMUX_PANE", "%7")]);
    assert_eq!(o["wake"], "tmux:%7");
}

// ---- harness profiles: new harnesses are data, no code change ----

#[test]
fn user_profile_detected_by_marker_env() {
    let mut t = Team::new();
    let prof = t.write(
        "harnesses.json",
        &json!([{"name": "opencode", "session_env": ["OPENCODE_SESSION"],
                 "marker_env": ["OPENCODE_RUN"], "hooks_file": ".opencode/hooks.json"}])
        .to_string(),
    );
    let owner = t.owner().to_string();
    let env = [
        ("TINCAN_HARNESSES", prof.as_str()),
        ("OPENCODE_RUN", "1"),
        ("OPENCODE_SESSION", "oc-1"),
        ("TINCAN_OWNER_PID", owner.as_str()),
    ];
    let (rc, o) = t.env(&["register", "oc"], &env);
    assert_eq!((rc, o["harness"].as_str()), (0, Some("opencode")));
    // its session id resolves identity without --as
    assert_eq!(t.env(&["whoami"], &env).1["role"], "oc");
    let (_, o) = exec(
        None,
        &["hooks", "--harness", "opencode"],
        Opts {
            env: &env,
            ..Opts::default()
        },
    );
    assert_eq!(o["file"], ".opencode/hooks.json");
}

// ---- fan-out: broadcast one question, gather every answer ----

#[test]
fn wait_replies_gathers_all_recipients() {
    let mut t = Team::new();
    t.reg("lead", "claude");
    t.reg("x", "codex");
    t.reg("y", "grok");
    let mid = id(&t.out(&["--as", "lead", "send", "*", "vote?"]));
    let o = thread::scope(|s| {
        let t = &t;
        let mid = &mid;
        for (role, delay) in [("x", 300), ("y", 1000)] {
            s.spawn(move || {
                thread::sleep(Duration::from_millis(delay));
                let body = format!("{role} says yes");
                t.run(&["--as", role, "send", "lead", &body, "--reply-to", mid]);
            });
        }
        t.out(&[
            "--as",
            "lead",
            "wait",
            "--replies-to",
            mid,
            "--timeout",
            "10",
        ])
    });
    assert_eq!(strs(&o["replied"]), ["x", "y"]);
    assert_eq!(o["waiting"], json!([]));
    assert_eq!(o["timed_out"], false);
    let inbox = t.out(&["--as", "lead", "inbox"]);
    let mut got = bodies(&inbox);
    got.sort();
    assert_eq!(got, ["x says yes", "y says yes"]);
}

#[test]
fn wait_replies_stops_for_ended_session() {
    let mut t = Team::new();
    t.reg("lead", "claude");
    let px = t.reg("x", "codex");
    let mid = id(&t.out(&["--as", "lead", "send", "x", "q?"]));
    t.kill(px);
    let start = Instant::now();
    let o = t.out(&[
        "--as",
        "lead",
        "wait",
        "--replies-to",
        &mid,
        "--timeout",
        "10",
    ]);
    assert_eq!(
        (&o["ended"], &o["timed_out"]),
        (&json!(["x"]), &json!(false))
    );
    assert!(start.elapsed() < Duration::from_secs(3));
}

#[test]
fn wait_replies_times_out_with_stragglers() {
    let mut t = Team::new();
    t.reg("lead", "claude");
    t.reg("x", "codex");
    let mid = id(&t.out(&["--as", "lead", "send", "x", "q?"]));
    let o = t.out(&[
        "--as",
        "lead",
        "wait",
        "--replies-to",
        &mid,
        "--timeout",
        "0.5",
    ]);
    assert_eq!(
        (&o["waiting"], &o["timed_out"]),
        (&json!(["x"]), &json!(true))
    );
}

// ---- extensions: drivers and harnesses plug in as JSON; a broken one never breaks the core ----

/// argv prefix of a fake terminal CLI that logs each call's args as `a|b|c|`.
#[cfg(unix)]
fn fake_terminal(t: &Team, log: &str) -> Vec<String> {
    use std::os::unix::fs::PermissionsExt;
    let fake = t.write(
        "fake-term",
        &format!("#!/bin/sh\nprintf '%s|' \"$@\" >> {log}; echo >> {log}\n"),
    );
    std::fs::set_permissions(&fake, std::fs::Permissions::from_mode(0o755)).unwrap();
    vec![fake]
}

/// cmd's echo logs the args space-separated, keeping the quotes argv added around the text.
/// (PowerShell would be closer, but its startup can exceed the 3 s helper timeout.)
#[cfg(windows)]
fn fake_terminal(_t: &Team, log: &str) -> Vec<String> {
    ["cmd", "/D", "/C", ">>", log, "echo"]
        .iter()
        .map(|s| s.to_string())
        .collect()
}

#[test]
fn user_driver_runs_argv_template_without_shell() {
    let mut t = Team::new();
    let log = t.path("argv.log").display().to_string();
    let fake = fake_terminal(&t, &log);
    let step = |args: &[&str]| -> Vec<String> {
        fake.iter()
            .map(String::as_str)
            .chain(args.iter().copied())
            .map(str::to_string)
            .collect()
    };
    let drivers = t.write(
        "drivers.json",
        &json!([{"name": "faketerm", "detect_env": "FAKETERM_PANE",
                 "nudge": [step(&["type", "{target}", "{text}"]), step(&["key", "{target}", "enter"])]}])
        .to_string(),
    );
    t.reg("a", "claude");
    let p = t.owner().to_string();
    let (_, o) = t.env(
        &["register", "b", "--pid", &p, "--wake", "auto"],
        &[
            ("TINCAN_DRIVERS", drivers.as_str()),
            ("FAKETERM_PANE", "w1:p9"),
        ],
    );
    assert_eq!(o["wake"], "faketerm:w1:p9");
    let (_, o) = t.env(
        &["--as", "a", "send", "b", "x"],
        &[("TINCAN_DRIVERS", drivers.as_str())],
    );
    assert_eq!(o["wake"], json!({"b": "nudged"}));
    let text = std::fs::read_to_string(&log).unwrap();
    let lines: Vec<&str> = text.lines().map(str::trim_end).collect();
    let (typed, key) = if cfg!(windows) {
        (
            "type w1:p9 \"[tincan] role b has 1 unread",
            "key w1:p9 enter",
        )
    } else {
        (
            "type|w1:p9|[tincan] role b has 1 unread",
            "key|w1:p9|enter|",
        )
    };
    assert!(lines[0].starts_with(typed), "{}", lines[0]);
    assert_eq!(lines[1], key);
}

#[test]
fn broken_driver_file_keeps_builtins() {
    let mut t = Team::new();
    let drivers = t.write("drivers.json", "{not json");
    let env = [("TINCAN_DRIVERS", drivers.as_str())];
    let (_, o) = exec(
        None,
        &["extensions"],
        Opts {
            env: &env,
            ..Opts::default()
        },
    );
    assert_eq!(o["ok"], false);
    let names: Vec<&str> = o["wake"]
        .as_array()
        .unwrap()
        .iter()
        .map(|w| w["name"].as_str().unwrap())
        .collect();
    assert_eq!(names, ["tmux", "cmux", "cmd"]);
    assert_eq!(o["errors"].as_array().unwrap().len(), 1);
    // and the core still works
    t.reg("a", "claude");
    t.reg("b", "codex");
    assert_eq!(t.env(&["--as", "a", "send", "b", "x"], &env).0, 0);
}

#[test]
fn invalid_entry_skipped_valid_one_loaded() {
    let t = Team::new();
    let drivers = t.write(
        "drivers.json",
        &json!([{"name": "cmd", "nudge": [["x"]]}, {"name": "good", "nudge": [["true"]]}])
            .to_string(),
    );
    let (_, o) = exec(
        None,
        &["extensions"],
        Opts {
            env: &[("TINCAN_DRIVERS", drivers.as_str())],
            ..Opts::default()
        },
    );
    assert!(
        o["wake"]
            .as_array()
            .unwrap()
            .iter()
            .any(|w| w["name"] == "good")
    );
    assert!(o["errors"][0].as_str().unwrap().contains("reserved"));
}

// ---- local-only ----

#[test]
fn no_team_error_points_remote_agents_to_run_locally() {
    let d = TmpDir::new();
    let (rc, o) = exec(
        None,
        &["inbox"],
        Opts {
            cwd: Some(&d.0),
            ..Opts::default()
        },
    );
    assert_eq!((rc, o["error"].as_str()), (2, Some("no_team")));
    let hint = o["hint"].as_str().unwrap();
    assert!(hint.contains("local-only"));
    assert!(hint.contains("run this session locally"));
}

#[test]
fn init_without_team_dir_initialises_cwd() {
    let d = TmpDir::new();
    let opts = Opts {
        cwd: Some(&d.0),
        ..Opts::default()
    };
    // other commands still refuse a dir with no team
    assert_eq!(exec(None, &["peers"], opts).0, 2);
    let (rc, o) = exec(None, &["init"], opts);
    assert_eq!(rc, 0, "{o}");
    assert!(d.0.join(".tincan").is_dir());
    assert_eq!(exec(None, &["peers"], opts).0, 0);
}
