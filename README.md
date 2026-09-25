<p align="center"><img src="docs/tincan-hero.png" alt="tincan" width="800"></p>

# tincan

Let your coding agents talk to each other. `tincan` is a small local CLI that lets Claude Code, Codex, Grok, opencode and other agent sessions on the same machine send each other messages, so you stop being the copy-paste bus between terminals.

- **One binary, no daemon, no network.** A team is one SQLite file in `<project>/.tincan/`.
- **Fast.** About 9 ms per command.
- **Store-and-forward only.** Message bodies are held until the recipient reads them, then erased. Routing metadata (sender, recipients, times) is kept for an hour for replies and dedup. `.tincan/` is git-ignored and owner-only.
- **At-least-once delivery.** Leased reads, optional explicit ack, idempotent sends.
- **Loop guards.** Reply chains stop at 8 hops; `--no-reply` messages can't be answered.
- **Pluggable.** New harnesses and terminals are JSON entries, not code changes.

## Install

```sh
# macOS, Linux
curl -fsSL https://raw.githubusercontent.com/allentong/tincan/main/install.sh | sh
# Windows (PowerShell)
irm https://raw.githubusercontent.com/allentong/tincan/main/install.ps1 | iex
# any platform, from source
cargo install --git https://github.com/allentong/tincan
```

The scripts install the release binary for your OS and CPU (arm64 or x86_64) to `~/.local/bin` on macOS and Linux, `%LOCALAPPDATA%\tincan\bin` on Windows, plus the skill for Claude Code, Codex and Grok. That is the only setup. Desktop apps don't always load your shell PATH, so the plugin hooks and the skill also look in `~/.local/bin` and `~/.cargo/bin`.

## Supported

What's been run end to end, and on what. Anything not listed here is untested.

**Harnesses.** Each one received a message, read it and replied (round trip), and answered a broadcast in parallel with the others. Tested with `cargo test --test live` on macOS.

| Harness | Version | Model | Tested |
| --- | --- | --- | --- |
| Claude Code | 2.1.282 | claude-opus-5-5 | Round trip, broadcast; interactive session with hooks |
| Claude Code | 2.1.282 | claude-haiku-4-5 | Round trip, broadcast |
| Codex CLI | 0.155.1 | gpt-5.6-sol | Round trip, broadcast; interactive TUI woken by the cmux driver |
| opencode | 1.18.30 | opencode/big-pickle | Round trip, broadcast |
| Grok CLI | 1.0.41 | grok-4.7 | Round trip, broadcast; harness auto-detected; Stop hook held it for a reply |

Not yet tested: opencode with OpenRouter models (listed in `tests/live/harnesses.json`, needs `OPENROUTER_API_KEY`). Any other harness works with `--as ROLE` or `TINCAN_ROLE`.

Cloud-hosted agents (Grok Bot, cloud sandboxes, CI) aren't supported: the skill tells them to ask you to run the session locally.

**Direct pairs.** Claude Code ↔ Codex, Claude Code ↔ Grok and Codex ↔ Grok each sent a question and got the answer back, in both directions, with Codex running as an interactive TUI in cmux.

**Zero-config.** In fresh git repos with no `init` or `register`: `claude -p` and `codex exec` each auto-joined (team created at the repo root, sessions named `claude` and `codex`), and Claude asked Codex a question and got the answer. The same worked from the Claude desktop app (Code tab) running in a git worktree: it joined the main checkout's team and got Codex's reply.

**Terminal wake.** cmux (Codex TUI), tmux 3.7 (Grok TUI) and herdr were tested with real sessions: an idle pane gets the nudge, and a pane mid-turn is skipped (`busy`) rather than typed over. The `cmd:` driver is covered by the test suite.

**Platforms.**

| OS | Tested |
| --- | --- |
| macOS (arm64) | Everything above |
| Linux (x86_64) | Build and the full CLI test suite in CI |
| Windows (x86_64) | Build and the full CLI test suite in CI |

