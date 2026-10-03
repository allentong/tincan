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
    /// argv that starts a quick headless session to answer mail sent to this harness's name
    /// while none is running. Placeholders: {prompt} {team} {store} {workspace} {role} {sender}
    /// {message_id} {sandbox}.
    pub launch: Option<Vec<String>>,
    /// Appended once per tool rule a `--skill` grants, with {tool} filled in.
    pub grant_argv: Option<Vec<String>>,
    /// Additional environment variables explicitly passed to a launched session. The default
    /// launch environment contains only paths, locale, and platform runtime variables.
    pub pass_env: Vec<String>,
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
            // --strict-mcp-config without --mcp-config loads no MCP servers; --setting-sources
            // user skips project settings but still loads the user's allow rules and hooks.
            launch: Some(strs(&[
                "claude",
                "-p",
                "{prompt}",
                "--permission-mode",
                "acceptEdits",
                "--setting-sources",
                "user",
                "--strict-mcp-config",
                "--settings",
                "{sandbox}",
                "--allowedTools",
                "Bash(tincan inbox:*)",
                "Bash(tincan reply:*)",
                "Bash(tincan send:*)",
                "Bash(tincan wait:*)",
                "Bash(tincan ack:*)",
                "Bash(tincan whoami:*)",
                "Bash(tincan peers:*)",
                "Bash(tincan unregister:*)",
                "Bash(git status:*)",
                "Bash(git diff:*)",
                "Bash(git log:*)",
                "Bash(git show:*)",
            ])),
            grant_argv: Some(strs(&["--allowedTools", "{tool}"])),
            pass_env: vec![],
        },
        Profile {
            name: "codex".into(),
            process_names: strs(&["codex"]),
            session_env: strs(&["CODEX_SESSION_ID", "CODEX_THREAD_ID"]),
            marker_env: strs(&["CODEX_THREAD_ID", "CODEX_SANDBOX"]),
            busy_text: Some("esc to interrupt".into()),
            hooks_file: Some(".codex/hooks.json".into()),
            // workspace-write: the team store lives in the repo, and tincan must write to it.
            launch: Some(strs(&[
                "codex",
                "exec",
                "--skip-git-repo-check",
                "-s",
                "workspace-write",
                "-C",
                "{workspace}",
                "--add-dir",
                "{store}",
                "{prompt}",
            ])),
            grant_argv: None,
            pass_env: vec![],
        },
        Profile {
            name: "grok".into(),
            process_names: strs(&["grok"]),
            session_env: strs(&["GROK_SESSION_ID"]),
            marker_env: strs(&["GROK_SESSION_ID"]),
            busy_text: Some("[stop]".into()),
            hooks_file: Some(".grok/hooks/tincan.json".into()),
            launch: Some(strs(&["grok", "-p", "{prompt}"])),
            grant_argv: None,
            pass_env: vec![],
        },
    ]
}

/// Built-ins merged with the user file; a user profile with a built-in's name replaces it.
/// Marker env is checked in list order, so user profiles go first: a nested harness wins.
pub fn load() -> Vec<Profile> {
    load_with_errors().0
}

pub fn load_with_errors() -> (Vec<Profile>, Vec<String>) {
    let (mut out, errors) = user_profiles();
    for b in builtins() {
        match out.iter_mut().find(|p| p.name == b.name) {
            // An override written before a built-in field existed keeps the built-in's.
            Some(p) => p.grant_argv = p.grant_argv.take().or(b.grant_argv),
            None => out.push(b),
        }
    }
    (out, errors)
}

/// Home is HOME, else USERPROFILE (Windows).
pub fn home() -> Option<std::path::PathBuf> {
    std::env::var_os("HOME")
        .filter(|h| !h.is_empty())
        .or_else(|| std::env::var_os("USERPROFILE"))
        .map(std::path::PathBuf::from)
}

/// `~/.config/tincan/<name>` on every OS.
pub fn config_file(name: &str) -> Option<std::path::PathBuf> {
    home().map(|h| h.join(".config").join("tincan").join(name))
}

pub fn find(name: &str) -> Option<Profile> {
    load().into_iter().find(|p| p.name == name)
}

