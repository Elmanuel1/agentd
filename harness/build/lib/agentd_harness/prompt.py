"""The system prompt a thread's model gets: the agent's own, then its skills, then, for a
coordinator, its roster, then the session's credentials. And the texts an outcome round adds."""

SKILLS_DIR = "/workspace/skills"

GRADER_SYSTEM = (
    "You are an outcome grader. You grade a deliverable against a rubric, independently: you did not "
    "produce it, and you check it against the source files in the sandbox, not against what such work "
    "usually says. Read the rubric in the message. Treat each `###` heading (or each numbered rule, when "
    "there are no headings) as one criterion. For each: reason, check the deliverable's values against "
    "the source material with the tools, and decide whether it passes. Change no file. When done, call "
    "`submit_verdict` once: `satisfied` only when every criterion passes, `needs_revision` when any "
    "fails, `failed` when the rubric does not apply to the deliverable. In `findings`, list every failed "
    "criterion with what is wrong, where in the deliverable, and the evidence that shows it.")


def skill_description(skill_md):
    """A skill's description from its SKILL.md front matter."""
    if not skill_md.startswith("---\n"):
        return ""
    front = skill_md[4:].split("\n---", 1)[0]
    for line in front.splitlines():
        if line.startswith("description:"):
            return line[len("description:"):].strip().strip('"')
    return ""


def credentials_section(credentials):
    """Each credential variable, what it is for and where it may be sent. Never a value."""
    if not credentials:
        return ""
    out = ("\n\n# Credentials\n\nThese environment variables hold placeholders; the egress proxy puts the "
           "real value into requests to the hosts listed, and refuses to send it anywhere else.\n\n")
    for c in credentials:
        hosts = ", ".join(c["hosts"]) if c.get("hosts") else "no host; it lists none"
        out += f"- `{c['name']}`: {c.get('description', '')} (sent to: {hosts})\n"
    return out


def system_prompt(agent, skills, roster, credentials):
    out = agent.get("system") or ""
    if skills:
        out += ("\n\n# Skills\n\nBefore doing work a skill covers, read its SKILL.md with the `read` tool; "
                "its scripts and references sit beside it.\n\n")
        for name, description in skills:
            out += f"- `{name}` ({SKILLS_DIR}/{name}/SKILL.md): {description}\n"
    if roster:
        out += ("\n\n# Your roster\n\nDelegate with `send_to_agent`. Each new task opens a thread; send "
                "follow-ups to that thread's id.\n\n")
        for a in roster:
            out += f"- `{a['name']}`: {a.get('description') or ''}\n"
    return out + credentials_section(credentials)


def outcome_prompt(description, rubric):
    return f"{description}\n\nAn independent grader will grade the result against this rubric:\n\n{rubric}"


def feedback_prompt(result, explanation):
    if result == "max_iterations_reached":
        return (f"The grader's last round still asks for revisions, and no rounds are left:\n\n{explanation}\n\n"
                "Summarize where the work stands; it will not be graded again.")
    return f"{explanation}\n\nRevise the work to address these findings. It will be graded again."
