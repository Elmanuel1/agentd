//! The system prompt a thread's model gets: the agent's own, then its
//! skills (name, description and where SKILL.md is), then, for a
//! coordinator, its roster.

use agents::AgentSnapshot;
use sessions::types::OutcomeResult;

/// Where an agent's skills are unpacked in the sandbox.
pub const SKILLS_DIR: &str = "/workspace/skills";

/// A skill as the prompt lists it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SkillInfo {
    /// The skill's directory name under [`SKILLS_DIR`].
    pub name: String,
    /// What it covers, from its SKILL.md front matter.
    pub description: String,
}

/// The section naming the session's credentials: each variable, what it is
/// for and where it may be sent. Values never reach the model.
pub fn credentials_section(credentials: &[(String, String, Option<Vec<String>>)]) -> String {
    if credentials.is_empty() {
        return String::new();
    }
    let mut out = String::from("\n\n# Credentials\n\nThese environment variables hold placeholders; the egress proxy puts the real value into requests to the hosts listed, and refuses to send it anywhere else.\n\n");
    for (name, description, hosts) in credentials {
        let hosts = hosts.as_ref().map_or_else(|| "no host; it lists none".to_owned(), |h| h.join(", "));
        out.push_str(&format!("- `{name}`: {description} (sent to: {hosts})\n"));
    }
    out
}

/// The system prompt for `agent`, ending with `notes` (the credentials
/// section).
pub fn system_prompt(
    agent: &AgentSnapshot,
    skills: &[SkillInfo],
    roster: &[&AgentSnapshot],
    notes: &str,
) -> String {
    let mut out = agent.config.system.clone().unwrap_or_default();
    if !skills.is_empty() {
        out.push_str("\n\n# Skills\n\nBefore doing work a skill covers, read its SKILL.md with the `read` tool; its scripts and references sit beside it.\n\n");
        for s in skills {
            out.push_str(&format!("- `{}` ({SKILLS_DIR}/{}/SKILL.md): {}\n", s.name, s.name, s.description));
        }
    }
    if !roster.is_empty() {
        out.push_str("\n\n# Your roster\n\nDelegate with `send_to_agent`. Each new task opens a thread; send follow-ups to that thread's id.\n\n");
        for a in roster {
            out.push_str(&format!(
                "- `{}`: {}\n",
                a.config.name,
                a.config.description.as_deref().unwrap_or("")
            ));
        }
    }
    out.push_str(notes);
    out
}

/// A skill's description from its SKILL.md front matter.
pub fn skill_description(skill_md: &str) -> String {
    let Some(front) = skill_md.strip_prefix("---\n").and_then(|rest| rest.split("\n---").next()) else {
        return String::new();
    };
    front
        .lines()
        .find_map(|l| l.strip_prefix("description:"))
        .map(|d| d.trim().trim_matches('"').to_owned())
        .unwrap_or_default()
}

/// The text the agent gets for an outcome: the task and the rubric it is
/// graded on.
pub fn outcome_prompt(description: &str, rubric: &str) -> String {
    format!("{description}\n\nAn independent grader will grade the result against this rubric:\n\n{rubric}")
}

/// The text the agent gets after a grading round.
pub fn feedback_prompt(result: OutcomeResult, explanation: &str) -> String {
    match result {
        OutcomeResult::MaxIterationsReached => format!(
            "The grader's last round still asks for revisions, and no rounds are left:\n\n{explanation}\n\nSummarize where the work stands; it will not be graded again."
        ),
        _ => format!("{explanation}\n\nRevise the work to address these findings. It will be graded again."),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_description_comes_from_the_front_matter() {
        let md = "---\nname: citing-sources\ndescription: Resolves snippets to pages.\n---\n# Citing";
        assert_eq!(skill_description(md), "Resolves snippets to pages.");
        assert_eq!(skill_description("# no front matter"), "");
    }
}
