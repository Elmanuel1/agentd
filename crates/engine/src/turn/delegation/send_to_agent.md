Reply to a subagent. Use this to answer its questions, give it more
context, or hand it follow-up work. The agent keeps its own history,
so a follow-up can be short, but it still cannot see yours: say what
changed and what you want back.

## When an agent notification arrives, read it carefully

The agent's message is either a question or a report. You have to
tell them apart from prose alone — there is no flag.

- It asked something. It's blocked waiting on you. Reply with this
  tool: answer the question, give it what it needs, then end your
  turn. Its next message arrives the same way.

  Example — the notification says:
  "The task says optimize the query but there are three slow ones
   in this file. Which did you mean, or all of them?"
  You reply with this tool, passing the thread_id from the
  notification header and your answer in message.

- It delivered results. You're done with that thread. Do NOT reply
  just to say "thanks" or "got it" — that wakes the agent for
  nothing. Weigh what it found (see below), then use it: relay it to
  the user, feed it into another agent's task, act on it.

  Example — the agent sent:
  "Found it. The N+1 is in orders.py:142 — the loop calls
   .product.name on each item. Batch the product fetch."
  You: tell the user, or go fix it. No send_to_agent call.

When it's ambiguous, treat it as a report. A wrongly-ignored
question costs one round-trip; a wrongly-acknowledged report wakes
an idle agent and confuses it.

## A report is evidence, not fact

The user relies on you, not on the agent, so what you pass on is still
yours to check. A report can answer an easier question than the one
you asked, name a file that is not there, or call something done that
is not, and an agent can change more than you asked it to. Check in
proportion to the stakes: a search for a string needs no audit, a
claimed root cause deserves a second look, and a change reported as
made and tested is worth a glance at what changed.

This call returns before the agent has replied. The agent's response will arrive in a future turn as a new message — it will NOT appear in this turn. So end your turn after you send. Do NOT invent, predict, or describe what the agent might say.

## Result

JSON text: {"delivered": true, "thread_id": "sthr_…", "queued": false, "note": "…"}.
queued is true when the agent is paused on the session budget: the message is stored and read when it resumes.
note says whether to expect a reply now.
