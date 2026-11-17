//! What adapters implement for the ports of the parent module; each comes with its `dyn` trait.

use super::*;

/// What an adapter implements to provide [`WorkReads`]; the methods are documented there.
pub trait WorkReadsImpl: Send + Sync {
    fn get(
        &self,
        ws: &WorkspaceId,
        session: &str,
        reach: &Reach,
    ) -> impl Future<Output = Result<Option<WorkItem>, StoreError>> + Send;
    fn find_on_queue(
        &self,
        queue: &str,
        session: &str,
    ) -> impl Future<Output = Result<Option<WorkItem>, StoreError>> + Send;
    fn list(
        &self,
        scope: &Scope,
        page: &Pagination,
    ) -> impl Future<Output = Result<Vec<WorkItem>, StoreError>> + Send;
    fn counts(
        &self,
        scope: &Scope,
        reclaim: Duration,
        now: Timestamp,
    ) -> impl Future<Output = Result<Counts, StoreError>> + Send;
    fn runtime_owner(
        &self,
        environment: &str,
    ) -> impl Future<Output = Result<Option<(OrgId, WorkspaceId)>, StoreError>> + Send;
}

impl<T: WorkReadsImpl> WorkReads for T {
    fn get<'a>(
        &'a self,
        ws: &'a WorkspaceId,
        session: &'a str,
        reach: &'a Reach,
    ) -> BoxFuture<'a, Result<Option<WorkItem>, StoreError>> {
        Box::pin(WorkReadsImpl::get(self, ws, session, reach))
    }
    fn find_on_queue<'a>(
        &'a self,
        queue: &'a str,
        session: &'a str,
    ) -> BoxFuture<'a, Result<Option<WorkItem>, StoreError>> {
        Box::pin(WorkReadsImpl::find_on_queue(self, queue, session))
    }
    fn list<'a>(
        &'a self,
        scope: &'a Scope,
        page: &'a Pagination,
    ) -> BoxFuture<'a, Result<Vec<WorkItem>, StoreError>> {
        Box::pin(WorkReadsImpl::list(self, scope, page))
    }
    fn counts<'a>(
        &'a self,
        scope: &'a Scope,
        reclaim: Duration,
        now: Timestamp,
    ) -> BoxFuture<'a, Result<Counts, StoreError>> {
        Box::pin(WorkReadsImpl::counts(self, scope, reclaim, now))
    }
    fn runtime_owner<'a>(
        &'a self,
        environment: &'a str,
    ) -> BoxFuture<'a, Result<Option<(OrgId, WorkspaceId)>, StoreError>> {
        Box::pin(WorkReadsImpl::runtime_owner(self, environment))
    }
}

/// What an adapter implements to provide [`WorkStore`]; the methods are documented there.
pub trait WorkStoreImpl: WorkReadsImpl {
    fn queue(&self, item: &WorkItem) -> impl Future<Output = Result<(), StoreError>> + Send;
    fn claim(
        &self,
        scope: &Scope,
        reclaim: Duration,
        now: Timestamp,
    ) -> impl Future<Output = Result<Option<WorkItem>, StoreError>> + Send;
    fn update_metadata(
        &self,
        ws: &WorkspaceId,
        session: &str,
        reach: &Reach,
        set: &BTreeMap<String, String>,
        remove: &[String],
    ) -> impl Future<Output = Result<Option<WorkItem>, StoreError>> + Send;
    fn ack(
        &self,
        ws: &WorkspaceId,
        session: &str,
        reach: &Reach,
        now: Timestamp,
        lease_until: Timestamp,
    ) -> impl Future<Output = Result<Option<WorkItem>, StoreError>> + Send;
    fn heartbeat(
        &self,
        ws: &WorkspaceId,
        session: &str,
        reach: &Reach,
        beat: &Beat,
    ) -> impl Future<Output = Result<Option<(WorkItem, bool)>, StoreError>> + Send;
    fn stop(
        &self,
        ws: &WorkspaceId,
        session: &str,
        reach: &Reach,
        force: bool,
        now: Timestamp,
    ) -> impl Future<Output = Result<Option<WorkItem>, StoreError>> + Send;
}

impl<T: WorkStoreImpl> WorkStore for T {
    fn queue<'a>(&'a self, item: &'a WorkItem) -> BoxFuture<'a, Result<(), StoreError>> {
        Box::pin(WorkStoreImpl::queue(self, item))
    }
    fn claim<'a>(
        &'a self,
        scope: &'a Scope,
        reclaim: Duration,
        now: Timestamp,
    ) -> BoxFuture<'a, Result<Option<WorkItem>, StoreError>> {
        Box::pin(WorkStoreImpl::claim(self, scope, reclaim, now))
    }
    fn update_metadata<'a>(
        &'a self,
        ws: &'a WorkspaceId,
        session: &'a str,
        reach: &'a Reach,
        set: &'a BTreeMap<String, String>,
        remove: &'a [String],
    ) -> BoxFuture<'a, Result<Option<WorkItem>, StoreError>> {
        Box::pin(WorkStoreImpl::update_metadata(self, ws, session, reach, set, remove))
    }
    fn ack<'a>(
        &'a self,
        ws: &'a WorkspaceId,
        session: &'a str,
        reach: &'a Reach,
        now: Timestamp,
        lease_until: Timestamp,
    ) -> BoxFuture<'a, Result<Option<WorkItem>, StoreError>> {
        Box::pin(WorkStoreImpl::ack(self, ws, session, reach, now, lease_until))
    }
    fn heartbeat<'a>(
        &'a self,
        ws: &'a WorkspaceId,
        session: &'a str,
        reach: &'a Reach,
        beat: &'a Beat,
    ) -> BoxFuture<'a, Result<Option<(WorkItem, bool)>, StoreError>> {
        Box::pin(WorkStoreImpl::heartbeat(self, ws, session, reach, beat))
    }
    fn stop<'a>(
        &'a self,
        ws: &'a WorkspaceId,
        session: &'a str,
        reach: &'a Reach,
        force: bool,
        now: Timestamp,
    ) -> BoxFuture<'a, Result<Option<WorkItem>, StoreError>> {
        Box::pin(WorkStoreImpl::stop(self, ws, session, reach, force, now))
    }
}
