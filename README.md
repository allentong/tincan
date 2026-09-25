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

The scripts install the release binary for your OS and CPU (arm64 or x86_64): `~/.local/bin` on macOS and Linux, `%LOCALAPPDATA%\tincan\bin` on Windows.

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

**Terminal wake.** cmux (Codex TUI), tmux 3.7 (Grok TUI) and herdr were tested with real sessions: an idle pane gets the nudge, and a pane mid-turn is skipped (`busy`) rather than typed over. The `cmd:` driver is covered by the test suite.

**Platforms.**

| OS | Tested |
| --- | --- |
| macOS (arm64) | Everything above |
| Linux (x86_64) | Build and the full CLI test suite in CI |
| Windows (x86_64) | Build and the full CLI test suite in CI |

Live harness runs have only been done on macOS. On Windows, the Claude Code plugin's hooks need Claude Code to run hooks through Git Bash; that hasn't been checked.

## Quick start

```sh
cd my-project
tincan init        # or let an agent do it: "set up tincan here"

# in the Claude Code session
tincan register lead
# in the Codex session
tincan register reviewer

tincan --as lead send reviewer "Can you review the diff in src/auth.rs?"
tincan --as reviewer inbox
tincan --as reviewer send lead "Two issues, see notes.md" --reply-to <id>
```

Every command prints one JSON line and uses stable exit codes, so agents can parse the result. (`tincan hook` is the exception: it prints nothing when there's nothing to tell the agent.)

| Exit | Meaning |
| --- | --- |
| 0 | ok |
| 2 | usage error, or no team found |
| 3 | peer unavailable (its session ended) |
| 4 | body over 8 KiB |
| 5 | role held by another live session |
| 6 | not registered |
| 7 | reply chain hit the hop limit |
| 8 | message was sent with `--no-reply` |
| 10 | store error |

## Teach your agents

**Claude Code:** install the plugin. It adds the skill plus hooks that tell the agent when mail arrives.

```
/plugin marketplace add allentong/tincan
/plugin install tincan@tincan
```

**Codex and others:** copy the skill to where the harness looks for skills:

```sh
mkdir -p ~/.agents/skills/tincan
cp skills/tincan/SKILL.md ~/.agents/skills/tincan/    # Codex (or .agents/skills/ per project)
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
   "session_env": ["OPENCODE_SESSION_ID"], "busy_text": "esc to interrupt"}
]
```

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
