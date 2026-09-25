//! Live round trips against real harness CLIs: the tester asks, the harness answers over tincan.
//!
//! Each harness is one entry in tests/live/harnesses.json (command template + requirements), so
//! adding one — e.g. another OpenRouter model through opencode — is a data change. Entries whose
//! binary or required env is missing are skipped, not failed.
//!
//!   cargo test --release --test live -- --ignored --nocapture                # every available harness
//!   TINCAN_LIVE=codex,claude cargo test --release --test live -- --ignored   # just these
//!   TINCAN_LIVE_MODEL=google/gemma-4-31b-it:free TINCAN_LIVE=opencode-openrouter cargo test --test live -- --ignored
//!   cargo test --release --test live broadcast -- --ignored --nocapture     # one question to all at once

use serde_json::Value;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::thread;
use std::time::{Duration, Instant};

const BIN: &str = env!("CARGO_BIN_EXE_tincan");
const PROMPT: &str = "You are role {role} on a tincan team; the `tincan` CLI is on PATH. Run `tincan inbox`, \
     then answer each message with ONE command: tincan send <from> \"<answer>\" --reply-to <id>. \
     Do nothing else, then stop.";

fn harnesses() -> Vec<Value> {
    let path = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/live/harnesses.json");
    let all: Vec<Value> = serde_json::from_str(&std::fs::read_to_string(path).unwrap()).unwrap();
    let wanted: Vec<String> = std::env::var("TINCAN_LIVE")
        .map(|s| s.split(',').map(|w| w.trim().to_string()).collect())
        .unwrap_or_default();
    all.into_iter()
        .filter(|h| wanted.is_empty() || wanted.iter().any(|w| h["name"] == w.as_str()))
        .collect()
}

fn name(h: &Value) -> &str {
    h["name"].as_str().unwrap()
}

fn strs(v: &Value) -> Vec<String> {
    v.as_array()
        .map(|a| a.iter().map(|s| s.as_str().unwrap().to_string()).collect())
        .unwrap_or_default()
}

fn timeout() -> f64 {
    std::env::var("TINCAN_LIVE_TIMEOUT")
        .ok()
        .and_then(|s| s.parse().ok())
        .unwrap_or(240.0)
}

/// PATH with the tincan under test first, so harnesses run this build.
fn path_env() -> std::ffi::OsString {
    let dir = Path::new(BIN).parent().unwrap().to_path_buf();
    let rest = std::env::var_os("PATH").unwrap_or_default();
    std::env::join_paths(std::iter::once(dir).chain(std::env::split_paths(&rest))).unwrap()
}

fn on_path(bin: &str) -> bool {
    let exts: &[&str] = if cfg!(windows) {
        &["", ".exe", ".cmd", ".bat"]
    } else {
        &[""]
    };
    std::env::split_paths(&std::env::var_os("PATH").unwrap_or_default())
        .any(|d| exts.iter().any(|e| d.join(format!("{bin}{e}")).is_file()))
}

fn skip_reason(h: &Value) -> Option<String> {
    // Entries marked free_only run only on `:free` models unless paid runs are explicitly allowed.
    if h["free_only"] == true && std::env::var_os("TINCAN_LIVE_ALLOW_PAID").is_none() {
        let model = std::env::var("TINCAN_LIVE_MODEL")
            .ok()
            .or_else(|| h["model"].as_str().map(str::to_string))
            .unwrap_or_default();
        if !model.ends_with(":free") {
            return Some(format!(
                "{model} is not a :free model (set TINCAN_LIVE_ALLOW_PAID=1)"
            ));
        }
    }
    let bin = h["bin"].as_str().unwrap();
    if !on_path(bin) {
        return Some(format!("{bin} not on PATH"));
    }
    let check = strs(&h["check"]);
    if !check.is_empty() {
        let ok = Command::new(&check[0])
            .args(&check[1..])
            .stdin(Stdio::null())
            .output()
            .is_ok_and(|o| o.status.success());
        if !ok {
            return Some(format!(
                "`{}` failed (shim without the real binary?)",
                check.join(" ")
            ));
        }
    }
    let missing: Vec<String> = strs(&h["requires_env"])
        .into_iter()
        .filter(|v| std::env::var(v).map_or(true, |s| s.is_empty()))
        .collect();
    (!missing.is_empty()).then(|| format!("needs {}", missing.join(", ")))
}

