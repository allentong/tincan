//! tincan — local, daemonless message bus between agent sessions (Claude Code, Codex, Grok).
//! Store: <team>/.tincan/tincan.db (SQLite WAL), holding only mail still in flight.
//! Every command prints one JSON object.

mod commands;
mod error;
mod harness;
mod hooks;
mod identity;
mod store;
mod wake;

use clap::{Parser, Subcommand};
use error::{Code, TincanError};

#[derive(Parser)]
#[command(
    name = "tincan",
    version,
    about = "Message other agent sessions on the same team floor"
)]
pub struct Cli {
    /// Team directory (else TINCAN_TEAM_DIR, else nearest ancestor with .tincan/)
    #[arg(long, global = true)]
    pub team_dir: Option<String>,
    /// Act as this role (else TINCAN_ROLE, else inferred from the harness session)
    #[arg(long = "as", global = true)]
    pub as_role: Option<String>,
    #[command(subcommand)]
    pub cmd: Cmd,
}

#[derive(Subcommand)]
pub enum Cmd {
    /// Create .tincan/ in the team dir
    Init,
    /// Claim a role for this session
    Register {
        role: String,
        #[arg(long)]
        harness: Option<String>,
        /// Owner PID to bind liveness to; 0 = heartbeat only (one-shot harnesses)
        #[arg(long)]
        pid: Option<i64>,
        /// How senders wake this session when idle: none (default; hooks/wait), auto,
        /// tmux:PANE, cmux:SURFACE, or cmd:SHELL
        #[arg(long)]
        wake: Option<String>,
    },
    /// Release this session's role
    Unregister,
    /// Show the resolved role and unread count
    Whoami,
    /// List peers (active only unless --all)
    Peers {
        #[arg(long)]
        all: bool,
    },
    /// Send a DM (`<role>`) or broadcast (`'*'`); body `-` reads stdin
    Send {
        to: String,
        body: String,
        /// Idempotency key: resending with the same key returns the original message
        #[arg(long)]
        client_id: Option<String>,
        #[arg(long)]
        reply_to: Option<String>,
        /// FYI only: replies to this message are rejected
        #[arg(long)]
        no_reply: bool,
    },
    /// Read unread messages
    Inbox {
        /// Show without leasing or marking read
        #[arg(long)]
        peek: bool,
        /// Print the unread count only
        #[arg(long)]
        count: bool,
        /// Lease messages and leave them unread until `tincan ack`
        #[arg(long)]
        require_ack: bool,
    },
    /// Mark leased messages read
    Ack { ids: Vec<String> },
    /// Block until a message is available or the timeout passes
    Wait {
        #[arg(long, default_value_t = 300.0, value_parser = seconds)]
        timeout: f64,
        /// Wait until every recipient of this message has replied (fan-out gather)
        #[arg(long)]
        replies_to: Option<String>,
    },
    /// Harness hook entry point (event also read from stdin); never fails the turn
    Hook {
        #[arg(long, default_value = "UserPromptSubmit")]
        event: String,
        /// Stop only: wait up to this many seconds for replies to this session's open requests
        #[arg(long, default_value_t = 0.0, value_parser = seconds)]
        linger: f64,
    },
    /// List loaded harness profiles and wake drivers, with any config errors
    Extensions,
    /// Print the hook config for a harness (merge it into the file it names)
    Hooks {
        #[arg(long)]
        harness: String,
    },
}

fn main() {
    let cli = match Cli::try_parse() {
        Ok(cli) => cli,
        Err(e)
            if matches!(
                e.kind(),
                clap::error::ErrorKind::DisplayHelp | clap::error::ErrorKind::DisplayVersion
            ) =>
        {
            e.exit()
        }
        Err(e) => fail(TincanError::new(
            Code::Usage,
            e.to_string().trim().to_string(),
        )),
    };
    match commands::run(cli) {
        Ok(Some(v)) => println!("{}", with_setup(v)),
        Ok(None) => {}
        Err(e) => fail(e),
    }
}

/// Anything tincan set up on its own (a team, a registration) is confirmed in the output.
fn with_setup(mut v: serde_json::Value) -> serde_json::Value {
    if let (Some(obj), Some(setup)) = (v.as_object_mut(), store::take_notes()) {
        obj.insert("setup".into(), setup.into());
    }
    v
}

fn fail(e: TincanError) -> ! {
    println!("{}", with_setup(e.to_json()));
    std::process::exit(e.code.exit());
}

/// Durations must be finite and non-negative: a NaN deadline would never pass.
fn seconds(s: &str) -> std::result::Result<f64, String> {
    match s.parse::<f64>() {
        Ok(v) if v.is_finite() && v >= 0.0 => Ok(v),
        _ => Err(format!("{s:?} is not a non-negative number of seconds")),
    }
}
