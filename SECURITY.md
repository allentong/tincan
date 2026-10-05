# Security policy

## Reporting a vulnerability

Please use GitHub's private security-advisory flow for this repository instead of opening a public issue. Include the affected version, impact, and a minimal reproduction when possible.

## Trust model

Tincan is a local coordination tool for sessions running as the same operating-system user. A role such as `claude` or `codex` is a routing label, not an authenticated identity. Any process that can access a team's `.tincan` directory can read or modify its SQLite database, impersonate a role, or inspect pending mail. Do not send credentials, secrets, or data that another local agent or process must not see.

Agent messages are untrusted peer requests. They do not grant user authority for destructive, outward-facing, privileged, or credentialed actions. User-owned harness profiles (`harnesses.json`), wake drivers (`drivers.json`) and skill grants (`skills.json`) are trusted executable configuration. They're read from the sending user's `~/.config/tincan`, never from the team store.

A launched session can write the team store, so nothing read from the store is executed or trusted as configuration. A peer's wake spec only names a driver from the sender's own `drivers.json` and a target, and the target is validated again before use. (The `cmd:` wake kind, which ran a shell command stored with the peer, was removed for this reason.) A message's `--skill` is a name passed to the recipient's prompt. The tools a launched session gets for that skill come from the sender's `skills.json`.

The boundary for a launched session is its harness's sandbox, not tincan's own checks. Those checks (no `--as`, `--team-dir`, `register` or `--new` inside a launched session) are guardrails against agent mistakes; a process can unset the environment that triggers them.
- **Codex** runs in `workspace-write` with the team store added.
- **Claude Code**, on macOS and Linux, runs its shell commands in Claude Code's sandbox:
  - writes only in the workspace and the team store, never `~/.config/tincan`
  - no reads of common credential stores, by shell commands or by its Read tool
  - no network except domains a requested skill is granted
  - no unsandboxed retry
  - no web tools or MCP servers
  - it refuses to start without the sandbox
  - it doesn't load the repository's `.claude` settings, but it does load the user's, whose allow rules, `excludedCommands` and hooks apply
- **Granted domains** are open to every command in that session. Combined with a `read` grant for a credential, they give the session that credential's authority on those hosts.
- **The home directory:** tincan refuses to launch a session whose workspace is the home directory or `/`, where workspace writes would reach shell and tool configuration.
- **Wake:** a peer row written straight into the store with an invalid role is never nudged. Nudge text contains no shell metacharacters that run anything, but a planted wake target can still type that fixed line plus Enter into another pane of the user's terminal.
- **Native Windows** has no Claude sandbox, so a launched Claude there is limited to edits plus tincan and read-only git commands. `git diff`, `git log` and `git show` accept `--output=<file>` and can still write outside the workspace.
- **Grok** has no sandbox flag; a launched Grok runs with Grok's own defaults.

Headless-agent stdout and stderr are written to owner-only `.tincan/launch-<role>.log` files. A later launch of the same role truncates its prior log, but otherwise logs persist and can contain message or tool output. Remove them when their diagnostic value ends.

The project rejects symlinked team stores and database files, opens launch logs without following final symlinks, applies owner-only Unix permissions, clears ambient child-process environment variables, and bounds message bodies, pending queues, and inbox batches. These controls reduce accidental exposure and resource exhaustion; they do not create isolation from another process with the same filesystem authority.

## Supported versions

Security fixes are made on the latest release and the `main` branch.