fn tincan(team: &Path, args: &[&str]) -> Value {
    let out = Command::new(BIN)
        .args(args)
        .env("TINCAN_TEAM_DIR", team)
        .stdin(Stdio::null())
        .output()
        .unwrap();
    let stdout = String::from_utf8_lossy(&out.stdout);
    serde_json::from_str(stdout.lines().next().unwrap_or("null")).unwrap()
}

fn new_team(label: &str) -> PathBuf {
    let secs = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_millis();
    let team =
        std::env::temp_dir().join(format!("tincan-live-{label}-{}-{secs}", std::process::id()));
    std::fs::create_dir_all(&team).unwrap();
    tincan(&team, &["init"]);
    tincan(
        &team,
        &["register", "tester", "--harness", "test", "--pid", "0"],
    );
    team
}

fn register(team: &Path, role: &str, h: &Value) {
    let profile = h["profile"].as_str().unwrap_or(name(h));
    tincan(
        team,
        &["register", role, "--harness", profile, "--pid", "0"],
    );
}

/// Run one headless turn of harness h as role. Returns the output tail, or None on timeout.
fn turn(h: &Value, team: &Path, role: &str, timeout: f64) -> Option<String> {
    let prompt = PROMPT.replace("{role}", role);
    let model = std::env::var("TINCAN_LIVE_MODEL")
        .ok()
        .or_else(|| h["model"].as_str().map(str::to_string))
        .unwrap_or_default();
    let team_s = team.display().to_string();
    let cmd: Vec<String> = strs(&h["cmd"])
        .iter()
        .map(|a| {
            a.replace("{team}", &team_s)
                .replace("{prompt}", &prompt)
                .replace("{model}", &model)
        })
        .collect();
    let feed_prompt = h["stdin"] == "prompt";
    let mut child = Command::new(&cmd[0])
        .args(&cmd[1..])
        .current_dir(team)
        .env("TINCAN_TEAM_DIR", team)
        .env("TINCAN_ROLE", role)
        .env("PATH", path_env())
        // Keep harnesses on their subscription logins: a stray API key would switch them to per-token billing.
        .env_remove("ANTHROPIC_API_KEY")
        .env_remove("ANTHROPIC_AUTH_TOKEN")
        .env_remove("OPENAI_API_KEY")
        .env_remove("CODEX_API_KEY")
        .env_remove("XAI_API_KEY")
        .env_remove("GROK_CODE_XAI_API_KEY")
        // Harnesses that don't read the prompt from stdin get /dev/null: `codex exec` hangs on an open stdin.
        .stdin(if feed_prompt {
            Stdio::piped()
        } else {
            Stdio::null()
        })
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .ok()?;
    if feed_prompt {
        use std::io::Write;
        let mut pipe = child.stdin.take().unwrap();
        let _ = pipe.write_all(prompt.as_bytes());
    }
    // Drain output on threads so the harness never blocks on a full pipe; poll for exit so a
    // timeout can still kill it.
    let drain = |r: Option<Box<dyn std::io::Read + Send>>| {
        thread::spawn(move || {
            let mut buf = Vec::new();
            if let Some(mut r) = r {
                let _ = r.read_to_end(&mut buf);
            }
            buf
        })
    };
    let out = drain(child.stdout.take().map(|r| Box::new(r) as _));
    let err = drain(child.stderr.take().map(|r| Box::new(r) as _));
    let deadline = Instant::now() + Duration::from_secs_f64(timeout);
    loop {
        match child.try_wait() {
            Ok(Some(_)) => break,
            Ok(None) if Instant::now() < deadline => {
                thread::sleep(Duration::from_millis(200));
            }
            _ => {
                let _ = child.kill();
                let _ = child.wait();
                return None;
            }
        }
    }
    let text = format!(
        "{}{}",
        String::from_utf8_lossy(&out.join().unwrap_or_default()),
        String::from_utf8_lossy(&err.join().unwrap_or_default())
    );
    let tail: String = text
        .chars()
        .rev()
        .take(400)
        .collect::<Vec<_>>()
        .into_iter()
        .rev()
        .collect();
    Some(tail)
}

