use serde_json::{Map, Value, json};

/// Attached to every no_team error. Teams are created on demand, so this means an unusable path,
/// or an agent running somewhere other than the machine that holds the team dir.
const LOCAL_ONLY_HINT: &str = "tincan could not open or create its team dir (normally the git repo root). \
Check the path is writable, or pass --team-dir. \
tincan is local-only: if you are a cloud or remote agent, ask the user to run this session locally instead.";
use std::fmt;

/// Every failure maps to one stable exit code and one machine-readable `error` string.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Code {
    Usage,
    NoTeam,
    PeerUnavailable,
    TooLarge,
    RoleTaken,
    NotRegistered,
    HopLimit,
    ReplyNotWanted,
    MailboxFull,
    Store,
}

impl Code {
    pub fn exit(self) -> i32 {
        match self {
            Code::Usage | Code::NoTeam => 2,
            Code::PeerUnavailable => 3,
            Code::TooLarge => 4,
            Code::RoleTaken => 5,
            Code::NotRegistered => 6,
            Code::HopLimit => 7,
            Code::ReplyNotWanted => 8,
            Code::MailboxFull => 9,
            Code::Store => 10,
        }
    }

    pub fn as_str(self) -> &'static str {
        match self {
            Code::Usage => "usage",
            Code::NoTeam => "no_team",
            Code::PeerUnavailable => "peer_unavailable",
            Code::TooLarge => "too_large",
            Code::RoleTaken => "role_taken",
            Code::NotRegistered => "not_registered",
            Code::HopLimit => "hop_limit",
            Code::ReplyNotWanted => "reply_not_wanted",
            Code::MailboxFull => "mailbox_full",
            Code::Store => "store",
        }
    }
}

#[derive(Debug)]
pub struct TincanError {
    pub code: Code,
    pub message: String,
    pub extra: Map<String, Value>,
}

impl TincanError {
    pub fn new(code: Code, message: impl Into<String>) -> Self {
        Self {
            code,
            message: message.into(),
            extra: Map::new(),
        }
    }

    pub fn with(mut self, key: &str, value: impl Into<Value>) -> Self {
        self.extra.insert(key.to_string(), value.into());
        self
    }

    pub fn to_json(&self) -> Value {
        let mut obj = json!({"ok": false, "error": self.code.as_str(), "message": self.message});
        if self.code == Code::NoTeam {
            obj["hint"] = LOCAL_ONLY_HINT.into();
        }
        obj.as_object_mut().unwrap().extend(self.extra.clone());
        obj
    }
}

impl fmt::Display for TincanError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}: {}", self.code.as_str(), self.message)
    }
}

impl From<rusqlite::Error> for TincanError {
    fn from(e: rusqlite::Error) -> Self {
        TincanError::new(Code::Store, e.to_string())
    }
}

pub type Result<T> = std::result::Result<T, TincanError>;
