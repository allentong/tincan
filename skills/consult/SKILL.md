---
name: consult
description: Get a read-only second opinion from another agent (Claude Code, Codex, Grok) through tincan, on a question or a review of the current changes; it never edits anything. Use when the user asks to consult, ask, or get a review from another agent, or on your own when you're stuck or unsure (see "When to consult on your own").
---

# Consult another agent

Ask another agent for a second opinion and bring back its answer with your own take. The other agent works in the same repo, so it can read the files and diffs you point it at. It runs on the `tincan` CLI; if `tincan` isn't found, follow the tincan skill's "Getting started".

This is read-only advice. Don't apply its suggestions unless the user asks. To hand off work instead, use tincan's delegate skill.

## 1. Pick the agent

Use the one the user named. Otherwise pick a different harness from yours: `codex` if you are Claude, `claude` if you are Codex or Grok. You don't need it to be running: sending to its name starts a quick session that answers and exits.

## 2. Pick the mode

- **Question:** a decision, a tradeoff, a bug you can't pin down, a design check. This is the default.
- **Challenge the changes:** the user asked for a review of the changes, or you want your work checked before calling it done. Find the scope: uncommitted work (`git diff HEAD --stat` for tracked files, staged or not, plus untracked files from `git status --short`) or a branch (`git diff --stat <base>...HEAD`, base usually `main`). If the scope is empty, say so and stop.

## 3. Write the message

The other agent starts cold. Make the message stand on its own, under 8 KiB, and point at files rather than pasting them.

Question:

```
## Context
<What we're building, what's done, what's been tried and why it failed.
File paths, exact error text, the constraint that matters.>

## Question
<The one thing you need: which approach, why this fails, is this design sound.>

Answer in under 300 words: your recommendation first, then the reasons.
```

Challenge the changes:

```
Review <the uncommitted changes | the diff from <base> to HEAD> in this repo.
Read it yourself: `<git diff HEAD | git diff <base>...HEAD>`, plus these new files: <untracked paths, if any>. Files: <from --stat>.
Intent: <what the change is for, in one or two lines>.
Focus: <the user's focus, if any>.

Try to find the strongest reasons this should not ship: auth and trust boundaries,
data loss, retries and partial failure, races and stale state, empty and error
paths, compatibility. Only real findings, no style notes. For each: file:line,
what goes wrong, why, and the fix. Put one strong finding before several weak
ones. Start your reply with SHIP or DON'T SHIP and one line why. If it's sound,
say so and list nothing.
```

## 4. Send and wait

```sh
tincan send <agent> - <<'EOF'
<message>
EOF
tincan wait --replies-to <id> --timeout 600
```

Give your shell tool a timeout of at least 660 seconds. A quick Claude session usually answers in about 10 s, Codex in about 20 s, longer for a real review. Then read the answer with `tincan inbox`.

If `wait` lists the agent under `ended`, it quit without answering, usually because that CLI isn't logged in. Show the user the last lines of the `log` path from the send's `launched` field and ask them to log in (`claude` then `/login`, or `codex login`). If it times out, tell the user; don't resend in a loop.

For a high-stakes call, or when the user asks for several opinions, send the same message to two agents (for example `codex` and `grok`) and wait for each; then report where they agree and where they split.

## 5. Report

- **Question:** give the other agent's recommendation, then yours. Say where you agree and where you don't, and the next step you'd take.
- **Challenge the changes:** show its verdict and findings as it wrote them. Then say which findings you think hold up after checking the code, and which don't and why. Don't fix anything unless the user asks.

Keep it short, and name which agent answered.

## When to consult on your own

Consult without being asked, and tell the user you're doing it, when:

- you've tried three different approaches to the same problem and none worked
- you're relying on an API or tool you're unsure about
- two approaches look equally good and the tradeoff isn't clear
- you're about to finish a change that touches auth, data or concurrency (challenge the changes)

Don't consult for things you can check yourself quickly by reading the code or running it.