fn round_trip(h: &Value, timeout: f64) -> (bool, f64, String, PathBuf) {
    let team = new_team(name(h));
    let role = format!("{}-peer", name(h));
    register(&team, &role, h);
    let ask = tincan(
        &team,
        &[
            "--as",
            "tester",
            "send",
            &role,
            "What is 6*7? Reply with just the number.",
        ],
    );
    let start = Instant::now();
    let Some(tail) = turn(h, &team, &role, timeout) else {
        return (
            false,
            start.elapsed().as_secs_f64(),
            "timed out".into(),
            team,
        );
    };
    let inbox = tincan(&team, &["--as", "tester", "inbox"]);
    let got = inbox["messages"].as_array().cloned().unwrap_or_default();
    let ok = got.iter().any(|m| {
        m["reply_to"] == ask["id"] && m["body"].as_str().is_some_and(|b| b.contains("42"))
    });
    let detail = if got.is_empty() {
        tail
    } else {
        Value::from(got).to_string()
    };
    (ok, start.elapsed().as_secs_f64(), detail, team)
}

#[test]
#[ignore = "runs real harness CLIs; see the module docs"]
fn round_trips() {
    let timeout = timeout();
    let mut failed = vec![];
    for h in harnesses() {
        if let Some(reason) = skip_reason(&h) {
            println!("SKIP {:<22} {reason}", name(&h));
            continue;
        }
        let (ok, secs, detail, team) = round_trip(&h, timeout);
        if ok {
            println!("PASS {:<22} {secs:5.1}s", name(&h));
        } else {
            println!(
                "FAIL {:<22} {secs:5.1}s  team={}\n     {detail}",
                name(&h),
                team.display()
            );
            failed.push(name(&h).to_string());
        }
    }
    assert!(failed.is_empty(), "failed: {failed:?}");
}

/// One team, one broadcast, every harness answering concurrently; the tester gathers all.
#[test]
#[ignore = "runs real harness CLIs; see the module docs"]
fn broadcast() {
    let timeout = timeout();
    let mut ready = vec![];
    for h in harnesses() {
        match skip_reason(&h) {
            Some(reason) => println!("SKIP {:<22} {reason}", name(&h)),
            None => ready.push(h),
        }
    }
    let team = new_team("broadcast");
    let roles: Vec<(String, &Value)> = ready
        .iter()
        .map(|h| (format!("{}-peer", name(h)), h))
        .collect();
    for (role, h) in &roles {
        register(&team, role, h);
    }
    let ask = tincan(
        &team,
        &[
            "--as",
            "tester",
            "send",
            "*",
            "Name one prime number between 10 and 20. Reply with just the number.",
        ],
    );
    let ask_id = ask["id"].as_str().unwrap().to_string();
    let start = Instant::now();
    thread::scope(|s| {
        for (role, h) in &roles {
            let team = &team;
            s.spawn(move || turn(h, team, role, timeout));
        }
        tincan(
            &team,
            &[
                "--as",
                "tester",
                "wait",
                "--replies-to",
                &ask_id,
                "--timeout",
                &timeout.to_string(),
            ],
        );
    });
    let inbox = tincan(&team, &["--as", "tester", "inbox"]);
    let answers: Vec<(String, String)> = inbox["messages"]
        .as_array()
        .cloned()
        .unwrap_or_default()
        .iter()
        .map(|m| {
            (
                m["from"].as_str().unwrap_or_default().to_string(),
                m["body"].as_str().unwrap_or_default().to_string(),
            )
        })
        .collect();
    let mut missing = vec![];
    for (role, _) in &roles {
        match answers.iter().find(|(from, _)| from == role) {
            Some((_, body)) => println!("PASS {role:<26} {body:?}"),
            None => {
                println!("FAIL {role:<26} -");
                missing.push(role.clone());
            }
        }
    }
    println!(
        "gathered {}/{} in {:.1}s  team={}",
        roles.len() - missing.len(),
        roles.len(),
        start.elapsed().as_secs_f64(),
        team.display()
    );
    assert!(missing.is_empty(), "no answer from: {missing:?}");
}
