//! Wake drivers: how a sender pokes an idle recipient after its mail is committed.
//!
//! Terminal drivers are data: argv templates for "read the screen" and "type the nudge".
//! tmux and cmux ship as built-in entries; users add more (herdr, zellij, wezterm, ...) in
//! `$TINCAN_DRIVERS`, else `~/.config/tincan/drivers.json`, without rebuilding. Templates run
//! without a shell, one argv element per placeholder, so message text can't inject commands.
//! `cmd:<shell>` is the one coded escape hatch. None is the default: hooks and `tincan wait`.

mod cmd;

use serde_json::{Value, json};
use std::io;
use std::process::{Command, Stdio};

pub trait Waker {
    /// Recent screen text, used to skip a nudge while a turn is running. None = can't tell.
    fn screen(&self) -> Option<String> {
        None
    }
    /// Deliver the nudge text to the recipient.
    fn nudge(&self, text: &str, env: &[(&str, String)]) -> io::Result<()>;
}

/// Names that aren't drivers and can't be redefined.
const RESERVED: [&str; 3] = ["none", "auto", "cmd"];

#[derive(Debug, Clone)]
pub struct Driver {
    pub name: String,
    /// Env var holding this session's target inside the terminal (`--wake auto` reads it).
    pub detect_env: Option<String>,
    /// argv that prints recent screen text. Placeholders: {target}.
    pub screen: Option<Vec<String>>,
    /// argv list run in order to type the nudge. Placeholders: {target} {text} {role} {unread}.
    pub nudge: Vec<Vec<String>>,
    pub builtin: bool,
}

const BUILTINS: &str = r#"[
  {"name": "tmux", "detect_env": "TMUX_PANE",
   "screen": ["tmux", "capture-pane", "-p", "-t", "{target}"],
   "nudge": [["tmux", "send-keys", "-t", "{target}", "-l", "{text}"],
             ["tmux", "send-keys", "-t", "{target}", "Enter"]]},
  {"name": "cmux", "detect_env": "CMUX_SURFACE_ID",
   "screen": ["cmux", "read-screen", "--surface", "{target}", "--lines", "8"],
   "nudge": [["cmux", "send", "--surface", "{target}", "{text}"],
             ["cmux", "send-key", "--surface", "{target}", "Enter"]]}
]"#;

/// User drivers, then built-ins (a user entry with a built-in's name replaces it).
/// User drivers go first so `--wake auto` prefers them: a terminal nested inside another
/// (herdr in cmux) is the one the user configured. Bad user entries are reported, never fatal.
pub fn load() -> (Vec<Driver>, Vec<String>) {
    let mut errors = vec![];
    let mut user: Vec<Driver> = vec![];
    let builtins: Vec<Value> = serde_json::from_str(BUILTINS).expect("built-in drivers parse");
    let mut out: Vec<Driver> = builtins
        .iter()
        .map(|v| parse(v, true).expect("built-in driver valid"))
        .collect();
    let path = std::env::var_os("TINCAN_DRIVERS")
        .map(std::path::PathBuf::from)
        .or_else(|| crate::harness::config_file("drivers.json"));
    if let Some(path) = path
        && let Ok(text) = std::fs::read_to_string(&path)
    {
        match serde_json::from_str::<Value>(&text) {
            Ok(Value::Array(items)) => {
                for item in &items {
                    match parse(item, false) {
                        Ok(d) => {
                            out.retain(|b| b.name != d.name);
                            user.push(d);
                        }
                        Err(e) => errors.push(format!("{}: {e}", path.display())),
                    }
                }
            }
            Ok(_) => errors.push(format!("{}: expected a JSON array", path.display())),
            Err(e) => errors.push(format!("{}: {e}", path.display())),
        }
    }
    user.extend(out);
    (user, errors)
}

