/// A wrapped infrastructure error carried by every entity store's `Backend`
/// variant. It preserves the underlying error as a `source()` chain (for
/// structured logging / `{:?}`) instead of flattening it to a `String`, while
/// staying `Clone` (`Arc`) and `Eq` (all backend errors compare equal — the
/// variant is what control flow branches on; the wrapped cause is diagnostic
/// only). This lets `StoreError` keep deriving `Clone`/`PartialEq`/`Eq`.
#[derive(Clone, Debug)]
pub struct BackendError(std::sync::Arc<dyn std::error::Error + Send + Sync>);

impl BackendError {
    /// Wrap a concrete source error.
    pub fn new<E: std::error::Error + Send + Sync + 'static>(source: E) -> Self {
        Self(std::sync::Arc::new(source))
    }
}

impl std::fmt::Display for BackendError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        std::fmt::Display::fmt(&self.0, f)
    }
}

impl std::error::Error for BackendError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        Some(&*self.0)
    }
}

impl PartialEq for BackendError {
    fn eq(&self, _: &Self) -> bool {
        true
    }
}

impl Eq for BackendError {}
