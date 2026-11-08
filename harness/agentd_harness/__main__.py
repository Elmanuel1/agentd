"""The sandbox's first process: run the session of a work item agentd already claimed, keeping
its lease, as the official SDK worker's `handle_item` does.

Environment, as Anthropic's poller hands a claimed item to a fresh process: `ANTHROPIC_BASE_URL`,
`ANTHROPIC_ENVIRONMENT_ID`, `ANTHROPIC_SESSION_ID`, `ANTHROPIC_WORK_ID` and `ANTHROPIC_WORK_SECRET`
(base64url JSON carrying `sessions_token`, the credential for every call), and the egress settings
agentd starts the sandbox with. No environment key reaches the sandbox.
"""

import base64
import json
import os
import signal
import sys
import threading
import time

import anthropic

from . import mounts
from .api import Agentd
from .run import SessionRun

LEASE_TTL = 60
BEAT_EVERY = 20


def sessions_token(secret):
    padded = secret + "=" * (-len(secret) % 4)
    return json.loads(base64.urlsafe_b64decode(padded))["sessions_token"]


def current_heartbeat(error):
    """The lease's current heartbeat from a 412 answer, or None."""
    body = error.body if isinstance(error.body, dict) else {}
    state = (body.get("error") or {}).get("current_state") or body.get("current_state") or {}
    return state.get("last_heartbeat") or None


def keep_lease(work, work_id, environment_id, ended):
    """Heartbeat the lease until it ends; `ended` says how: `stopping` (agentd stops the work, so
    the harness stops it in turn) or `lost` (another holder may own it now; it is left alone)."""
    last, unknown = "NO_HEARTBEAT", False
    while True:
        try:
            beat = work.heartbeat(work_id, environment_id=environment_id, desired_ttl_seconds=LEASE_TTL,
                                  expected_last_heartbeat=last)
            unknown = False
            if beat.state in ("stopping", "stopped"):
                return ended("stopping")
            if beat.lease_extended is False:
                return ended("lost")
            last = beat.last_heartbeat
        except anthropic.APIStatusError as e:
            # A beat whose answer was lost leaves `last` stale: adopt the lease's own and go on.
            current = current_heartbeat(e) if e.status_code == 412 and unknown else None
            if current:
                last, unknown = current, False
                continue
            if e.status_code in (404, 409, 412):
                return ended("lost")
            unknown = True
        except anthropic.APIConnectionError:
            unknown = True
        time.sleep(BEAT_EVERY)


def on_stop(agentd):
    """The SIGTERM handler: agentd is stopping the sandbox, so the outputs the agent wrote since
    its last turn ended are saved as session files before the process exits."""
    def handler(_signum, _frame):
        try:
            mounts.sync_outputs(agentd)
        except Exception:  # noqa: BLE001 - the sandbox stops either way
            pass
        os._exit(0)
    return handler


def main(env=os.environ):
    # First: the sandbox's TLS settings name this bundle, and every HTTP client reads it when built.
    mounts.trust_ca(env.get("AGENTD_EGRESS_CA"))
    environment_id, session_id = env["ANTHROPIC_ENVIRONMENT_ID"], env["ANTHROPIC_SESSION_ID"]
    work_id = env["ANTHROPIC_WORK_ID"]
    token = sessions_token(env["ANTHROPIC_WORK_SECRET"])
    client = anthropic.Anthropic(base_url=env["ANTHROPIC_BASE_URL"], auth_token=token, max_retries=5)
    work = client.beta.environments.work
    # No SDK retries for heartbeats: a lost answer must come back to keep_lease, not become a 412.
    beats = client.with_options(max_retries=0).beta.environments.work
    end, done = [], threading.Event()

    def ended(how):
        end.append(how)
        done.set()

    threading.Thread(target=keep_lease, args=(beats, work_id, environment_id, ended), daemon=True).start()
    agentd = Agentd(client, session_id)
    signal.signal(signal.SIGTERM, on_stop(agentd))
    runner = threading.Thread(target=SessionRun(agentd, env).serve, daemon=True)
    runner.start()
    while runner.is_alive() and not done.is_set():
        runner.join(timeout=1)
    # A lost lease is left alone: another holder may own the work now.
    if end != ["lost"]:
        try:
            work.stop(work_id, environment_id=environment_id, force=True)
        except anthropic.APIError:
            pass
    return 0


if __name__ == "__main__":
    sys.exit(main())