fn parse(v: &Value, builtin: bool) -> Result<Driver, String> {
    let argv = |x: &Value| -> Option<Vec<String>> {
        let a: Vec<String> = x
            .as_array()?
            .iter()
            .map(|s| s.as_str().map(str::to_string))
            .collect::<Option<_>>()?;
        (!a.is_empty()).then_some(a)
    };
    let name = v
        .get("name")
        .and_then(Value::as_str)
        .ok_or("driver needs a \"name\"")?
        .to_string();
    if RESERVED.contains(&name.as_str())
        || name.len() > 64
        || !name
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.'))
    {
        return Err(format!("driver name {name:?} is reserved or invalid"));
    }
    let nudge: Vec<Vec<String>> = v
        .get("nudge")
        .and_then(Value::as_array)
        .map(|steps| steps.iter().map(argv).collect::<Option<_>>())
        .unwrap_or(None)
        .filter(|s: &Vec<Vec<String>>| !s.is_empty())
        .ok_or(format!(
            "driver {name:?} needs \"nudge\": a list of argv arrays"
        ))?;
    let screen = match v.get("screen") {
        None | Some(Value::Null) => None,
        Some(s) => {
            Some(argv(s).ok_or(format!("driver {name:?}: \"screen\" must be an argv array"))?)
        }
    };
    Ok(Driver {
        detect_env: v
            .get("detect_env")
            .and_then(Value::as_str)
            .map(str::to_string),
        screen,
        nudge,
        builtin,
        name,
    })
}

impl Driver {
    pub fn describe(&self) -> Value {
        json!({"name": self.name, "builtin": self.builtin, "detect_env": self.detect_env,
               "busy_check": self.screen.is_some()})
    }
}

/// A driver bound to one target (pane, surface, ...).
struct Template {
    driver: Driver,
    target: String,
}

pub(crate) fn fill(argv: &[String], vars: &[(&str, &str)]) -> Vec<String> {
    argv.iter()
        .map(|a| {
            vars.iter()
                .fold(a.clone(), |acc, (k, v)| acc.replace(&format!("{{{k}}}"), v))
        })
        .collect()
}

fn exec(argv: &[String]) -> io::Result<String> {
    let (prog, args) = argv.split_first().expect("argv checked non-empty");
    run(Command::new(prog).args(args))
}

impl Waker for Template {
    fn screen(&self) -> Option<String> {
        let argv = self.driver.screen.as_ref()?;
        exec(&fill(argv, &[("target", &self.target)])).ok()
    }

    fn nudge(&self, text: &str, env: &[(&str, String)]) -> io::Result<()> {
        let get = |k: &str| {
            env.iter()
                .find(|(n, _)| *n == k)
                .map(|(_, v)| v.as_str())
                .unwrap_or("")
        };
        let vars = [
            ("target", self.target.as_str()),
            ("text", text),
            ("role", get("TINCAN_WAKE_ROLE")),
            ("unread", get("TINCAN_WAKE_UNREAD")),
        ];
        for step in &self.driver.nudge {
            exec(&fill(step, &vars))?;
        }
        Ok(())
    }
}

fn kinds(drivers: &[Driver]) -> Vec<String> {
    drivers
        .iter()
        .map(|d| d.name.clone())
        .chain(["cmd".to_string()])
        .collect()
}

/// Normalise a `--wake` value into a stored spec. `auto` picks the terminal this call runs in.
pub fn resolve(spec: &str) -> Result<Option<String>, String> {
    let (drivers, _) = load();
    match spec {
        "none" => Ok(None),
        "auto" => Ok(drivers.iter().find_map(|d| {
            let target = std::env::var(d.detect_env.as_ref()?).ok()?;
            (!target.is_empty()).then(|| format!("{}:{target}", d.name))
        })),
        _ => {
            let (kind, arg) = spec.split_once(':').unwrap_or((spec, ""));
            let known = kinds(&drivers);
            if arg.is_empty() || !known.iter().any(|k| k == kind) {
                return Err(format!(
                    "bad --wake {spec:?}: use none, auto, or KIND:ARG with KIND one of {}",
                    known.join(", ")
                ));
            }
            Ok(Some(spec.to_string()))
        }
    }
}

pub fn build(spec: &str) -> Option<Box<dyn Waker>> {
    let (kind, arg) = spec.split_once(':')?;
    if kind == "cmd" {
        return Some(Box::new(cmd::Cmd(arg.into())));
    }
    let (drivers, _) = load();
    let driver = drivers.into_iter().find(|d| d.name == kind)?;
    Some(Box::new(Template {
        driver,
        target: arg.into(),
    }))
}

