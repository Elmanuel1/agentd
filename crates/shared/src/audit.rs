//! Who wrote a change. The edge, the scheduler and the CLI each run their
//! unit of work inside [`with_writer`]; the store reads [`Writer::current`]
//! for every row it writes.

use std::future::Future;

use crate::RequestId;

/// The actor and request behind a write.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Writer {
    /// The key id behind the request (`apikey_…`, `adminkey_…`), a work
    /// token's session (`sesn_…`), `operator` for the CLI, or `system`.
    pub actor: String,
    /// The request, scheduled fire or CLI run that wrote (`req_…`).
    pub request_id: String,
}

tokio::task_local! {
    static WRITER: Writer;
}

/// Run `fut` as `writer`.
pub async fn with_writer<F: Future>(writer: Writer, fut: F) -> F::Output {
    WRITER.scope(writer, fut).await
}

impl Writer {
    /// `actor`, writing as request `request_id`.
    pub fn new(actor: impl Into<String>, request_id: &RequestId) -> Self {
        Self { actor: actor.into(), request_id: request_id.as_str().to_owned() }
    }

    /// `actor`, writing as a request of its own.
    pub fn fresh(actor: impl Into<String>) -> Self {
        Self::new(actor, &RequestId::generate())
    }

    /// The scoped writer; outside any scope, `system` with a fresh request id.
    pub fn current() -> Self {
        WRITER.try_with(Clone::clone).unwrap_or_else(|_| Self::fresh("system"))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn inside_a_scope_the_writer_is_the_scoped_one() {
        let writer = Writer::fresh("apikey_a");
        let seen = with_writer(writer.clone(), async { Writer::current() }).await;
        assert_eq!(seen, writer);
    }

    #[tokio::test]
    async fn outside_any_scope_the_writer_is_system_with_its_own_request_id() {
        let (one, two) = (Writer::current(), Writer::current());
        assert_eq!(one.actor, "system");
        assert!(one.request_id.starts_with("req_"), "{}", one.request_id);
        assert_ne!(one.request_id, two.request_id);
    }
}
