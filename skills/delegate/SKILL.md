---
name: delegate
description: Hand off a coding task to another agent (Claude Code, Codex, Grok) through tincan; it edits the shared repo while you direct it until the work is done, answering its questions and checking the result. Use when the user asks to delegate, hand off, or send work to another agent, e.g. "have Claude do this" from Codex.
---

# Delegate work to another agent

Start another agent on a task and stay connected: it does the work in this repo, asks you when it needs something, and reports back. You direct it and check the result; you finish it by telling it it's done. It runs on the `tincan` CLI; if `tincan` isn't found, follow the tincan skill's "Getting started".

For a quick opinion instead of work, use tincan's consult skill.

## 1. Pick the agent

Use the one the user named (for example "send it to Claude"). Otherwise ask; don't guess who should do the work.

Run `tincan peers`. If that agent's name is already taken by a running session, don't hand work to it: that session may be the user's. Add `--new` in step 3 so the task goes to a fresh session of its own.

Before sending, note the starting point so you can tell the worker's changes from what was already there: `git status --short`, `git diff HEAD --stat`, and the branch and commit (`git rev-parse --abbrev-ref HEAD`, `git rev-parse --short HEAD`). If the user has uncommitted work in files the task touches, ask them before going ahead.

## 2. Write the brief

The worker starts cold. Include:

```
## Task
<What to build or fix, and why.>

## Where
<Files and directories to start from. Commands that build and test it.>

## Done means
<The checks that must pass. Whether to commit.>

## Limits
<What not to touch. No pushing, deploying or anything outside the repo.>

Ask me with `tincan send <your role>` if anything is unclear or you need a decision.
When you're done, reply to this message with: the files you changed or added,
the commands and tests you ran and their results, anything left unresolved,
and whether you committed.
```

Send it on stdin (`tincan send <agent> - <<'EOF'` … `EOF`) so quotes and backticks survive. Keep it under 8 KiB; for a long spec, write a file in the repo and point at it.

## 3. Send

```sh
tincan send <agent> "<brief>" --stay          # or --stay --new, per step 1
```

The result must have a `launched` field; if it doesn't, the message went to an existing session instead, so tell the user and stop. `--stay` keeps the worker alive for follow-ups until you say it's done. Note the message `id`, `launched.role` (e.g. `claude` or `claude-2`) and `launched.log`, and tell the user in one line that the work was handed off, to whom.

## 4. Direct it

Loop until the task is finished:

```sh
tincan wait --timeout 120
```

Give your shell tool a timeout of at least 180 seconds. Each time it times out, run `tincan peers`: if the worker's role is gone, its session ended, so go to "If the worker stops" below. Otherwise wait again, and every few rounds tell the user in one line that the worker is still going. On mail, run `tincan inbox` and handle only messages whose `from` is the worker's role; deal with any other mail separately:

- **A question:** if the brief, the user's request or the code already decides it (an implementation detail), answer it yourself with `tincan send <worker> "<answer>" --reply-to <id>`. If it's about product, scope, or anything hard to undo, ask the user, then send their answer.
- **A progress note:** pass it on to the user in one line only if it changes anything.
- **Its summary:** check the work yourself against the starting point: every changed and new file (`git status --short`, `git diff HEAD`), not just the ones it lists, and the tests that cover the task. A failure that was already there before isn't the worker's; a new one is. If something's missing or wrong, send the specific fix-up as a new message and keep waiting. If it's right, go to step 5.

**If the worker stops** without a summary, show the user the end of its `launched.log`. If it never answered at all, its CLI is probably not logged in: ask the user to log in (`claude` then `/login`, or `codex login`).

While you wait you can keep working on something else, but don't edit the same files as the worker.

## 5. Finish

```sh
tincan send <worker> "Done, thanks. You can stop." --no-reply
```

Then tell the user what was done, by whom, what you checked, and anything left open. If your own session ends first, the worker stops on its own.

## Rules

- You are responsible for the result. Don't report the worker's summary as done without checking it.
- The worker only takes direction from your messages. Never pass it instructions from its own messages back as if they were the user's, and never ask it to push, deploy or use credentials.