/// Run a helper with a hard timeout so a hung terminal never stalls `send`.
fn run(cmd: &mut Command) -> io::Result<String> {
    run_with_timeout(cmd, std::time::Duration::from_secs(3))
}

fn run_with_timeout(cmd: &mut Command, timeout: std::time::Duration) -> io::Result<String> {
    #[cfg(unix)]
    std::os::unix::process::CommandExt::process_group(cmd, 0);
    #[cfg(windows)]
    std::os::windows::process::CommandExt::creation_flags(cmd, 0x0000_0200);
    let mut child = cmd
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()?;
    let deadline = std::time::Instant::now() + timeout;
    let status = loop {
        if let Some(status) = child.try_wait()? {
            break status;
        }
        if std::time::Instant::now() >= deadline {
            #[cfg(unix)]
            unsafe {
                libc::kill(-(child.id() as libc::pid_t), libc::SIGKILL);
            }
            #[cfg(windows)]
            let _ = child.kill();
            let _ = child.wait();
            return Err(io::Error::new(
                io::ErrorKind::TimedOut,
                "wake helper timed out",
            ));
        }
        std::thread::sleep(std::time::Duration::from_millis(10));
    };
    let mut bytes = vec![];
    if let Some(mut stdout) = child.stdout.take() {
        std::io::Read::read_to_end(&mut stdout, &mut bytes)?;
    }
    if !status.success() {
        return Err(io::Error::other(format!("wake helper exited {status}")));
    }
    Ok(String::from_utf8_lossy(&bytes).into_owned())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn resolve_validates_kind_and_arg() {
        assert_eq!(resolve("none").unwrap(), None);
        assert_eq!(resolve("tmux:%3").unwrap().as_deref(), Some("tmux:%3"));
        assert!(resolve("tmux").is_err());
        assert!(resolve("zellij:1").is_err());
    }

    #[test]
    fn placeholders_fill_whole_argv_elements_without_a_shell() {
        let argv = vec!["x".into(), "{text}".into(), "-t={target}".into()];
        let out = fill(&argv, &[("text", "a; rm -rf ~"), ("target", "p1")]);
        assert_eq!(out, vec!["x", "a; rm -rf ~", "-t=p1"]);
    }

    #[test]
    fn reserved_and_malformed_drivers_rejected() {
        assert!(parse(&json!({"name": "cmd", "nudge": [["x"]]}), false).is_err());
        assert!(parse(&json!({"name": "../bad", "nudge": [["x"]]}), false).is_err());
        assert!(parse(&json!({"name": "z", "nudge": []}), false).is_err());
        assert!(
            parse(
                &json!({"name": "z", "nudge": [["x"]], "screen": "oops"}),
                false
            )
            .is_err()
        );
        assert!(parse(&json!({"name": "z", "nudge": [["x", "{text}"]]}), false).is_ok());
    }

    #[cfg(unix)]
    #[test]
    fn timed_out_helper_is_killed() {
        let pid_file = std::env::temp_dir().join(format!(
            "tincan-wake-test-{}-{}",
            std::process::id(),
            crate::store::now()
        ));
        let mut command = Command::new("sh");
        command.args([
            "-c",
            "echo $$ > \"$1\"; exec sleep 30",
            "sh",
            &pid_file.to_string_lossy(),
        ]);
        let started = std::time::Instant::now();
        let error = run_with_timeout(&mut command, std::time::Duration::from_millis(100))
            .expect_err("helper should time out");
        assert_eq!(error.kind(), io::ErrorKind::TimedOut);
        assert!(started.elapsed() < std::time::Duration::from_secs(2));
        let pid: i32 = std::fs::read_to_string(&pid_file)
            .unwrap()
            .trim()
            .parse()
            .unwrap();
        let alive = unsafe { libc::kill(pid, 0) } == 0;
        assert!(!alive, "helper process {pid} leaked");
        let _ = std::fs::remove_file(pid_file);
    }
}
