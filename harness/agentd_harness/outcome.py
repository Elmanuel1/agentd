"""`user.define_outcome`: the agent works, an independent grader grades the result against the
rubric in its own context, and its findings go back to the agent until the rubric is met, the
grader fails it, or the rounds run out (then one closing turn)."""

from .prompt import GRADER_SYSTEM, feedback_prompt, outcome_prompt
from .thread import Thread
from .tools import Policy

DEFAULT_MAX_ITERATIONS = 3
GRADER_TOOLS = ("bash", "read", "glob", "grep")
VERDICT = {"type": "function", "function": {
    "name": "submit_verdict", "description": "Submit the grade.",
    "parameters": {"type": "object", "properties": {
        "result": {"type": "string", "enum": ["satisfied", "needs_revision", "failed"]},
        "findings": {"type": "array", "items": {"type": "object", "properties": {
            "criterion": {"type": "string"}, "finding": {"type": "string"}}, "required": ["criterion", "finding"]}}},
        "required": ["result", "findings"]}}}


def explain(verdict):
    result = verdict.get("result") if verdict.get("result") in ("satisfied", "failed") else "needs_revision"
    findings = [f"- {f.get('criterion', '')}: {f.get('finding', '')}" for f in verdict.get("findings") or []]
    if result == "satisfied":
        return result, "All criteria are met."
    return result, "\n".join(findings) or "The grader gave no findings."


def rubric_text(run, rubric):
    if rubric.get("type") == "file":
        return run.agentd.file(rubric["file_id"]).decode("utf-8", "replace")
    return rubric.get("content") or ""


def grade(run, agent, rubric, description):
    """One grading round in a fresh context: `(result, explanation, usage)`."""
    policy = Policy({"tools": [{"type": "agent_toolset_20260401", "default_config": {"enabled": False},
                                "configs": [{"name": n, "enabled": True} for n in GRADER_TOOLS]}]})
    grader = Thread(run, {**agent, "name": "grader"}, GRADER_SYSTEM, policy.functions([VERDICT]), policy, record=False)
    verdict = {}

    def submit(tool_input):
        verdict.update(tool_input)
        grader.finished = True
        return [{"type": "text", "text": "Verdict recorded."}], False

    grader.handlers["submit_verdict"] = submit
    grader.send(f"# Task\n\n{description}\n\n# Rubric\n\n{rubric}\n\nGrade the deliverable in the sandbox.")
    if not verdict:
        return "failed", "The grader did not reach a verdict.", grader.usage
    result, explanation = explain(verdict)
    return result, explanation, grader.usage


def run_outcome(run, primary, event):
    outcome_id = event.get("outcome_id")
    rubric = rubric_text(run, event["rubric"])
    description = event.get("description") or ""
    max_iterations = event.get("max_iterations") or DEFAULT_MAX_ITERATIONS
    prompt = outcome_prompt(description, rubric)
    for iteration in range(max_iterations):
        run.turn(prompt)
        (start,) = run.agentd.emit([{"type": "span.outcome_evaluation_start", "outcome_id": outcome_id, "iteration": iteration}])
        result, explanation, used = grade(run, primary.agent, rubric, description)
        if result == "needs_revision" and iteration + 1 >= max_iterations:
            result = "max_iterations_reached"
        run.agentd.emit([{"type": "span.outcome_evaluation_end", "outcome_evaluation_start_id": start["id"],
                          "outcome_id": outcome_id, "result": result, "explanation": explanation,
                          "iteration": iteration, "usage": used}])
        if result in ("satisfied", "failed"):
            return result
        prompt = feedback_prompt(result, explanation)
        if result == "max_iterations_reached":
            run.turn(prompt)
            return result
    return "max_iterations_reached"
