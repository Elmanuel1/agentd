Spawn a subagent and give it a task. The agent runs in the background
with its own context window and tools; this call returns immediately
with the thread_id to address it by.

## How the loop works

You spawn, you end your turn, the agent's reply arrives as a new
message in your conversation tagged <agent-notification>. That tag is
how you tell agent traffic apart from the human user. From there it's
a conversation: if the agent asked something, reply with send_to_agent
using the thread_id from the notification; if it delivered results,
you're done with that thread: weigh what it found, then use it. Call
list_agents at any time to see where every agent stands.

## Writing the task

The task is all the agent knows of the work. It cannot see your
conversation, the files you have read or what the user said four turns
ago, so a line that is a complete thought to you ("find why the auth
tests fail") leaves it guessing. Write the way you would brief a
capable colleague who is new to this: in plain prose, at whatever
length that takes. Three or four short paragraphs are normal.

Say what the work is for and not only what to do, because an agent
that knows why you want a summary knows what to look for and what to
skip. Say what done looks like, since it has no natural stopping
point. Say what you already know and have ruled out, or it will
rediscover it. Give a constraint with its reason, so that the agent
can judge the cases you did not foresee, and pass on the user's
preferences that bear on this piece.

Spend your words where a smart newcomer would go wrong. The agent will
be thorough and careful without being told. What it cannot guess is
what makes this task hard: where there is an obvious wrong way and a
subtle right one, name both, or it may solve the easier problem and
report success.

Say what it may touch. It has real tools and less context than you
have. If the work is read-only or belongs in one directory, say so,
and if a step would be hard to undo, allow it, forbid it, or ask the
agent to report back instead of acting.

Say what you want back. The reply is raw material for your next step,
not something the user will read, so ask for the shape that makes that
step easy: a plain yes or no before the details where you will branch
on it, the same format from agents whose findings you will merge, and
what was checked kept apart from what was inferred.

Two checks before you send it. Would a capable contractor holding only
this task do excellent work, or spend the first twenty minutes
rebuilding what three sentences could have told them? And if the user
read it over your shoulder, would they say that it captures what they
need?

A vague task costs you a clarifying question and a round-trip, or,
worse, a confident answer to a question you did not ask.

## When to spawn

Spawn when the user's request splits into independent pieces that can
run in parallel, when one piece needs to read so much that it would
crowd your own context, or when a long search can run while you get on
with other work. Don't spawn for things you can answer directly: the
agent starts from nothing, so you pay to explain the context again,
plus a round-trip of latency.

## One task or a conversation

Ask whether the agent will need something it can only get by showing
you work first: a draft for you to react to, a finding that changes
the question. If so, plan for a conversation. Give it the goal and the
first step, read what comes back, and steer with send_to_agent. Trying
to front-load every preference into one task does worse, because you
cannot foresee what the first draft will reveal. If the inputs are
known and what you want back is clear, it is one task: write it in
full and expect one reply, since a round spent on a question that the
task could have answered is wasted. If you also hold
`run_workflow`, many one-task jobs of the same shape are a
workflow: there an agent gets one brief, answers once and cannot ask.

## Agents available in this session

{roster}
## Two examples

These show how much a task explains. They are illustrations, not
templates: write yours from the work in front of you.
`researcher` stands in for an agent: pass a name from the list above.

Parallel review, one report. User: "Security-review the auth module
and the billing module before we open to outside customers."

Turn 1 — you spawn both and end your turn:
  create_agent({agent_name: "researcher", task: "The user is about to
    open this service to outside customers and wants a security review
    first. I am splitting it by module, and yours is auth/. Please look
    at token validation, session handling and CSRF. What matters is
    anything an outsider could exploit, so leave style and nice-to-have
    hardening aside. Read only: change nothing. Report each finding as
    file:line, what an attacker could do with it, and whether you
    traced the code path or are inferring. If an area is clean, say
    so."})
  create_agent({agent_name: "researcher", task: the same opening and
    report format, then "yours is billing/. Please look at input
    validation on amounts, idempotency on charges and PII in logs."})

Turn 2 — both notifications arrive, each header carrying the sender's
thread_id. Both are reports in the same shape, so they merge easily.
Neither needs a reply. You look into the findings marked as inferred
before you pass them on, then synthesize for the user.

A planned conversation. User: "Figure out why checkout is slow."
You cannot say yet which part to dig into, so you plan for two rounds.

Turn 1 — you spawn:
  create_agent({agent_name: "researcher", task: "The user says checkout
    is slow, and nobody knows yet where the time goes. Please profile
    the flow end to end and send me the three slowest operations with
    timings. Don't fix or dig into anything yet: I will pick one with
    the user's priorities in mind. The staging data is small, so treat
    a query that scales with order count as suspect even if it is fast
    today."})

Turn 2 — it reports three operations. You pick one and reply with
send_to_agent, passing the thread_id from the notification header.

Turn 3 — it reports a cause. A claimed root cause is worth a look at
the code it names before you tell the user.

This call returns before the agent has finished. The agent's response will arrive in a future turn as a new message — it will NOT appear in this turn. So end your turn once you have spawned what you need. Do NOT invent, predict, or describe what the agent might say.

## Result

JSON text: {"thread_id": "sthr_…", "agent_name": "…", "note": "…"}.
thread_id addresses the spawned agent: send_to_agent takes it and list_agents reports it.
note repeats the reminder above: the reply arrives in a later turn, so end this one.
