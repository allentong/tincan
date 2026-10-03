//! Skills a sender can ask the recipient to use (`tincan send --skill NAME`), and the extra
//! tools a launched session gets for one. Grants live in the sender's own config
//! (`$TINCAN_SKILLS`, else `~/.config/tincan/skills.json`), never in the shared store:
//!
//! `{"shepherd-pr": {"claude": {"allow": ["Bash(gh pr view:*)"], "domains": ["api.github.com"]}}}`
//!
//! Keys are skill names; each maps harness names to that harness's tool rules (`allow`, or a
//! bare array as shorthand) and, for a sandboxed Claude, the network `domains` its commands
//! may reach. A skill with no entry still works: the recipient is told to use it, with the
//! default tools only.

use serde_json::Value;

/// Skill names as harnesses write them: `code-review`, `plugin:skill`. No leading `-`, so a
/// name can't be read as an option once it's in a launch argv.
pub fn valid_name(name: &str) -> bool {
    !name.is_empty()
        && name.len() <= 128
        && !name.starts_with('-')
        && name
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.' | ':'))
}

#[derive(Debug, Default, PartialEq)]
pub struct Grant {
    pub allow: Vec<String>,
    pub domains: Vec<String>,
    /// Credential paths a sandboxed Claude may read anyway (e.g. `~/.config/gh` for `gh`).
    pub read: Vec<String>,
}

/// What `harness` gets for `skill`, plus any config errors.
pub fn grant(skill: &str, harness: &str) -> (Grant, Vec<String>) {
    let (config, mut errors) = load();
    let Some(entry) = config.as_ref().and_then(|c| c.get(skill)?.get(harness)) else {
        return (Grant::default(), errors);
    };
    match parse_grant(entry) {
        Ok(grant) => (grant, errors),
        Err(e) => {
            errors.push(format!("skills.json: {skill:?}.{harness:?}: {e}"));
            (Grant::default(), errors)
        }
    }
}

fn parse_grant(v: &Value) -> Result<Grant, String> {
    let strings = |v: Option<&Value>, key: &str| -> Result<Vec<String>, String> {
        let Some(v) = v else { return Ok(vec![]) };
        v.as_array()
            .and_then(|items| {
                items
                    .iter()
                    .map(|i| i.as_str().map(str::to_string))
                    .collect::<Option<Vec<_>>>()
            })
            .ok_or(format!("{key:?} must be an array of strings"))
    };
    if v.is_array() {
        return Ok(Grant {
            allow: strings(Some(v), "allow")?,
            ..Grant::default()
        });
    }
    if !v.is_object() {
        return Err(
            "expected {\"allow\": [...], \"domains\": [...], \"read\": [...]} or an array".into(),
        );
    }
    let grant = Grant {
        allow: strings(v.get("allow"), "allow")?,
        domains: strings(v.get("domains"), "domains")?,
        read: strings(v.get("read"), "read")?,
    };
    if let Some(bad) = grant
        .read
        .iter()
        .find(|p| !(p.starts_with("~/") || p.starts_with('/')) || p.contains(".."))
    {
        return Err(format!("bad read path {bad:?}: use an absolute or ~/ path"));
    }
    // A domain lands in the sandbox's network allowlist: keep it to a plain host pattern.
    if let Some(bad) = grant.domains.iter().find(|d| {
        d.is_empty()
            || !d
                .chars()
                .all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '-' | '*'))
    }) {
        return Err(format!("bad domain {bad:?}"));
    }
    Ok(grant)
}

/// Skill names with grants, for `tincan extensions`.
pub fn configured() -> (Vec<String>, Vec<String>) {
    let (config, errors) = load();
    let names = config
        .and_then(|c| c.as_object().map(|o| o.keys().cloned().collect()))
        .unwrap_or_default();
    (names, errors)
}

fn load() -> (Option<Value>, Vec<String>) {
    let configured = std::env::var_os("TINCAN_SKILLS");
    let Some(path) = configured
        .clone()
        .map(std::path::PathBuf::from)
        .or_else(|| crate::harness::config_file("skills.json"))
    else {
        return (None, vec![]);
    };
    let text = match std::fs::read_to_string(&path) {
        Ok(text) => text,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound && configured.is_none() => {
            return (None, vec![]);
        }
        Err(e) => return (None, vec![format!("{}: {e}", path.display())]),
    };
    match serde_json::from_str::<Value>(&text) {
        Ok(v @ Value::Object(_)) => (Some(v), vec![]),
        Ok(_) => (
            None,
            vec![format!("{}: expected a JSON object", path.display())],
        ),
        Err(e) => (None, vec![format!("{}: {e}", path.display())]),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn names() {
        assert!(valid_name("code-review"));
        assert!(valid_name("tincan:consult"));
        assert!(!valid_name(""));
        assert!(!valid_name("--dangerously-skip-permissions"));
        assert!(!valid_name("a b"));
        assert!(!valid_name("x)"));
        assert!(!valid_name("../x"));
    }

    #[test]
    fn grant_forms() {
        use serde_json::json;
        assert_eq!(
            parse_grant(&json!(["Bash(gh pr view:*)"])).unwrap().allow,
            ["Bash(gh pr view:*)"]
        );
        let g = parse_grant(&json!({"allow": ["X"], "domains": ["*.github.com"]})).unwrap();
        assert_eq!(
            (g.allow, g.domains),
            (vec!["X".into()], vec!["*.github.com".into()])
        );
        assert!(parse_grant(&json!({"domains": ["evil.com\"],\"x"]})).is_err());
        assert!(parse_grant(&json!({"allow": "X"})).is_err());
        assert!(parse_grant(&json!("X")).is_err());
        assert!(parse_grant(&json!({"read": ["relative"]})).is_err());
        assert!(parse_grant(&json!({"read": ["~/../etc"]})).is_err());
        assert_eq!(
            parse_grant(&json!({"read": ["~/.config/gh"]}))
                .unwrap()
                .read,
            ["~/.config/gh"]
        );
    }
}