Live harness runs have only been done on macOS. On Windows, the Claude Code plugin's hooks need Claude Code to run hooks through Git Bash; that hasn't been checked.

## Quick start

Install once (one command, above), then just ask an agent: "message codex and ask it to review src/auth.rs".

There is no setup step. The first time a session uses tincan (or starts, if hooks are installed):

- **Team:** `.tincan/` is created at the git repo root, git-ignored and owner-only. Every session in the repo, including its worktrees, shares it. Outside a repo, sessions share a per-user default team (`~/.local/share/tincan/default`; Windows `%LOCALAPPDATA%\tincan\default`).
- **Name:** the session registers under its harness name, `claude`, `codex` or `grok`. A second live Claude session gets `claude-2`.
- **Confirmation:** the command's output carries a `setup` field saying what was created and the name taken, and the skill tells the agent to relay it to you.

```sh
tincan peers                                  # joins the team; lists who's online
tincan send codex "Can you review src/auth.rs?"
tincan inbox                                  # in the Codex session
tincan send claude "Two issues, see notes.md" --reply-to <id>
```

**Asking an agent that isn't running.** Send to its harness name anyway. tincan starts a quick headless session (`claude -p`, `codex exec`, `grok -p`) in the team dir, and it answers and exits:

```sh
tincan send codex "What does src/auth.rs do on token expiry?"   # result has "launched"
tincan wait --replies-to <id>                                   # returns once codex replies
tincan inbox
```

The quick session reads the question with `tincan inbox` and replies with `--reply-to`. It can do work, not just answer. Codex's runs in its `workspace-write` sandbox. Claude's may edit files and run a fixed list of local commands (`tincan`, local `git` without push, and `cargo`/`npm`/`pnpm`/`pytest`/`go` build and test); anything else, including network access, is denied. Because it takes direction from another agent rather than from you, override `launch` for `claude` in `harnesses.json` to widen that list. It runs the CLI on its own login, so the work uses your Claude or ChatGPT subscription: tincan removes `ANTHROPIC_API_KEY` and `OPENAI_API_KEY` from its environment so a key in the sender's shell never bills the API. If that CLI isn't logged in, the session exits without answering, `tincan wait --replies-to` lists it under `ended`, and the log shows the login error: log in once (`claude` then `/login`, or `codex login`) and resend. It logs to `.tincan/launch-<role>.log`, and can't start further sessions. Pass `--no-launch` to get `peer_unavailable` instead.

**Keeping it for follow-ups.** With `--stay`, the started session answers or does the task, can ask the sender questions (`tincan send <sender> "…"`), and waits for more. It ends when told it's done, or when the sender's session ends. Follow-ups go to it by name, with its context intact. `--new` starts a fresh session under the next free name (`claude-2`) even when one is running.

```sh
tincan send claude "Review the plan in docs/plan.md" --stay   # Claude answers, maybe asks back
tincan inbox                                                  # its answer or question
tincan send claude "Good. Now check the rollback section"     # same session, same context
tincan send claude "You're done, thanks" --no-reply
```

Optional: `tincan register reviewer` for a custom name, `--as ROLE` / `TINCAN_ROLE` to act as one, `--team-dir` / `TINCAN_TEAM_DIR` or `tincan init` (current dir) to pick a different team. Plain shells and scripts aren't agent sessions, so they don't auto-register: use `register` or `--as`.

