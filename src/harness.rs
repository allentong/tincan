//! Harness profiles: everything tincan knows about one agent CLI, as data.
//! Built-ins cover claude, codex and grok; a JSON file adds or overrides profiles
//! (`$TINCAN_HARNESSES`, else `~/.config/tincan/harnesses.json`), so a new harness —
//! opencode on OpenRouter, goose, aider — needs no code change.

use serde_json::Value;

#[derive(Debug, Clone, PartialEq)]
pub struct Profile {
    pub name: String,
    /// Executable-name prefixes that identify the harness process when walking up the tree.
    pub process_names: Vec<String>,
    /// Env vars holding the harness's session id, first match wins.
    pub session_env: Vec<String>,
    /// Env vars that mark "running inside this harness" when the process walk is blocked (sandboxes).
    pub marker_env: Vec<String>,
    /// Screen text shown while a turn is running; terminal wake drivers don't type over it.
    pub busy_text: Option<String>,
    /// Settings file (relative to the project) for Claude-Code-style command hooks; None = no hooks.
    pub hooks_file: Option<String>,
}

fn strs(v: &[&str]) -> Vec<String> {
    v.iter().map(|s| s.to_string()).collect()
}

pub fn builtins() -> Vec<Profile> {
    vec![
        Profile {
            name: "claude".into(),
            process_names: strs(&["claude"]),
            session_env: strs(&["CLAUDE_CODE_SESSION_ID"]),
            marker_env: strs(&["CLAUDECODE"]),
            busy_text: Some("esc to interrupt".into()),
            hooks_file: Some(".claude/settings.local.json".into()),
        },
        Profile {
            name: "codex".into(),
            process_names: strs(&["codex"]),
            session_env: strs(&["CODEX_SESSION_ID", "CODEX_THREAD_ID"]),
            marker_env: strs(&["CODEX_THREAD_ID", "CODEX_SANDBOX"]),
            busy_text: Some("esc to interrupt".into()),
            hooks_file: Some(".codex/hooks.json".into()),
        },
        Profile {
            name: "grok".into(),
            process_names: strs(&["grok"]),
            session_env: strs(&["GROK_SESSION_ID"]),
            marker_env: vec![],
            busy_text: None,
            hooks_file: None,
        },
    ]
}

/// Built-ins merged with the user file; a user profile with a built-in's name replaces it.
/// Marker env is checked in list order, so user profiles go first: a nested harness wins.
pub fn load() -> Vec<Profile> {
    let mut out: Vec<Profile> = user_profiles();
    for b in builtins() {
        if !out.iter().any(|p| p.name == b.name) {
            out.push(b);
        }
    }
    out
}

/// `~/.config/tincan/<name>` on every OS; home is HOME, else USERPROFILE (Windows).
pub fn config_file(name: &str) -> Option<std::path::PathBuf> {
    std::env::var_os("HOME")
        .filter(|h| !h.is_empty())
        .or_else(|| std::env::var_os("USERPROFILE"))
        .map(|h| {
            std::path::Path::new(&h)
                .join(".config")
                .join("tincan")
                .join(name)
        })
}

pub fn find(name: &str) -> Option<Profile> {
    load().into_iter().find(|p| p.name == name)
}

fn user_profiles() -> Vec<Profile> {
    let path = std::env::var_os("TINCAN_HARNESSES")
        .map(std::path::PathBuf::from)
        .or_else(|| config_file("harnesses.json"));
    let Some(text) = path.and_then(|p| std::fs::read_to_string(p).ok()) else {
        return vec![];
    };
    match serde_json::from_str::<Value>(&text) {
        Ok(Value::Array(items)) => items.iter().filter_map(parse).collect(),
        _ => vec![],
    }
}

/// `{"name": "opencode", "process_names": ["opencode"], "session_env": [...], "marker_env": [...],
///   "busy_text": "esc to interrupt", "hooks_file": null}` — only `name` is required.
fn parse(v: &Value) -> Option<Profile> {
    let list = |k: &str| -> Vec<String> {
        v.get(k)
            .and_then(Value::as_array)
            .map(|a| {
                a.iter()
                    .filter_map(|s| s.as_str().map(str::to_string))
                    .collect()
            })
            .unwrap_or_default()
    };
    let name = v.get("name")?.as_str()?.to_string();
    let mut process_names = list("process_names");
    if process_names.is_empty() {
        process_names.push(name.clone());
    }
    Some(Profile {
        process_names,
        session_env: list("session_env"),
        marker_env: list("marker_env"),
        busy_text: v
            .get("busy_text")
            .and_then(Value::as_str)
            .map(str::to_string),
        hooks_file: v
            .get("hooks_file")
            .and_then(Value::as_str)
            .map(str::to_string),
        name,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn parse_defaults_process_name_to_harness_name() {
        let p = parse(&json!({"name": "opencode"})).unwrap();
        assert_eq!(p.process_names, vec!["opencode"]);
        assert!(p.hooks_file.is_none());
    }

    #[test]
    fn parse_requires_name() {
        assert!(parse(&json!({"process_names": ["x"]})).is_none());
    }
}
