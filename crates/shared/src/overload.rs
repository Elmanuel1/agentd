//! Load shedding (D14): a request whose database checkout timed out answers
//! 529 `overloaded_error` on any route, as live does under load, whatever
//! error the route made of the timeout. The pool marks the request; the edge
//! reads the mark. Background work has no mark and retries as its own path does.

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

tokio::task_local! {
    static WAITED_OUT: Arc<AtomicBool>;
}

/// Run `fut` as one request; `true` alongside its output when a checkout in it timed out.
pub async fn watched<F: Future>(fut: F) -> (F::Output, bool) {
    let mark = Arc::new(AtomicBool::new(false));
    let output = WAITED_OUT.scope(mark.clone(), fut).await;
    (output, mark.load(Ordering::Relaxed))
}

/// A database checkout of the current request timed out.
pub fn waited_out() {
    let _ = WAITED_OUT.try_with(|mark| mark.store(true, Ordering::Relaxed));
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn only_a_request_that_waited_out_is_marked() {
        assert!(!watched(async {}).await.1);
        assert!(watched(async { waited_out() }).await.1);
        waited_out();
    }
}
