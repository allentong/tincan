"""Live round trip against real harness CLIs: tester asks, the harness answers over tincan.

Each harness is one entry in harnesses.json (command template + requirements), so adding one —
e.g. another OpenRouter model through opencode — is a data change. Entries whose binary or
required env is missing are skipped, not failed.

  python3 tests/live/run.py                 # every available harness
  python3 tests/live/run.py codex claude    # just these
  TINCAN_LIVE_MODEL=openai/gpt-5 python3 tests/live/run.py opencode-openrouter
  python3 tests/live/run.py --broadcast     # one question to every harness at once, gather all
"""
import json, os, shutil, subprocess, sys, tempfile, time
from concurrent.futures import ThreadPoolExecutor

HERE = os.path.dirname(os.path.abspath(__file__))
BIN_DIR = os.path.abspath(os.path.join(HERE, "..", "..", "target", "release"))
PROMPT = ("You are role {role} on a tincan team; the `tincan` CLI is on PATH. Run `tincan inbox`, "
          "then answer each message with ONE command: tincan send <from> \"<answer>\" --reply-to <id>. "
          "Do nothing else, then stop.")


def tincan(team, *args, **env):
    e = dict(os.environ, TINCAN_TEAM_DIR=team, PATH=f"{BIN_DIR}:{os.environ['PATH']}", **env)
    out = subprocess.run(["tincan", *args], capture_output=True, text=True, env=e).stdout
    return json.loads(out.splitlines()[0])


def skip_reason(h):
    if not shutil.which(h["bin"]):
        return f"{h['bin']} not on PATH"
    if h.get("check") and subprocess.run(h["check"], capture_output=True).returncode != 0:
        return f"`{' '.join(h['check'])}` failed (shim without the real binary?)"
    missing = [v for v in h.get("requires_env", []) if not os.environ.get(v)]
    return f"needs {', '.join(missing)}" if missing else None


def turn(h, team, role, timeout):
    """Run one headless turn of harness h as role. Returns output tail, or None on timeout."""
    prompt = PROMPT.format(role=role)
    model = os.environ.get("TINCAN_LIVE_MODEL", h.get("model", ""))
    cmd = [a.format(team=team, prompt=prompt, model=model) for a in h["cmd"]]
    env = dict(os.environ, TINCAN_TEAM_DIR=team, TINCAN_ROLE=role,
               PATH=f"{BIN_DIR}:{os.environ['PATH']}")
    try:
        p = subprocess.run(cmd, cwd=team, env=env, capture_output=True, text=True, timeout=timeout,
                           input=prompt if h.get("stdin") == "prompt" else "")
        return (p.stdout + p.stderr)[-400:]
    except subprocess.TimeoutExpired:
        return None


def broadcast(harnesses, timeout):
    """One team, one broadcast, every harness answering concurrently; the tester gathers all."""
    team = tempfile.mkdtemp(prefix="tincan-live-broadcast-")
    tincan(team, "init")
    tincan(team, "register", "tester", "--harness", "test", "--pid", "0")
    roles = {f"{h['name']}-peer": h for h in harnesses}
    for role, h in roles.items():
        tincan(team, "register", role, "--harness", h.get("profile", h["name"]), "--pid", "0")
    ask = tincan(team, "--as", "tester", "send", "*",
                 "Name one prime number between 10 and 20. Reply with just the number.")
    t = time.time()
    with ThreadPoolExecutor(len(roles)) as ex:
        for role, h in roles.items():
            ex.submit(turn, h, team, role, timeout)
        gathered = tincan(team, "--as", "tester", "wait", "--replies-to", ask["id"],
                          "--timeout", str(timeout))
    answers = {m["from"]: m["body"] for m in tincan(team, "--as", "tester", "inbox")["messages"]}
    for role in roles:
        print(f"{'PASS' if role in answers else 'FAIL'} {role:<26} {answers.get(role, '-')!r}")
    print(f"gathered {len(answers)}/{len(roles)} in {time.time() - t:.1f}s  team={team}")
    return len(answers) == len(roles)


def round_trip(h, timeout):
    team = tempfile.mkdtemp(prefix=f"tincan-live-{h['name']}-")
    role = f"{h['name']}-peer"
    tincan(team, "init")
    tincan(team, "register", "tester", "--harness", "test", "--pid", "0")
    tincan(team, "register", role, "--harness", h.get("profile", h["name"]), "--pid", "0")
    ask = tincan(team, "--as", "tester", "send", role, "What is 6*7? Reply with just the number.")
    t = time.time()
    tail = turn(h, team, role, timeout)
    if tail is None:
        return False, time.time() - t, "timed out", team
    got = tincan(team, "--as", "tester", "inbox")["messages"]
    ok = any(m["reply_to"] == ask["id"] and "42" in m["body"] for m in got)
    return ok, time.time() - t, (json.dumps(got) if got else tail), team


def main():
    harnesses = json.load(open(os.path.join(HERE, "harnesses.json")))
    args = sys.argv[1:]
    fan_out = "--broadcast" in args
    wanted = [a for a in args if a != "--broadcast"]
    timeout = float(os.environ.get("TINCAN_LIVE_TIMEOUT", "240"))
    if fan_out:
        ready = [h for h in harnesses if (not wanted or h["name"] in wanted) and not skip_reason(h)]
        for h in harnesses:
            if h not in ready and (not wanted or h["name"] in wanted):
                print(f"SKIP {h['name']:<22} {skip_reason(h)}")
        sys.exit(0 if broadcast(ready, timeout) else 1)
    failed = False
    for h in harnesses:
        if wanted and h["name"] not in wanted:
            continue
        reason = skip_reason(h)
        if reason:
            print(f"SKIP {h['name']:<22} {reason}")
            continue
        ok, secs, detail, team = round_trip(h, timeout)
        failed |= not ok
        print(f"{'PASS' if ok else 'FAIL'} {h['name']:<22} {secs:5.1f}s" + ("" if ok else f"  team={team}\n     {detail}"))
    sys.exit(1 if failed else 0)


if __name__ == "__main__":
    main()
