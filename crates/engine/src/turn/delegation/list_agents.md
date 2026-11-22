List every subagent you've spawned, with its status. Also lists the
agent types you may spawn.

Replies arrive on their own, so you do not need this tool to receive
them. Call it when several agents are out and you want to see where
each one stands, or to look up a thread_id. How the loop works, when
to spawn and worked examples are in create_agent's description.

## Result

JSON text: {"agents": [...], "available": [...]}.
agents — one entry per spawned instance:
  thread_id ("sthr_…"), agent_name,
  status: PENDING | RUNNING | IDLE | ARCHIVED | UNSPECIFIED | UNKNOWN,
  parked: true when it stopped for a reason that needs outside action, with park_reason "requires_action" | "budget_reached" (null otherwise).
available — {name, description} for every agent type you may spawn.
