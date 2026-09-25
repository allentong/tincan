"""Black-box acceptance tests for the tincan CLI, mapped to spec AC numbers.
Runs against TINCAN_BIN (default: target/release/tincan)."""
import json, os, sqlite3, subprocess, sys, tempfile, time, unittest
from concurrent.futures import ThreadPoolExecutor

HERE = os.path.dirname(os.path.abspath(__file__))
TINCAN_BIN = os.environ.get("TINCAN_BIN", os.path.join(HERE, "..", "target", "release", "tincan"))
CLEAN_ENV = {k: v for k, v in os.environ.items()
             if not k.startswith(("CLAUDE", "CODEX", "TINCAN", "GROK", "CMUX", "TMUX", "HERDR"))}


def run(*args, team=None, env=None, cwd=None, stdin=None):
    """Returns (exit code, parsed JSON of the first stdout line or None, stderr)."""
    e = dict(CLEAN_ENV, **(env or {}))
    cmd = [TINCAN_BIN] + (["--team-dir", team] if team else []) + list(args)
    p = subprocess.run(cmd, capture_output=True, text=True, env=e, cwd=cwd, input=stdin)
    return p.returncode, (json.loads(p.stdout.splitlines()[0]) if p.stdout.strip() else None), p.stderr


class Base(unittest.TestCase):
    def setUp(self):
        self.tmp = tempfile.TemporaryDirectory()
        self.team = self.tmp.name
        self.procs = []
        self.assertEqual(run("init", team=self.team)[0], 0)

    def tearDown(self):
        for p in self.procs:
            p.kill(); p.wait()
        self.tmp.cleanup()

    def owner(self):
        """Stand-in for a live harness process that a role's liveness binds to."""
        p = subprocess.Popen(["sleep", "600"])
        self.procs.append(p)
        return p

    def reg(self, role, harness):
        p = self.owner()
        rc, o, _ = run("register", role, "--harness", harness, "--pid", str(p.pid), team=self.team)
        self.assertEqual(rc, 0, o)
        return p


