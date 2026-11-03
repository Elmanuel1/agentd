use super::*;

fn req() -> RequestId {
    RequestId::generate()
}

#[test]
fn envelope_matches_live_shape() {
    let e = ApiError::invalid_request("model: Field required");
    let rid = req();
    let v = e.to_envelope(Some(&rid));
    assert_eq!(v["type"], "error");
    assert_eq!(v["error"]["type"], "invalid_request_error");
    assert_eq!(v["error"]["message"], "model: Field required");
    assert_eq!(v["request_id"], rid.as_str());
    // Exactly three top-level keys.
    assert_eq!(v.as_object().unwrap().len(), 3);
}

#[test]
fn memory_path_conflict_matches_live_shape() {
    // Live probe (409):
    // {"type":"error","error":{"type":"memory_path_conflict_error",
    //  "message":"path `/a.md` is already used by `mem_019f…`; use update to modify it",
    //  "conflicting_path":"/a.md","conflicting_memory_id":"mem_019f…"}, …}
    let e = ApiError::memory_path_conflict("/a.md", "mem_019fjZBeerJmg5Smuyy2mCS5");
    assert_eq!(e.status, 409);
    let v = e.to_envelope(Some(&req()));
    assert_eq!(v["error"]["type"], "memory_path_conflict_error");
    assert_eq!(
        v["error"]["message"],
        "path `/a.md` is already used by `mem_019fjZBeerJmg5Smuyy2mCS5`; use update to modify it"
    );
    assert_eq!(v["error"]["conflicting_path"], "/a.md");
    assert_eq!(v["error"]["conflicting_memory_id"], "mem_019fjZBeerJmg5Smuyy2mCS5");
}

#[test]
fn status_map_is_complete() {
    assert_eq!(ErrorType::InvalidRequest.status(), 400);
    assert_eq!(ErrorType::Authentication.status(), 401);
    assert_eq!(ErrorType::Permission.status(), 403);
    assert_eq!(ErrorType::NotFound.status(), 404);
    assert_eq!(ErrorType::MemoryPathConflict.status(), 409);
    assert_eq!(ErrorType::MemoryPreconditionFailed.status(), 409);
    assert_eq!(ErrorType::RequestTooLarge.status(), 413);
    assert_eq!(ErrorType::RateLimit.status(), 429);
    assert_eq!(ErrorType::Api.status(), 500);
    assert_eq!(ErrorType::Overloaded.status(), 529);
}

#[test]
fn conflict_invalid_request_is_409_with_invalid_request_type() {
    let e = ApiError::conflict_invalid_request("cannot archive while running");
    assert_eq!(e.status, 409);
    assert_eq!(e.error_type.wire_name(), "invalid_request_error");
}

#[test]
fn backend_error_preserves_source_chain_and_stays_eq() {
    use std::error::Error;
    let io = std::io::Error::other("disk on fire");
    let be = BackendError::new(io);
    // Display forwards to the wrapped cause; source() exposes it for logging.
    assert_eq!(be.to_string(), "disk on fire");
    assert_eq!(be.source().map(ToString::to_string), Some("disk on fire".to_string()));
    // All backend errors compare equal (variant identity, not the cause) so
    // enums that carry one keep deriving Eq/Clone.
    let other = BackendError::new(std::io::Error::other("different"));
    assert_eq!(be, other);
    let _cloned = be.clone();
}
