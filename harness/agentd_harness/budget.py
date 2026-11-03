"""The session's `budget`, as Anthropic enforces it: no new model request once the session's list
cost reaches `max_list_cost`, or once a model with no list price ran. The cost is OpenRouter's
`usage.cost` for each request (USD at list prices), kept run-wide across every thread."""

import threading
from decimal import ROUND_HALF_UP, Decimal


class BudgetReached(Exception):
    """The session's list cost reached its budget, or a model with no list price ran."""


class Budget:
    def __init__(self):
        self.lock = threading.Lock()
        self.limit = None
        self.cents = Decimal(0)
        self.unpriced = False

    def refresh(self, session):
        """Take the session's budget and the list cost agentd recorded; read at each batch of work."""
        usage = session.get("usage") or {}
        cost = usage.get("list_cost")
        budget = session.get("budget")
        with self.lock:
            self.limit = int(budget["max_list_cost"]["amount"]) if budget else None
            if cost:
                self.cents = max(self.cents, Decimal(cost["amount"]))
            elif usage.get("input_tokens") or usage.get("output_tokens") or usage.get("cache_read_input_tokens"):
                self.unpriced = True

    def check(self):
        """Raise BudgetReached instead of starting a model request the budget does not allow."""
        with self.lock:
            if self.limit is not None and (self.unpriced or self.cents >= self.limit):
                raise BudgetReached()

    def add(self, cost):
        """Add one request's cost in USD (None: its model has no list price); the session's list
        cost so far as a wire amount, or None once it is not measurable."""
        with self.lock:
            if cost is None:
                self.unpriced = True
            else:
                self.cents += Decimal(str(cost)) * 100
            if self.unpriced:
                return None
            return {"amount": str(self.cents.quantize(Decimal(1), ROUND_HALF_UP)), "currency": "USD"}
