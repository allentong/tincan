use crate::harness::{read_config, string_list};
use crate::identity::valid_token;
use serde_json::Value;

pub fn valid_name(name: &str) -> bool {
    !name.starts_with('-') && valid_token(name, 128, &['-', '_', '.', ':'])
}

#[derive(Debug, Default, PartialEq)]
pub struct Grant {
    pub allow: Vec<String>,
    pub domains: Vec<String>,
    pub allow_read: Vec<String>,
}

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
    if v.is_array() {
        return Ok(Grant {
            allow: string_list(Some(v), "allow")?,
            ..Grant::default()
        });
    }
    if !v.is_object() {
        return Err(
            "expected {\"allow\": [...], \"domains\": [...], \"read\": [...]} or an array".into(),
        );
    }
    let grant = Grant {
        allow: string_list(v.get("allow"), "allow")?,
        domains: string_list(v.get("domains"), "domains")?,
        allow_read: string_list(v.get("read"), "read")?,
    };
    if let Some(bad) = grant
        .allow_read
        .iter()
        .find(|p| !(p.starts_with("~/") || p.starts_with('/')) || p.contains(".."))
    {
        return Err(format!("bad read path {bad:?}: use an absolute or ~/ path"));
    }
    if let Some(bad) = grant
        .domains
        .iter()
        .find(|d| !valid_token(d, 253, &['.', '-', '*']))
    {
        return Err(format!("bad domain {bad:?}"));
    }
    Ok(grant)
}

pub fn granted_skill_names() -> (Vec<String>, Vec<String>) {
    let (config, errors) = load();
    let names = config
        .and_then(|c| c.as_object().map(|o| o.keys().cloned().collect()))
        .unwrap_or_default();
    (names, errors)
}

fn load() -> (Option<Value>, Vec<String>) {
    match read_config("TINCAN_SKILLS", "skills.json") {
        Ok(Some((_, v @ Value::Object(_)))) => (Some(v), vec![]),
        Ok(Some((path, _))) => (
            None,
            vec![format!("{}: expected a JSON object", path.display())],
        ),
        Ok(None) => (None, vec![]),
        Err(e) => (None, vec![e]),
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
                .allow_read,
            ["~/.config/gh"]
        );
    }
}
