use super::*;
use agentd_core::Actor;

const MEM_FIXTURE: &str = include_str!("../../../../fixtures/memory.create.json");
const STORE_FIXTURE: &str = include_str!("../../../../fixtures/memory_store.create.json");
const VERSIONS_FIXTURE: &str = include_str!("../../../../fixtures/memory_versions.list.json");

#[test]
fn memory_fixture_roundtrip() {
    let original: serde_json::Value = serde_json::from_str(MEM_FIXTURE).unwrap();
    let m: Memory = serde_json::from_str(MEM_FIXTURE).unwrap();
    assert_eq!(serde_json::to_value(&m).unwrap(), original);
}

#[test]
fn store_fixture_roundtrip() {
    let original: serde_json::Value = serde_json::from_str(STORE_FIXTURE).unwrap();
    let s: MemoryStoreEntity = serde_json::from_str(STORE_FIXTURE).unwrap();
    assert_eq!(serde_json::to_value(&s).unwrap(), original);
}

#[test]
fn version_fixture_roundtrip() {
    let wrapper: serde_json::Value = serde_json::from_str(VERSIONS_FIXTURE).unwrap();
    let first = wrapper["data"][0].clone();
    let v: MemoryVersion = serde_json::from_value(first.clone()).unwrap();
    assert_eq!(serde_json::to_value(&v).unwrap(), first);
    assert_eq!(v.operation, VersionOperation::Created);
    assert!(matches!(v.created_by, Actor::ApiActor { .. }));
}

#[test]
fn precondition_parses() {
    let p: Precondition =
        serde_json::from_str(r#"{"type":"content_sha256","content_sha256":"deadbeef"}"#).unwrap();
    assert!(matches!(p, Precondition::ContentSha256 { .. }));
}