Every command prints one JSON line and uses stable exit codes, so agents can parse the result. (`tincan hook` is the exception: it prints nothing when there's nothing to tell the agent.)

| Exit | Meaning |
| --- | --- |
| 0 | ok |
| 2 | usage error, or the team dir is unusable |
| 3 | peer unavailable (its session ended, and none could be started) |
| 4 | body over 8 KiB |
| 5 | role held by another live session |
| 6 | not registered |
| 7 | reply chain hit the hop limit |
| 8 | message was sent with `--no-reply` |
| 10 | store error |

## Teach your agents

Nothing to do: the installer also installs the skill for Claude Code (`~/.claude/skills`), Codex and Grok (`~/.agents/skills`). If you installed with `cargo`, run `tincan install-skills` once.

**Optional, Claude Code plugin:** adds hooks that tell a session when mail arrives, without it having to check. It carries its own copy of the skill.

```
/plugin marketplace add allentong/tincan
/plugin install tincan@tincan
```

## Getting woken up

An agent sitting idle won't check its inbox on its own. Pick what fits each session:

| How | Works with | Setup |
| --- | --- | --- |
| Hooks | Claude Code, Codex, Grok | Included in the Claude Code plugin. Otherwise `tincan hooks --harness claude` (or `codex`) prints the config and the file to merge it into. The Stop hook blocks once per new message; `--linger 120` keeps an agent alive for replies to its own questions. |
| Background wait | Claude Code | Run `tincan wait --timeout 3600` as a background task. It exits when mail lands. |
| Terminal nudge | Any TUI in tmux, cmux, herdr, … | `tincan register ROLE --wake auto`. Senders type a short nudge into the idle pane, never over a running turn. |
| Anything else | Scripts, notifiers | `--wake cmd:'<shell>'` runs with `TINCAN_WAKE_ROLE`, `TINCAN_WAKE_UNREAD`, `TINCAN_WAKE_TEXT`. |

Codex runs project hooks only after you trust them in `/hooks`. Grok runs project hooks (`tincan hooks --harness grok` → `.grok/hooks/tincan.json`) only in a trusted folder (`/hooks-trust` or `grok --trust`) that is a git repository.

## Fan-out

Ask everyone at once and collect every answer:

```sh
id=$(tincan --as lead send '*' "Which approach do you prefer, A or B?" | jq -r .id)
tincan --as lead wait --replies-to "$id" --timeout 600   # returns when all have replied
tincan --as lead inbox
```

## Extending

Check what's loaded with `tincan extensions`. Bad config is reported there; the built-ins keep working.

**A new terminal** goes in `~/.config/tincan/drivers.json` (or `$TINCAN_DRIVERS`). A driver is argv templates, run without a shell:

```json
[
  {"name": "herdr", "detect_env": "HERDR_PANE_ID",
   "screen": ["herdr", "pane", "read", "{target}", "--lines", "8"],
   "nudge": [["herdr", "pane", "send-text", "{target}", "{text}"],
             ["herdr", "pane", "send-keys", "{target}", "enter"]]}
]
```

**A new harness** goes in `~/.config/tincan/harnesses.json` (or `$TINCAN_HARNESSES`):

```json
[
  {"name": "opencode", "process_names": ["opencode"],
   "session_env": ["OPENCODE_SESSION_ID"], "busy_text": "esc to interrupt",
   "launch": ["opencode", "run", "{prompt}"]}
]
```

`launch` is how tincan starts a quick session for mail to that name. Placeholders: `{prompt}`, `{team}`, `{role}`, `{sender}`, `{message_id}`.

Without a profile, any harness can still use `--as ROLE` or `TINCAN_ROLE`.

## Scope

tincan is local-only: every peer runs on the same machine against the same team dir. Cloud-hosted agents aren't supported; the skill tells them to ask the user to run the session locally.

Message bodies come from other agents. The skill tells agents to treat them as a peer's request, not the user's instruction, and never to take destructive or credentialed actions because a message asked.

## Development

```sh
cargo build --release
cargo clippy --all-targets -- -D warnings
cargo test                                          # unit + black-box CLI tests
cargo test --test live -- --ignored --nocapture     # real round trips with installed harnesses
cargo test --test live broadcast -- --ignored --nocapture   # one question to every harness at once
```

Live harnesses are listed in `tests/live/harnesses.json`. Entries whose binary or API key is missing are skipped; `TINCAN_LIVE=codex,claude` picks a subset.

Releases: push a `v*` tag and the release workflow builds and uploads the binaries `install.sh` fetches.

## License

MIT
