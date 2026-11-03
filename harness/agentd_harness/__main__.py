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


def keep_lease(work, work_id, environment_id, lost):
    """Heartbeat the lease until it is lost or the work is stopping; then set `lost`."""
    last = "NO_HEARTBEAT"
    while not lost.is_set():
        try:
            beat = work.heartbeat(work_id, environment_id=environment_id, desired_ttl_seconds=LEASE_TTL,
                                  expected_last_heartbeat=last)
            last = beat.last_heartbeat
            if beat.state in ("stopping", "stopped"):
                break
        except anthropic.APIStatusError as e:
            if e.status_code in (404, 409, 412):
                break
        except anthropic.APIConnectionError:
            pass
        time.sleep(BEAT_EVERY)
    lost.set()


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
    lost = threading.Event()
    threading.Thread(target=keep_lease, args=(work, work_id, environment_id, lost), daemon=True).start()
    runner = threading.Thread(target=SessionRun(Agentd(api, token, session_id), env).serve, daemon=True)
    runner.start()
    while runner.is_alive() and not lost.is_set():
        runner.join(timeout=1)
    try:
        work.stop(work_id, environment_id=environment_id, force=True)
    except anthropic.APIError:
        pass
    return 0


if __name__ == "__main__":
    sys.exit(main())
