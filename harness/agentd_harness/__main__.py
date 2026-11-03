"""The sandbox's first process: take the session's work item, keep its lease, and run the session.

Environment: `AGENTD_API_URL`, `AGENTD_ENVIRONMENT_ID`, `AGENTD_WORK_ID`, `AGENTD_WORK_SECRET`
(base64url JSON carrying `sessions_token`), `AGENTD_TARGET_ID` (the session), and the egress
settings agentd starts the sandbox with.
"""

import base64
import json
import os
import sys
import threading
import time

import anthropic

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


def main(env=os.environ):
    api, environment_id, work_id = env["AGENTD_API_URL"], env["AGENTD_ENVIRONMENT_ID"], env["AGENTD_WORK_ID"]
    token = sessions_token(env["AGENTD_WORK_SECRET"])
    work = anthropic.Anthropic(base_url=api, auth_token=token, max_retries=5).beta.environments.work
    while True:
        try:
            item = work.ack(work_id, environment_id=environment_id)
            break
        except anthropic.APIConnectionError:
            time.sleep(1)
    session_id = env.get("AGENTD_TARGET_ID") or item.data.id
    end, done = [], threading.Event()

    def ended(how):
        end.append(how)
        done.set()

    threading.Thread(target=keep_lease, args=(work, work_id, environment_id, ended), daemon=True).start()
    runner = threading.Thread(target=SessionRun(Agentd(api, token, session_id), env).serve, daemon=True)
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
