//! The organization rows: made by `agentd organization create`, removed with everything they own by
//! `agentd organization delete`.

use std::future::Future;

use shared::ports::secrets::SecretKey;
use shared::ports::{BoxFuture, FireTarget};
use shared::{Id, OrgId, Resource};

/// Marker for generated `org_…` ids.
#[derive(Debug, Clone, Copy)]
pub struct OrganizationResource;
impl Resource for OrganizationResource {
    const PREFIX: &'static str = "org";
    const LABEL: &'static str = "organization";
}

/// A new organization's id: `requested` as given, else a generated `org_…` id.
pub fn new_id(requested: Option<&str>) -> OrgId {
    requested.map_or_else(|| OrgId::new(Id::<OrganizationResource>::generate().as_str()), OrgId::new)
}

/// Store-level errors.
#[derive(Debug, Clone, thiserror::Error)]
pub enum StoreError {
    /// An organization with this id exists already.
    #[error("organization {0} already exists")]
    Exists(String),
    /// No organization has this id.
    #[error("no organization {0}")]
    NotFound(String),
    /// The backing store failed (infrastructure error).
    #[error("store backend: {0}")]
    Backend(#[source] shared::BackendError),
}

/// What an organization delete removed in its transaction, and what lives outside the database.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Removed {
    /// The organization's workspaces: its `workspaces` rows and the workspaces its keys name.
    pub workspaces: Vec<String>,
    /// Rows deleted from each workspace-scoped table, by table.
    pub rows: Vec<(&'static str, u64)>,
    /// The secrets of the credentials it deleted.
    pub secrets: Vec<SecretKey>,
    /// The blob keys of the files, skill versions and memory versions it deleted.
    pub blobs: Vec<String>,
    /// The schedules of the dreams it deleted that were not archived.
    pub dreams: Vec<FireTarget>,
}

/// Persistence port for organizations.
pub trait OrganizationsStore: Send + Sync {
    fn create<'a>(&'a self, org: &'a OrgId) -> BoxFuture<'a, Result<(), StoreError>>;
    /// Delete `org` and every row it owns in one transaction, writing each removed deployment's
    /// schedule delete into the outbox.
    fn delete<'a>(&'a self, org: &'a OrgId) -> BoxFuture<'a, Result<Removed, StoreError>>;
}

/// What an adapter implements to provide [`OrganizationsStore`]; the methods are documented there.
pub trait NativeOrganizationsStore: Send + Sync {
    fn create(&self, org: &OrgId) -> impl Future<Output = Result<(), StoreError>> + Send;
    fn delete(&self, org: &OrgId) -> impl Future<Output = Result<Removed, StoreError>> + Send;
}

impl<T: NativeOrganizationsStore> OrganizationsStore for T {
    fn create<'a>(&'a self, org: &'a OrgId) -> BoxFuture<'a, Result<(), StoreError>> {
        Box::pin(NativeOrganizationsStore::create(self, org))
    }
    fn delete<'a>(&'a self, org: &'a OrgId) -> BoxFuture<'a, Result<Removed, StoreError>> {
        Box::pin(NativeOrganizationsStore::delete(self, org))
    }
}
