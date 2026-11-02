//! The threads of one session run, and delegation between them: a
//! coordinator's `send_to_agent` opens or follows up a roster agent's thread,
//! and the thread's reply comes back to the coordinator as a message.

use std::collections::HashMap;
use std::sync::Arc;

use agents::AgentSnapshot;
use parking_lot::Mutex;
use sessions::events::{AgentEvent, SessionEvent};
use sessions::types::ThreadId;
use sessions::EngineContract;
use shared::ApiError;
use tokio::sync::Notify;

use crate::turn::policy::Toolbox;
use crate::turn::prompt::{system_prompt, SkillInfo};
use crate::turn::{Inbound, Inbox, ThreadCtx};

/// One thread and its queue.
#[derive(Debug)]
pub struct Member {
    /// The thread.
    pub ctx: ThreadCtx,
    /// Its new input.
    pub inbox: Inbox,
}

/// The session's threads, shared by the worker and the coordinator's tools.
pub struct Team {
    contract: Arc<dyn EngineContract>,
    primary: ThreadCtx,
    roster: Vec<AgentSnapshot>,
    skills: HashMap<String, Vec<SkillInfo>>,
    /// The system prompt's closing notes (the credentials section).
    notes: String,
    members: Mutex<HashMap<ThreadId, Arc<Member>>>,
    /// Threads with input to answer, in arrival order.
    runnable: Mutex<Vec<ThreadId>>,
    /// Threads whose turn is under way.
    busy: Mutex<std::collections::HashSet<ThreadId>>,
    wake: Arc<Notify>,
}

impl std::fmt::Debug for Team {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Team").field("primary", &self.primary.id).finish_non_exhaustive()
    }
}

impl Team {
    /// A team led by `primary`, with `roster` to delegate to; `wake` is
    /// notified whenever a thread gets input.
    pub fn new(
        contract: Arc<dyn EngineContract>,
        primary: ThreadCtx,
        roster: Vec<AgentSnapshot>,
        skills: HashMap<String, Vec<SkillInfo>>,
        notes: String,
        wake: Arc<Notify>,
    ) -> Self {
        let members = HashMap::from([(
            primary.id.clone(),
            Arc::new(Member { ctx: primary.clone(), inbox: Inbox::default() }),
        )]);
        Self {
            contract,
            primary,
            roster,
            skills,
            notes,
            members: Mutex::new(members),
            runnable: Mutex::new(Vec::new()),
            busy: Mutex::default(),
            wake,
        }
    }

    /// The primary thread.
    pub fn primary(&self) -> &ThreadCtx {
        &self.primary
    }

    /// Add an existing thread of roster agent `agent_name` (read from the log).
    pub fn restore(&self, id: ThreadId, agent_name: &str) {
        if let Some(agent) = self.roster.iter().find(|a| a.config.name == agent_name) {
            let ctx = self.thread_ctx(id.clone(), agent.clone());
            self.members.lock().insert(id, Arc::new(Member { ctx, inbox: Inbox::default() }));
        }
    }

    fn thread_ctx(&self, id: ThreadId, agent: AgentSnapshot) -> ThreadCtx {
        let skills = self.skills.get(&agent.config.name).cloned().unwrap_or_default();
        ThreadCtx {
            ws: self.primary.ws.clone(),
            session: self.primary.session.clone(),
            scope: Some(id.clone()),
            id,
            toolbox: Toolbox::for_agent(&agent, false),
            system: system_prompt(&agent, &skills, &[], &self.notes),
            threaded: true,
            agent,
        }
    }

    /// A thread by id.
    pub fn member(&self, id: &ThreadId) -> Option<Arc<Member>> {
        self.members.lock().get(id).cloned()
    }

    /// Every thread.
    pub fn members(&self) -> Vec<Arc<Member>> {
        self.members.lock().values().cloned().collect()
    }

    /// Queue `item` for thread `id` and mark it runnable.
    pub fn deliver(&self, id: &ThreadId, item: Option<Inbound>) {
        if let (Some(m), Some(item)) = (self.member(id), item) {
            m.inbox.push(item);
        }
        self.mark(id);
    }

    /// Mark thread `id` runnable.
    pub fn mark(&self, id: &ThreadId) {
        let mut r = self.runnable.lock();
        if !r.contains(id) {
            r.push(id.clone());
        }
        drop(r);
        self.wake.notify_one();
    }

    /// Record whether thread `id`'s turn is under way.
    pub fn set_busy(&self, id: &ThreadId, busy: bool) {
        let mut b = self.busy.lock();
        if busy {
            b.insert(id.clone());
        } else {
            b.remove(id);
        }
    }

    /// Take the runnable threads.
    pub fn take_runnable(&self) -> Vec<ThreadId> {
        std::mem::take(&mut *self.runnable.lock())
    }

    async fn message(&self, scope: Option<&ThreadId>, ev: AgentEvent) -> Result<sessions::Event, ApiError> {
        self.contract.emit(&self.primary.ws, &self.primary.session, scope, ev.into()).await
    }

    async fn open(&self, agent_name: &str) -> Result<Arc<Member>, String> {
        let agent = self.roster.iter().find(|a| a.config.name == agent_name).cloned().ok_or_else(|| {
            let names: Vec<&str> = self.roster.iter().map(|a| a.config.name.as_str()).collect();
            format!("`{agent_name}` is not on your roster ({})", names.join(", "))
        })?;
        let id = ThreadId::generate();
        let created = SessionEvent::ThreadCreated { agent_name: agent_name.to_owned() };
        self.contract
            .emit(&self.primary.ws, &self.primary.session, Some(&id), created.into())
            .await
            .map_err(|e| e.message)?;
        let member = Arc::new(Member { ctx: self.thread_ctx(id.clone(), agent), inbox: Inbox::default() });
        self.members.lock().insert(id, member.clone());
        Ok(member)
    }
}

mod delegation;
