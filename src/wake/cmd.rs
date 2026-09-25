use super::{Waker, run};
use std::process::Command;

/// `cmd:<shell command>`: the escape hatch for anything else (zellij, a desktop notifier,
/// `codex exec resume`, an OpenRouter-backed agent's API). Gets TINCAN_WAKE_* in its env.
pub struct Cmd(pub String);

impl Waker for Cmd {
    fn nudge(&self, _text: &str, env: &[(&str, String)]) -> std::io::Result<()> {
        let mut c = Command::new("sh");
        c.args(["-c", &self.0]);
        for (k, v) in env {
            c.env(k, v);
        }
        run(&mut c).map(|_| ())
    }
}
