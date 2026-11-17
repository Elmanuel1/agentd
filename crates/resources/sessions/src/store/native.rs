//! What adapters implement for the ports of the parent module; each comes with its `dyn` trait.

use super::*;

/// What an adapter implements to provide [`SessionReplace`]; the methods are documented there.
pub trait SessionReplaceImpl: Send + Sync {
    fn replace(
        &self,
        ws: &WorkspaceId,
        read: &Session,
        s: Session,
        deliveries: &[webhooks::Delivery],
    ) -> impl Future<Output = Result<(), StoreError>> + Send;
}

impl<T: SessionReplaceImpl> SessionReplace for T {
    fn replace<'a>(
        &'a self,
        ws: &'a WorkspaceId,
        read: &'a Session,
        s: Session,
        deliveries: &'a [webhooks::Delivery],
    ) -> BoxFuture<'a, Result<(), StoreError>> {
        Box::pin(SessionReplaceImpl::replace(self, ws, read, s, deliveries))
    }
}

/// What an adapter implements to provide [`SessionsStore`]; the methods are documented there.
pub trait SessionsStoreImpl: SessionReplaceImpl {
    fn insert(&self, ws: &WorkspaceId, s: Session) -> impl Future<Output = Result<(), StoreError>> + Send;
    fn get(
        &self,
        ws: &WorkspaceId,
        id: &SessionId,
    ) -> impl Future<Output = Result<Session, StoreError>> + Send;
    fn delete(
        &self,
        ws: &WorkspaceId,
        id: &SessionId,
    ) -> impl Future<Output = Result<Session, StoreError>> + Send;
    fn list(
        &self,
        ws: &WorkspaceId,
        filter: &SessionFilter,
        page: &Pagination,
    ) -> impl Future<Output = Result<Vec<Session>, StoreError>> + Send;
}

impl<T: SessionsStoreImpl> SessionsStore for T {
    fn insert<'a>(&'a self, ws: &'a WorkspaceId, s: Session) -> BoxFuture<'a, Result<(), StoreError>> {
        Box::pin(SessionsStoreImpl::insert(self, ws, s))
    }
    fn get<'a>(
        &'a self,
        ws: &'a WorkspaceId,
        id: &'a SessionId,
    ) -> BoxFuture<'a, Result<Session, StoreError>> {
        Box::pin(SessionsStoreImpl::get(self, ws, id))
    }
    fn delete<'a>(
        &'a self,
        ws: &'a WorkspaceId,
        id: &'a SessionId,
    ) -> BoxFuture<'a, Result<Session, StoreError>> {
        Box::pin(SessionsStoreImpl::delete(self, ws, id))
    }
    fn list<'a>(
        &'a self,
        ws: &'a WorkspaceId,
        filter: &'a SessionFilter,
        page: &'a Pagination,
    ) -> BoxFuture<'a, Result<Vec<Session>, StoreError>> {
        Box::pin(SessionsStoreImpl::list(self, ws, filter, page))
    }
}

/// What an adapter implements to provide [`SessionEventLog`]; the methods are documented there.
pub trait SessionEventLogImpl: SessionEventReadsImpl {
    fn append_events(
        &self,
        ws: &WorkspaceId,
        id: &SessionId,
        events: &[Event],
        wake: Wake,
    ) -> impl Future<Output = Result<(), StoreError>> + Send;
    fn update_processed_at(
        &self,
        ws: &WorkspaceId,
        id: &SessionId,
        ids: &[EventId],
        at: Timestamp,
    ) -> impl Future<Output = Result<Vec<Event>, StoreError>> + Send;
    fn append_answers(
        &self,
        ws: &WorkspaceId,
        id: &SessionId,
        calls: &[EventId],
        events: &[Event],
        check: &AnswerCheck<'_>,
        wake: Wake,
    ) -> impl Future<Output = Result<Result<(), ApiError>, StoreError>> + Send;
    fn delete_events_of(
        &self,
        sessions: &[(WorkspaceId, SessionId)],
    ) -> impl Future<Output = Result<(), StoreError>> + Send;
}

impl<T: SessionEventLogImpl> SessionEventLog for T {
    fn append_events<'a>(
        &'a self,
        ws: &'a WorkspaceId,
        id: &'a SessionId,
        events: &'a [Event],
        wake: Wake,
    ) -> BoxFuture<'a, Result<(), StoreError>> {
        Box::pin(SessionEventLogImpl::append_events(self, ws, id, events, wake))
    }
    fn update_processed_at<'a>(
        &'a self,
        ws: &'a WorkspaceId,
        id: &'a SessionId,
        ids: &'a [EventId],
        at: Timestamp,
    ) -> BoxFuture<'a, Result<Vec<Event>, StoreError>> {
        Box::pin(SessionEventLogImpl::update_processed_at(self, ws, id, ids, at))
    }
    fn append_answers<'a>(
        &'a self,
        ws: &'a WorkspaceId,
        id: &'a SessionId,
        calls: &'a [EventId],
        events: &'a [Event],
        check: &'a AnswerCheck<'a>,
        wake: Wake,
    ) -> BoxFuture<'a, Result<Result<(), ApiError>, StoreError>> {
        Box::pin(SessionEventLogImpl::append_answers(self, ws, id, calls, events, check, wake))
    }
    fn delete_events_of<'a>(
        &'a self,
        sessions: &'a [(WorkspaceId, SessionId)],
    ) -> BoxFuture<'a, Result<(), StoreError>> {
        Box::pin(SessionEventLogImpl::delete_events_of(self, sessions))
    }
}