class TestAcceptance(Base):
    def test_ac1_ac2_dm_both_directions(self):
        self.reg("claude", "claude"); self.reg("grok", "grok")
        rc, o, _ = run("--as", "claude", "send", "grok", "please review PR 8230", team=self.team)
        self.assertEqual(rc, 0, o)
        rc, o, _ = run("--as", "grok", "inbox", team=self.team)
        self.assertEqual([m["body"] for m in o["messages"]], ["please review PR 8230"])
        mid = o["messages"][0]["id"]
        rc, o, _ = run("--as", "grok", "send", "claude", "LGTM", "--reply-to", mid, team=self.team)
        self.assertEqual((rc, o["hop"]), (0, 1))
        rc, o, _ = run("--as", "claude", "inbox", team=self.team)
        self.assertEqual(o["messages"][0]["reply_to"], mid)
        # read is sticky: second inbox is empty
        self.assertEqual(run("--as", "claude", "inbox", team=self.team)[1]["messages"], [])

    def test_ac3_stale_peer_does_not_black_hole(self):
        self.reg("claude", "claude"); g = self.reg("grok", "grok")
        g.kill(); g.wait()
        roles = {p["role"]: p["state"] for p in run("peers", "--all", team=self.team)[1]["peers"]}
        self.assertEqual(roles, {"claude": "active", "grok": "stale"})
        self.assertEqual([p["role"] for p in run("peers", team=self.team)[1]["peers"]], ["claude"])
        rc, o, _ = run("--as", "claude", "send", "grok", "hello?", team=self.team)
        self.assertEqual((rc, o["error"], o["state"]), (3, "peer_unavailable", "stale"))

    def test_ac3_unregister_marks_gone(self):
        self.reg("claude", "claude"); self.reg("codex", "codex")
        self.assertEqual(run("--as", "codex", "unregister", team=self.team)[0], 0)
        rc, o, _ = run("--as", "claude", "send", "codex", "x", team=self.team)
        self.assertEqual((rc, o["state"]), (3, "gone"))

    def test_ac4_concurrent_sends_consistent(self):
        self.reg("claude", "claude"); self.reg("grok", "grok")
        N = 60
        def send(i):
            a, b = ("claude", "grok") if i % 2 else ("grok", "claude")
            return run("--as", a, "send", b, f"{a}-{i}", "--client-id", f"c{i}", team=self.team)[0]
        with ThreadPoolExecutor(16) as ex:
            codes = list(ex.map(send, range(N)))
        self.assertEqual(codes, [0] * N)
        # pending mail survives the senders exiting; each side gets exactly its half, no dupes
        for p in self.procs: p.kill(); p.wait()
        g = run("--as", "grok", "inbox", team=self.team)[1]["messages"]
        c = run("--as", "claude", "inbox", team=self.team)[1]["messages"]
        self.assertEqual((len(g), len(c)), (N // 2, N // 2))
        self.assertEqual(len({m["id"] for m in g + c}), N)

    def test_ac6_idempotent_resend(self):
        self.reg("claude", "claude"); self.reg("grok", "grok")
        a = run("--as", "claude", "send", "grok", "hi", "--client-id", "abc", team=self.team)[1]
        b = run("--as", "claude", "send", "grok", "hi", "--client-id", "abc", team=self.team)[1]
        self.assertEqual(a["id"], b["id"]); self.assertTrue(b["duplicate"])
        self.assertEqual(len(run("--as", "grok", "inbox", team=self.team)[1]["messages"]), 1)

    def test_ac8_any_pair_and_broadcast(self):
        self.reg("claude", "claude"); self.reg("grok", "grok"); self.reg("codex", "codex")
        self.assertEqual(run("--as", "codex", "send", "grok", "c->g", team=self.team)[0], 0)
        rc, o, _ = run("--as", "grok", "send", "*", "all hands", team=self.team)
        self.assertEqual(sorted(o["recipients"]), ["claude", "codex"])
        self.assertEqual([m["body"] for m in run("--as", "grok", "inbox", team=self.team)[1]["messages"]], ["c->g"])
        self.assertEqual(run("--as", "codex", "inbox", "--count", team=self.team)[1]["unread"], 1)
        self.assertEqual(run("--as", "claude", "inbox", team=self.team)[1]["messages"][0]["kind"], "broadcast")


class TestEdgeCases(Base):
    def test_role_collision_rejected(self):
        self.reg("claude", "claude")
        p = self.owner()
        rc, o, _ = run("register", "claude", "--pid", str(p.pid), team=self.team)
        self.assertEqual((rc, o["error"]), (5, "role_taken"))

    def test_no_team_dir_fails_loud(self):
        with tempfile.TemporaryDirectory() as d:
            rc, o, _ = run("peers", cwd=d)
            self.assertEqual((rc, o["error"]), (2, "no_team"))

    def test_team_dir_discovered_from_subdir(self):
        self.reg("claude", "claude")
        sub = os.path.join(self.team, "a", "b"); os.makedirs(sub)
        rc, o, _ = run("peers", cwd=sub)
        self.assertEqual([p["role"] for p in o["peers"]], ["claude"])

    def test_large_payload_rejected(self):
        self.reg("claude", "claude"); self.reg("grok", "grok")
        rc, o, _ = run("--as", "claude", "send", "grok", "-", team=self.team, stdin="x" * 9000)
        self.assertEqual((rc, o["error"]), (4, "too_large"))

    def test_reply_loop_capped(self):
        self.reg("a", "claude"); self.reg("b", "codex")
        mid = run("--as", "a", "send", "b", "0", team=self.team)[1]["id"]
        for i in range(8):
            s, t = ("b", "a") if i % 2 == 0 else ("a", "b")
            rc, o, _ = run("--as", s, "send", t, str(i), "--reply-to", mid, team=self.team)
            self.assertEqual(rc, 0, o); mid = o["id"]
        rc, o, _ = run("--as", "a", "send", "b", "loop", "--reply-to", mid, team=self.team)
        self.assertEqual((rc, o["error"]), (7, "hop_limit"))

    def test_identity_inferred_from_session_env(self):
        p = self.owner()
        env = {"CLAUDE_CODE_SESSION_ID": "sess-1"}
        run("register", "claude", "--pid", str(p.pid), team=self.team, env=env)
        rc, o, _ = run("whoami", team=self.team, env=env)
        self.assertEqual((rc, o["role"]), (0, "claude"))
        rc, o, _ = run("whoami", team=self.team)
        self.assertEqual((rc, o["error"]), (6, "not_registered"))

    def test_peek_keeps_unread(self):
        self.reg("claude", "claude"); self.reg("grok", "grok")
        run("--as", "claude", "send", "grok", "hi", team=self.team)
        self.assertEqual(len(run("--as", "grok", "inbox", "--peek", team=self.team)[1]["messages"]), 1)
        self.assertEqual(run("--as", "grok", "whoami", team=self.team)[1]["unread"], 1)

    def test_hook_emits_context_only_when_unread(self):
        self.reg("claude", "claude"); self.reg("grok", "grok")
        self.assertIsNone(run("--as", "claude", "hook", team=self.team)[1])
        run("--as", "grok", "send", "claude", "ping", team=self.team)
        o = run("--as", "claude", "hook", team=self.team)[1]
        self.assertIn("1 unread", o["hookSpecificOutput"]["additionalContext"])
        # hook never breaks the turn when misconfigured
        with tempfile.TemporaryDirectory() as d:
            rc, o, _ = run("hook", cwd=d)
            self.assertEqual((rc, o), (0, None))

    def test_wait_wakes_on_message(self):
        self.reg("claude", "claude"); self.reg("grok", "grok")
        e = dict(CLEAN_ENV)
        w = subprocess.Popen([TINCAN_BIN, "--team-dir", self.team, "--as", "claude", "wait",
                              "--timeout", "20"], stdout=subprocess.PIPE, text=True, env=e)
        time.sleep(1)
        t0 = time.time()
        run("--as", "grok", "send", "claude", "wake", team=self.team)
        o = json.loads(w.communicate(timeout=10)[0])
        self.assertEqual((o["unread"], o["timed_out"]), (1, False))
        self.assertLess(time.time() - t0, 2)


class TestV2(Base):
    """Behaviour added by the v2 spec."""

    def test_require_ack_redelivers_after_lease(self):
        self.reg("claude", "claude"); self.reg("grok", "grok")
        mid = run("--as", "claude", "send", "grok", "important", team=self.team)[1]["id"]
        env = {"TINCAN_LEASE_SECS": "1"}
        got = run("--as", "grok", "inbox", "--require-ack", team=self.team, env=env)[1]["messages"]
        self.assertEqual([m["id"] for m in got], [mid])
        # leased: hidden from a second reader, still unread
        self.assertEqual(run("--as", "grok", "inbox", team=self.team, env=env)[1]["messages"], [])
        self.assertEqual(run("--as", "grok", "whoami", team=self.team)[1]["unread"], 1)
        time.sleep(1.3)
        again = run("--as", "grok", "inbox", "--require-ack", team=self.team, env=env)[1]["messages"]
        self.assertEqual([m["id"] for m in again], [mid])
        self.assertEqual(run("--as", "grok", "ack", mid, team=self.team)[1]["acked"], 1)
        time.sleep(1.3)
        self.assertEqual(run("--as", "grok", "inbox", team=self.team, env=env)[1]["messages"], [])
        self.assertEqual(run("--as", "grok", "whoami", team=self.team)[1]["unread"], 0)

    def db(self, sql):
        con = sqlite3.connect(os.path.join(self.team, ".tincan", "tincan.db"))
        try:
            return con.execute(sql).fetchall()
        finally:
            con.close()

    def test_read_message_is_erased(self):
        self.reg("claude", "claude"); self.reg("grok", "grok")
        run("--as", "claude", "send", "grok", "secret diff", team=self.team)
        run("--as", "grok", "inbox", team=self.team)
        self.assertEqual(self.db("SELECT COUNT(*) FROM deliveries"), [(0,)])
        self.assertEqual(self.db("SELECT body FROM messages"), [("",)])

    def test_broadcast_body_kept_until_last_reader(self):
        self.reg("a", "claude"); self.reg("b", "codex"); self.reg("c", "grok")
        run("--as", "a", "send", "*", "all hands", team=self.team)
        run("--as", "b", "inbox", team=self.team)
        self.assertEqual(self.db("SELECT body FROM messages"), [("all hands",)])
        run("--as", "c", "inbox", team=self.team)
        self.assertEqual(self.db("SELECT body FROM messages"), [("",)])

    def test_dedupe_survives_read(self):
        self.reg("claude", "claude"); self.reg("grok", "grok")
        a = run("--as", "claude", "send", "grok", "hi", "--client-id", "k1", team=self.team)[1]
        run("--as", "grok", "inbox", team=self.team)
        b = run("--as", "claude", "send", "grok", "hi", "--client-id", "k1", team=self.team)[1]
        self.assertEqual((b["id"], b["duplicate"]), (a["id"], True))
        self.assertEqual(run("--as", "grok", "inbox", team=self.team)[1]["messages"], [])

    def test_stub_expires_after_window(self):
        self.reg("claude", "claude"); self.reg("grok", "grok")
        mid = run("--as", "claude", "send", "grok", "x", team=self.team)[1]["id"]
        run("--as", "grok", "inbox", team=self.team)
        # the next send sweeps; with a zero window the wiped stub is gone
        run("--as", "claude", "send", "grok", "y", team=self.team, env={"TINCAN_STUB_SECS": "0"})
        rc, o, _ = run("--as", "grok", "send", "claude", "late", "--reply-to", mid, team=self.team)
        self.assertEqual((rc, o["error"]), (2, "usage"))

    def test_new_session_does_not_inherit_mail(self):
        self.reg("claude", "claude"); g = self.reg("grok", "grok")
        run("--as", "claude", "send", "grok", "for the old session", team=self.team)
        g.kill(); g.wait()
        p = self.owner()
        rc, o, _ = run("register", "grok", "--pid", str(p.pid), team=self.team)
        self.assertEqual((rc, o["reclaimed"], o["dropped_pending"]), (0, True, 1))
        self.assertEqual(run("--as", "grok", "inbox", team=self.team)[1]["messages"], [])

    def test_unregister_drops_pending(self):
        self.reg("claude", "claude"); self.reg("grok", "grok")
        run("--as", "claude", "send", "grok", "never read", team=self.team)
        self.assertEqual(run("--as", "grok", "unregister", team=self.team)[1]["dropped_pending"], 1)
        self.assertEqual(self.db("SELECT body FROM messages"), [("",)])

    def test_sweep_removes_ended_peers(self):
        self.reg("claude", "claude"); g = self.reg("grok", "grok")
        run("--as", "claude", "send", "grok", "x", team=self.team)
        g.kill(); g.wait()
        self.assertEqual(len(run("peers", "--all", team=self.team)[1]["peers"]), 2)
        # any register/send sweeps; zero grace removes the dead peer and its pending mail now
        self.reg("codex", "codex")
        self.assertEqual(len(run("peers", "--all", team=self.team)[1]["peers"]), 3)
        run("--as", "claude", "send", "codex", "y", team=self.team, env={"TINCAN_PEER_GRACE_SECS": "0"})
        self.assertEqual(sorted(p["role"] for p in run("peers", "--all", team=self.team)[1]["peers"]), ["claude", "codex"])
        self.assertEqual(self.db("SELECT COUNT(*) FROM deliveries WHERE recipient = 'grok'"), [(0,)])

    def test_inbox_killed_before_output_redelivers(self):
        """stdout closed before inbox can write: the lease expires and the message comes back."""
        self.reg("claude", "claude"); self.reg("grok", "grok")
        run("--as", "claude", "send", "grok", "survives", team=self.team)
        e = dict(CLEAN_ENV, TINCAN_LEASE_SECS="1")
        r, w = os.pipe(); os.close(r)  # reader gone: the write fails with EPIPE
        p = subprocess.Popen([TINCAN_BIN, "--team-dir", self.team, "--as", "grok", "inbox"], stdout=w, env=e)
        os.close(w); p.wait()
        time.sleep(1.3)
        got = run("--as", "grok", "inbox", team=self.team, env={"TINCAN_LEASE_SECS": "1"})[1]["messages"]
        self.assertEqual([m["body"] for m in got], ["survives"])

    def test_no_reply_rejects_answers(self):
        self.reg("a", "claude"); self.reg("b", "codex")
        mid = run("--as", "a", "send", "b", "thanks, done", "--no-reply", team=self.team)[1]["id"]
        self.assertTrue(run("--as", "b", "inbox", team=self.team)[1]["messages"][0]["no_reply"])
        rc, o, _ = run("--as", "b", "send", "a", "you're welcome", "--reply-to", mid, team=self.team)
        self.assertEqual((rc, o["error"]), (8, "reply_not_wanted"))

    def test_resumed_session_rebinds_pid(self):
        old, new = self.owner(), self.owner()
        run("register", "claude", team=self.team, env={"TINCAN_OWNER_PID": str(old.pid), "TINCAN_SESSION": "s1"})
        old.kill(); old.wait()
        self.assertEqual(run("peers", "--all", team=self.team)[1]["peers"][0]["state"], "stale")
        # same session id, new process: first call rebinds instead of failing
        rc, o, _ = run("whoami", team=self.team, env={"TINCAN_OWNER_PID": str(new.pid), "TINCAN_SESSION": "s1"})
        self.assertEqual((rc, o["role"], o["pid"]), (0, "claude", new.pid))
        self.assertEqual(run("peers", team=self.team)[1]["peers"][0]["state"], "active")

    def test_heartbeat_peer_stays_active_without_pid(self):
        run("register", "codex", "--pid", "0", team=self.team)
        p = run("peers", team=self.team)[1]["peers"][0]
        self.assertEqual((p["pid"], p["state"]), (None, "active"))

    def test_usage_errors_are_json(self):
        rc, o, _ = run("send", team=self.team)
        self.assertEqual((rc, o["ok"], o["error"]), (2, False, "usage"))


class TestHooks(Base):
    """Terminal-free delivery through harness hooks (Claude Code and Codex share the contract)."""

    def hook(self, event, role="b", linger=None, payload=None):
        args = ["--as", role, "hook", "--event", event] + (["--linger", str(linger)] if linger else [])
        return run(*args, team=self.team, stdin=json.dumps(payload or {"hook_event_name": event}))

    def test_stop_blocks_once_per_new_message(self):
        self.reg("a", "claude"); self.reg("b", "codex")
        self.assertIsNone(self.hook("Stop")[1])
        run("--as", "a", "send", "b", "one", team=self.team)
        o = self.hook("Stop")[1]
        self.assertEqual(o["decision"], "block")
        self.assertIn("1 unread", o["reason"])
        # agent ignored it: the same message never blocks a second stop (no stop loop)
        self.assertIsNone(self.hook("Stop")[1])
        run("--as", "a", "send", "b", "two", team=self.team)
        self.assertEqual(self.hook("Stop")[1]["decision"], "block")

    def test_stop_lingers_for_reply_to_open_request(self):
        self.reg("a", "claude"); self.reg("b", "codex")
        mid = run("--as", "b", "send", "a", "question?", team=self.team)[1]["id"]
        def answer():
            time.sleep(1)
            run("--as", "a", "send", "b", "answer", "--reply-to", mid, team=self.team)
        with ThreadPoolExecutor() as ex:
            ex.submit(answer)
            t = time.time(); o = self.hook("Stop", linger=10)[1]
        self.assertEqual(o["decision"], "block")
        self.assertLess(time.time() - t, 5)

    def test_stop_does_not_linger_without_open_request(self):
        self.reg("a", "claude"); self.reg("b", "codex")
        mid = run("--as", "b", "send", "a", "fyi", "--no-reply", team=self.team)[1]["id"]
        t = time.time()
        self.assertIsNone(self.hook("Stop", linger=10)[1])
        self.assertLess(time.time() - t, 2)

    def test_post_tool_use_tells_once(self):
        self.reg("a", "claude"); self.reg("b", "codex")
        run("--as", "a", "send", "b", "mid-turn news", team=self.team)
        o = self.hook("PostToolUse")[1]
        self.assertIn("1 unread", o["hookSpecificOutput"]["additionalContext"])
        self.assertIsNone(self.hook("PostToolUse")[1])

    def test_session_end_releases_role(self):
        self.reg("b", "codex")
        self.hook("SessionEnd")
        self.assertEqual(run("peers", "--all", team=self.team)[1]["peers"][0]["state"], "gone")

    def test_hook_identity_from_stdin_session_id(self):
        run("register", "b", "--pid", "0", team=self.team, env={"TINCAN_SESSION": "thread-9"})
        self.reg("a", "claude")
        run("--as", "a", "send", "b", "hi", team=self.team)
        rc, o, _ = run("hook", "--event", "Stop", team=self.team,
                       stdin=json.dumps({"hook_event_name": "Stop", "session_id": "thread-9"}))
        self.assertEqual(o["decision"], "block")

    def test_hooks_config_per_harness(self):
        o = run("hooks", "--harness", "codex")[1]
        self.assertEqual(o["file"], ".codex/hooks.json")
        self.assertIn("Stop", o["config"]["hooks"])
        rc, o, _ = run("hooks", "--harness", "grok")
        self.assertEqual((rc, o["error"]), (2, "usage"))


class TestWake(Base):
    """Wake drivers are opt-in per peer; the default touches no terminal."""

    def reg_wake(self, role, spec, env=None):
        p = self.owner()
        return run("register", role, "--harness", "codex", "--pid", str(p.pid), "--wake", spec,
                   team=self.team, env=env)

    def test_default_is_no_wake(self):
        self.reg("a", "claude"); self.reg("b", "codex")
        o = run("--as", "a", "send", "b", "x", team=self.team)[1]
        self.assertEqual(o["wake"], {})

    def test_cmd_driver_nudges_once_per_new_message(self):
        log = os.path.join(self.team, "wake.log")
        self.reg("a", "claude")
        rc, o, _ = self.reg_wake("b", f'cmd:echo "$TINCAN_WAKE_ROLE $TINCAN_WAKE_UNREAD" >> {log}')
        self.assertEqual(rc, 0, o)
        self.assertEqual(run("--as", "a", "send", "b", "1", team=self.team)[1]["wake"], {"b": "nudged"})
        # recipient hasn't read yet; a Stop hook or earlier nudge already told it about seq<=1
        self.assertEqual(run("--as", "a", "send", "b", "2", team=self.team)[1]["wake"], {"b": "nudged"})
        run("--as", "b", "inbox", team=self.team)
        self.assertEqual(open(log).read().split("\n")[:2], ["b 1", "b 2"])

    def test_nudge_counts_as_told_for_stop_hook(self):
        log = os.path.join(self.team, "wake.log")
        self.reg("a", "claude"); self.reg_wake("b", f"cmd:echo x >> {log}")
        run("--as", "b", "hook", "--event", "Stop", team=self.team)
        mid = run("--as", "a", "send", "b", "1", team=self.team)
        self.assertEqual(mid[1]["wake"], {"b": "nudged"})
        self.assertEqual(run("--as", "b", "hook", "--event", "Stop", team=self.team,
                             stdin='{"hook_event_name":"Stop"}')[1], None)

    def test_failed_nudge_never_fails_send(self):
        self.reg("a", "claude"); self.reg_wake("b", "cmd:exit 3")
        rc, o, _ = run("--as", "a", "send", "b", "x", team=self.team)
        self.assertEqual(rc, 0)
        self.assertTrue(o["wake"]["b"].startswith("failed"))

    def test_bad_spec_rejected(self):
        rc, o, _ = self.reg_wake("b", "zellij:1")
        self.assertEqual((rc, o["error"]), (2, "usage"))

    def test_auto_picks_current_terminal(self):
        rc, o, _ = self.reg_wake("b", "auto", env={"TMUX_PANE": "%7"})
        self.assertEqual(o["wake"], "tmux:%7")


class TestHarnessProfiles(Base):
    """New harnesses are data: a JSON profile, no code change."""

    def test_user_profile_detected_by_marker_env(self):
        prof = os.path.join(self.team, "harnesses.json")
        with open(prof, "w") as f:
            json.dump([{"name": "opencode", "session_env": ["OPENCODE_SESSION"],
                        "marker_env": ["OPENCODE_RUN"], "hooks_file": ".opencode/hooks.json"}], f)
        owner = self.owner()
        env = {"TINCAN_HARNESSES": prof, "OPENCODE_RUN": "1", "OPENCODE_SESSION": "oc-1",
               "TINCAN_OWNER_PID": str(owner.pid)}
        rc, o, _ = run("register", "oc", team=self.team, env=env)
        self.assertEqual((rc, o["harness"]), (0, "opencode"))
        # its session id resolves identity without --as
        self.assertEqual(run("whoami", team=self.team, env=env)[1]["role"], "oc")
        self.assertEqual(run("hooks", "--harness", "opencode", env=env)[1]["file"], ".opencode/hooks.json")


class TestFanOut(Base):
    """Broadcast one question, gather every answer."""

    def test_wait_replies_gathers_all_recipients(self):
        self.reg("lead", "claude"); self.reg("x", "codex"); self.reg("y", "grok")
        mid = run("--as", "lead", "send", "*", "vote?", team=self.team)[1]["id"]
        def answer(role, delay):
            time.sleep(delay)
            run("--as", role, "send", "lead", f"{role} says yes", "--reply-to", mid, team=self.team)
        with ThreadPoolExecutor() as ex:
            ex.submit(answer, "x", 0.3); ex.submit(answer, "y", 1.0)
            o = run("--as", "lead", "wait", "--replies-to", mid, "--timeout", "10", team=self.team)[1]
        self.assertEqual((sorted(o["replied"]), o["waiting"], o["timed_out"]), (["x", "y"], [], False))
        bodies = sorted(m["body"] for m in run("--as", "lead", "inbox", team=self.team)[1]["messages"])
        self.assertEqual(bodies, ["x says yes", "y says yes"])

    def test_wait_replies_stops_for_ended_session(self):
        self.reg("lead", "claude"); px = self.reg("x", "codex")
        mid = run("--as", "lead", "send", "x", "q?", team=self.team)[1]["id"]
        px.kill(); px.wait()
        t = time.time()
        o = run("--as", "lead", "wait", "--replies-to", mid, "--timeout", "10", team=self.team)[1]
        self.assertEqual((o["ended"], o["timed_out"]), (["x"], False))
        self.assertLess(time.time() - t, 3)

    def test_wait_replies_times_out_with_stragglers(self):
        self.reg("lead", "claude"); self.reg("x", "codex")
        mid = run("--as", "lead", "send", "x", "q?", team=self.team)[1]["id"]
        o = run("--as", "lead", "wait", "--replies-to", mid, "--timeout", "0.5", team=self.team)[1]
        self.assertEqual((o["waiting"], o["timed_out"]), (["x"], True))


class TestExtensions(Base):
    """Drivers and harnesses plug in as JSON; a broken extension never breaks the core."""

    def write(self, name, obj):
        path = os.path.join(self.team, name)
        with open(path, "w") as f:
            f.write(obj if isinstance(obj, str) else json.dumps(obj))
        return path

    def test_user_driver_runs_argv_template_without_shell(self):
        log = os.path.join(self.team, "argv.log")
        fake = self.write("fake-term", f"#!/bin/sh\nprintf '%s|' \"$@\" >> {log}; echo >> {log}\n")
        os.chmod(fake, 0o755)
        drivers = self.write("drivers.json", [{"name": "faketerm", "detect_env": "FAKETERM_PANE",
            "nudge": [[fake, "type", "{target}", "{text}"], [fake, "key", "{target}", "enter"]]}])
        env = {"TINCAN_DRIVERS": drivers}
        self.reg("a", "claude")
        p = self.owner()
        rc, o, _ = run("register", "b", "--pid", str(p.pid), "--wake", "auto", team=self.team,
                       env=dict(env, FAKETERM_PANE="w1:p9"))
        self.assertEqual(o["wake"], "faketerm:w1:p9")
        o = run("--as", "a", "send", "b", "x", team=self.team, env=env)[1]
        self.assertEqual(o["wake"], {"b": "nudged"})
        lines = open(log).read().splitlines()
        self.assertTrue(lines[0].startswith("type|w1:p9|[tincan] role b has 1 unread"))
        self.assertEqual(lines[1], "key|w1:p9|enter|")

    def test_broken_driver_file_keeps_builtins(self):
        env = {"TINCAN_DRIVERS": self.write("drivers.json", "{not json")}
        o = run("extensions", env=env)[1]
        self.assertFalse(o["ok"])
        self.assertEqual([w["name"] for w in o["wake"]], ["tmux", "cmux", "cmd"])
        self.assertEqual(len(o["errors"]), 1)
        # and the core still works
        self.reg("a", "claude"); self.reg("b", "codex")
        self.assertEqual(run("--as", "a", "send", "b", "x", team=self.team, env=env)[0], 0)

    def test_invalid_entry_skipped_valid_one_loaded(self):
        env = {"TINCAN_DRIVERS": self.write("drivers.json", [
            {"name": "cmd", "nudge": [["x"]]},
            {"name": "good", "nudge": [["true"]]}])}
        o = run("extensions", env=env)[1]
        self.assertIn("good", [w["name"] for w in o["wake"]])
        self.assertIn("reserved", o["errors"][0])


class TestLocalOnly(Base):
    def test_no_team_error_points_remote_agents_to_run_locally(self):
        with tempfile.TemporaryDirectory() as d:
            rc, o, _ = run("inbox", cwd=d)
        self.assertEqual((rc, o["error"]), (2, "no_team"))
        self.assertIn("local-only", o["hint"])
        self.assertIn("run this session locally", o["hint"])


if __name__ == "__main__":
    unittest.main(verbosity=2)
