//! Taking in a batch of client events: messages and outcomes go to the
//! primary thread; answers to waiting calls wake the threads that wait.

use std::collections::HashMap;

use sessions::events::{Event, EventPayload, Rubric, UserEvent};
use sessions::types::ThreadId;
use shared::{ApiError, ContentBlock};

use super::outcome::{self, Outcome};
use super::team::Team;
use super::SessionRun;
use crate::turn::history::outcome_prompt;
use crate::turn::Inbound;

/// What a batch of client events asks of the run.
#[derive(Debug, Default)]
pub(super) struct Intake {
    /// An outcome to work toward, newly defined.
    pub outcome: Option<Outcome>,
    /// A confirmation or tool result arrived for a waiting call.
    pub answered: bool,
}

fn texts(content: &[ContentBlock]) -> String {
    content.iter().filter_map(ContentBlock::as_text).collect::<Vec<_>>().join("\n")
}

/// A rubric's text: inline, or the file's from `files`.
pub(super) fn rubric_text(r: &Rubric, files: &HashMap<String, String>) -> String {
    match r {
        Rubric::Text { content } => content.clone(),
        Rubric::File { file_id } => files.get(file_id).cloned().unwrap_or_default(),
    }
}

impl SessionRun {
    /// Deliver `drained` to the threads; rubric files read for a new outcome
    /// are added to `rubrics`.
    pub(super) async fn intake(
        &self,
        drained: &[Event],
        team: &Team,
        primary: &ThreadId,
        rubrics: &mut HashMap<String, String>,
    ) -> Result<Intake, ApiError> {
        let mut got = Intake::default();
        for e in drained {
            let id = e.id.as_str().to_owned();
            match &e.payload {
                EventPayload::User(UserEvent::Message { content }) => {
                    team.deliver(primary, Some(Inbound { event: id, text: texts(content) }));
                }
                EventPayload::User(UserEvent::SystemMessage { content }) => {
                    let text = format!("<system>{}</system>", texts(content));
                    team.deliver(primary, Some(Inbound { event: id, text }));
                }
                EventPayload::User(UserEvent::DefineOutcome {
                    description,
                    rubric,
                    max_iterations,
                    outcome_id: Some(oid),
                }) => {
                    if let Rubric::File { file_id } = rubric {
                        if !rubrics.contains_key(file_id) {
                            let bytes = self.engine.assets.file(&self.ctx, file_id).await?;
                            rubrics.insert(file_id.clone(), String::from_utf8_lossy(&bytes).into_owned());
                        }
                    }
                    let text = rubric_text(rubric, rubrics);
                    got.outcome = Some(Outcome {
                        id: oid.clone(),
                        description: description.clone(),
                        rubric: text.clone(),
                        max_iterations: max_iterations.unwrap_or(outcome::DEFAULT_MAX_ITERATIONS),
                        iteration: 0,
                    });
                    team.deliver(
                        primary,
                        Some(Inbound { event: id, text: outcome_prompt(description, &text) }),
                    );
                }
                EventPayload::User(
                    UserEvent::ToolConfirmation { .. }
                    | UserEvent::CustomToolResult { .. }
                    | UserEvent::ToolResult { .. },
                ) => {
                    got.answered = true;
                }
                _ => {}
            }
        }
        Ok(got)
    }
}