/// A JSON config file: `$env`, else `~/.config/tincan/<file>`. A missing default file is no
/// config; a missing file named by `$env` is an error.
pub fn read_config(env: &str, file: &str) -> Result<Option<(std::path::PathBuf, Value)>, String> {
    let configured = std::env::var_os(env);
    let Some(path) = configured
        .clone()
        .map(std::path::PathBuf::from)
        .or_else(|| config_file(file))
    else {
        return Ok(None);
    };
    let text = match std::fs::read_to_string(&path) {
        Ok(text) => text,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound && configured.is_none() => {
            return Ok(None);
        }
        Err(e) => return Err(format!("{}: {e}", path.display())),
    };
    serde_json::from_str(&text)
        .map(|v| Some((path.clone(), v)))
        .map_err(|e| format!("{}: {e}", path.display()))
}

/// An optional array of strings.
pub fn string_list(v: Option<&Value>, key: &str) -> Result<Vec<String>, String> {
    let Some(value) = v else {
        return Ok(vec![]);
    };
    value
        .as_array()
        .and_then(|items| {
            items
                .iter()
                .map(|i| i.as_str().map(str::to_string))
                .collect::<Option<Vec<_>>>()
        })
        .ok_or_else(|| format!("{key:?} must be an array of strings"))
}

fn user_profiles() -> (Vec<Profile>, Vec<String>) {
    let (path, value) = match read_config("TINCAN_HARNESSES", "harnesses.json") {
        Ok(Some(found)) => found,
        Ok(None) => return (vec![], vec![]),
        Err(e) => return (vec![], vec![e]),
    };
    match value {
        Value::Array(items) => {
            let mut profiles = vec![];
            let mut errors = vec![];
            for item in &items {
                match parse(item) {
                    Ok(profile) => profiles.push(profile),
                    Err(e) => errors.push(format!("{}: {e}", path.display())),
                }
            }
            (profiles, errors)
        }
        _ => (
            vec![],
            vec![format!("{}: expected a JSON array", path.display())],
        ),
    }
}

fn parse(v: &Value) -> Result<Profile, String> {
    let list = |key: &str| string_list(v.get(key), key);
    // `null` or `[]` means "none", as if the key were absent.
    let optional = |key: &str| -> Result<Option<Vec<String>>, String> {
        match v.get(key) {
            None | Some(Value::Null) => Ok(None),
            _ => Ok(Some(list(key)?).filter(|items| !items.is_empty())),
        }
    };
    let name = v
        .get("name")
        .and_then(Value::as_str)
        .filter(|name| !name.is_empty())
        .ok_or_else(|| "harness needs a non-empty \"name\"".to_string())?
        .to_string();
    if !crate::identity::valid_token(&name, 64, &['-', '_', '.']) {
        return Err(format!(
            "invalid harness name {name:?}: use 1-64 letters, digits, '-', '_' or '.'"
        ));
    }
    let mut process_names = list("process_names")?;
    if process_names.is_empty() {
        process_names.push(name.clone());
    }
    let launch = optional("launch")?;
    let grant_argv = optional("grant")?;
    Ok(Profile {
        process_names,
        session_env: list("session_env")?,
        marker_env: list("marker_env")?,
        busy_text: v
            .get("busy_text")
            .and_then(Value::as_str)
            .map(str::to_string),
        hooks_file: v
            .get("hooks_file")
            .and_then(Value::as_str)
            .map(str::to_string),
        launch,
        grant_argv,
        pass_env: list("pass_env")?,
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
        assert!(p.pass_env.is_empty());
    }

    #[test]
    fn parse_explicit_launch_environment() {
        let p = parse(&json!({"name": "opencode", "pass_env": ["OPENROUTER_API_KEY"]})).unwrap();
        assert_eq!(p.pass_env, vec!["OPENROUTER_API_KEY"]);
    }

    #[test]
    fn launched_claude_is_sandboxed_and_skips_repo_settings() {
        let claude = builtins().into_iter().find(|p| p.name == "claude").unwrap();
        let argv = claude.launch.unwrap().join(" ");
        assert!(argv.contains("--setting-sources user"), "{argv}");
        assert!(argv.contains("--settings {sandbox}"), "{argv}");
        for gone in ["cargo", "npm", "pytest", "git commit", "git add"] {
            assert!(!argv.contains(gone), "{gone} in {argv}");
        }
        assert_eq!(claude.grant_argv, Some(strs(&["--allowedTools", "{tool}"])));
    }

    #[test]
    fn parse_requires_name() {
        assert!(parse(&json!({"process_names": ["x"]})).is_err());
        assert!(parse(&json!({"name": "../../escape"})).is_err());
    }
}
