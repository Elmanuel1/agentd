use super::*;

fn claims() -> WorkClaims {
    WorkClaims { session: "sesn_1".into(), ws: "w".into(), org: "o".into() }
}

#[test]
fn a_work_token_reaches_its_own_item_session_and_the_worker_s_routes() {
    let (get, post, del) = (Method::GET, Method::POST, Method::DELETE);
    for (method, path) in [
        (&post, "/v1/environments/env_1/work/sesn_1/heartbeat"),
        (&post, "/v1/environments/runtime/work/sesn_1/stop"),
        (&get, "/v1/environments/env_1/work/sesn_1"),
        (&get, "/v1/sessions/sesn_1"),
        (&get, "/v1/sessions/sesn_1/events"),
        (&post, "/v1/sessions/sesn_1/events"),
        (&get, "/v1/sessions/sesn_1/events/stream"),
        (&post, "/v1/sessions/sesn_1/harness/events"),
        (&get, "/v1/files/file_1/content"),
        (&get, "/v1/files/file_1"),
        (&post, "/v1/files"),
        (&del, "/v1/files/file_1"),
        (&get, "/v1/skills/skill_1"),
        (&get, "/v1/skills/skill_1/versions/1"),
        (&get, "/v1/skills/skill_1/versions/1/content"),
        (&get, "/v1/memory_stores/memstore_1/memories"),
        (&post, "/v1/memory_stores/memstore_1/memories/mem_1"),
        (&del, "/v1/memory_stores/memstore_1/memories/mem_1"),
    ] {
        assert!(work_route(method, path, &claims()), "{method} {path}");
    }
    for (method, path) in [
        (&get, "/v1/environments/env_1/work/sesn_2"),
        (&get, "/v1/environments/env_1/work/poll"),
        (&get, "/v1/environments/env_1/work/stats"),
        (&get, "/v1/environments/env_1/work"),
        (&post, "/v1/environments/env_1/work/sesn_1"),
        (&post, "/v1/sessions/sesn_2/events"),
        (&post, "/v1/sessions/sesn_1"),
        (&del, "/v1/sessions/sesn_1"),
        (&post, "/v1/sessions/sesn_2/harness/events"),
        (&get, "/v1/files"),
        (&del, "/v1/files/file_1/content"),
        (&get, "/v1/skills"),
        (&post, "/v1/skills/skill_1/versions"),
        (&get, "/v1/memory_stores/memstore_1"),
        (&post, "/v1/memory_stores"),
        (&get, "/v1/agents"),
        (&get, "/v1/sessions"),
    ] {
        assert!(!work_route(method, path, &claims()), "{method} {path}");
    }
}

#[test]
fn an_environment_key_reaches_its_queue_s_work_and_session_events() {
    let (get, post, del) = (Method::GET, Method::POST, Method::DELETE);
    for (method, path) in [
        (&get, "/v1/environments/env_1/work"),
        (&get, "/v1/environments/env_1/work/poll"),
        (&get, "/v1/environments/env_1/work/stats"),
        (&post, "/v1/environments/env_1/work/sesn_1/ack"),
        (&post, "/v1/environments/env_1/work/sesn_1/heartbeat"),
        (&post, "/v1/environments/env_1/work/sesn_1/stop"),
        (&get, "/v1/sessions/sesn_1"),
        (&get, "/v1/sessions/sesn_1/events"),
        (&post, "/v1/sessions/sesn_1/events"),
        (&get, "/v1/sessions/sesn_1/events/stream"),
    ] {
        assert!(environment_route(method, path, "env_1"), "{method} {path}");
    }
    for (method, path) in [
        (&get, "/v1/environments/env_1"),
        (&get, "/v1/environments/env_1/work/sesn_1"),
        (&post, "/v1/environments/env_1/work/sesn_1"),
        (&get, "/v1/environments/env_2/work/poll"),
        (&post, "/v1/environments/env_2/work/sesn_1/heartbeat"),
        (&post, "/v1/environments/env_1/work/poll"),
        (&del, "/v1/sessions/sesn_1"),
        (&post, "/v1/sessions/sesn_1"),
        (&get, "/v1/sessions"),
        (&get, "/v1/sessions/sesn_1/threads"),
        (&get, "/v1/agents"),
        (&get, "/v1/files/file_1/content"),
        (&get, "/v1/memory_stores/memstore_1/memories"),
    ] {
        assert!(!environment_route(method, path, "env_1"), "{method} {path}");
    }
}

#[test]
fn a_path_names_its_session_and_the_environment_it_reads() {
    assert_eq!(named_session("/v1/environments/runtime/work/sesn_1/ack"), Some("sesn_1"));
    assert_eq!(named_session("/v1/sessions/sesn_1/events"), Some("sesn_1"));
    assert_eq!(named_session("/v1/environments/runtime/work/poll"), None);
    assert_eq!(named_session("/v1/environments/runtime/work"), None);
    assert_eq!(environment_read(&Method::GET, "/v1/environments/env_1"), Some("env_1"));
    assert_eq!(environment_read(&Method::POST, "/v1/environments/env_1"), None);
    assert!(is_poll("/v1/environments/env_1/work/poll") && !is_poll("/v1/environments/env_1/work"));
}

struct Fixed(Grants);

#[async_trait]
impl SessionGrants for Fixed {
    async fn grants(&self, _: &AuthContext, _: &str) -> Result<Grants, ApiError> {
        Ok(self.0.clone())
    }
}

#[tokio::test]
async fn a_token_reaches_only_its_session_s_skills_and_stores_and_writes_only_read_write_ones() {
    let ctx = AuthContext {
        org: shared::OrgId::new("o"),
        workspace: shared::WorkspaceId::new("w"),
        api_key_id: String::new(),
    };
    let grants = Fixed(Grants {
        skills: vec!["skill_a".into()],
        memory_stores: vec![("memstore_rw".into(), true), ("memstore_ro".into(), false)],
    });
    let check = |m: Method, p: &'static str| {
        let (ctx, grants) = (&ctx, &grants);
        async move { granted(&m, p, ctx, "sesn_1", grants).await.map_err(|e| e.status) }
    };
    assert_eq!(check(Method::GET, "/v1/skills/skill_a/versions/1").await, Ok(()));
    assert_eq!(check(Method::GET, "/v1/skills/skill_b/versions/1").await, Err(404));
    assert_eq!(check(Method::GET, "/v1/skills/skill_a").await, Ok(()));
    assert_eq!(check(Method::GET, "/v1/skills/skill_b").await, Err(404));
    assert_eq!(check(Method::POST, "/v1/memory_stores/memstore_rw/memories").await, Ok(()));
    assert_eq!(check(Method::GET, "/v1/memory_stores/memstore_ro/memories").await, Ok(()));
    assert_eq!(check(Method::POST, "/v1/memory_stores/memstore_ro/memories").await, Err(403));
    assert_eq!(check(Method::DELETE, "/v1/memory_stores/memstore_ro/memories/mem_1").await, Err(403));
    assert_eq!(check(Method::GET, "/v1/memory_stores/memstore_x/memories").await, Err(404));
    assert_eq!(check(Method::GET, "/v1/files/file_1").await, Ok(()));
}
