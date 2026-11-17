#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic, clippy::indexing_slicing)]

use std::sync::Arc;

use parking_lot::Mutex;
use serde_json::{json, Value};
use shared::ports::changes::{Change, ChangeHandler, ChangeOp, Position, Source};
use shared::ports::history::{HistoryError, HistoryLogImpl, HistoryOp};
use shared::{BackendError, Timestamp};

use super::*;

/// Keeps what it is told to record; fails while `failing` is set.
#[derive(Default)]
struct Kept {
    recorded: Mutex<Vec<(String, HistoryOp, u64, Value)>>,
    scrubbed: Mutex<Vec<(String, String, String)>>,
    failing: Mutex<bool>,
}

impl HistoryLogImpl for Kept {
    fn tables(&self) -> Vec<String> {
        vec!["memories".into(), "memory_versions".into(), "vaults".into()]
    }

    async fn record(
        &self,
        table: &str,
        op: HistoryOp,
        at: &Position,
        row: &Value,
    ) -> Result<(), HistoryError> {
        if *self.failing.lock() {
            return Err(HistoryError::Backend(BackendError::new(std::io::Error::other("down"))));
        }
        if !HistoryLogImpl::tables(self).iter().any(|t| t == table) {
            return Err(HistoryError::Unrecordable(format!("{table} has no history table")));
        }
        self.recorded.lock().push((table.to_owned(), op, at.lsn, row.clone()));
        Ok(())
    }

    async fn scrub_memory_content(
        &self,
        workspace: &str,
        store: &str,
        version: &str,
    ) -> Result<(), HistoryError> {
        self.scrubbed.lock().push((workspace.to_owned(), store.to_owned(), version.to_owned()));
        Ok(())
    }
}

fn position(lsn: u64) -> Option<Position> {
    Some(Position { lsn, tx_id: Some(1), committed_at: Timestamp::now() })
}

fn change(table: &str, op: ChangeOp, lsn: u64, after: Option<Value>) -> Change {
    Change { table: table.into(), op, position: position(lsn), before: None, after }
}

/// A history log, its recorders, and how many recordings they reported.
struct Fixture {
    kept: Arc<Kept>,
    handlers: Vec<Arc<dyn ChangeHandler>>,
    applied: Arc<Mutex<usize>>,
}

fn fixture() -> Fixture {
    let kept = Arc::new(Kept::default());
    let applied = Arc::new(Mutex::new(0));
    let count = applied.clone();
    let handlers = recorders(kept.clone(), Arc::new(move |_| *count.lock() += 1));
    Fixture { kept, handlers, applied }
}

fn handler<'a>(handlers: &'a [Arc<dyn ChangeHandler>], source: &Source) -> &'a dyn ChangeHandler {
    handlers.iter().find(|h| &h.source() == source).unwrap().as_ref()
}

#[tokio::test]
async fn each_history_table_gets_a_recorder_and_the_messages_one() {
    let Fixture { handlers, .. } = fixture();
    let sources: Vec<Source> = handlers.iter().map(|h| h.source()).collect();
    let tables = ["memories", "memory_versions", "vaults"].map(|t| Source::Table(t.into()));
    let tables = [tables.as_slice(), &[Source::Messages]].concat();
    assert_eq!(sources, tables);
}

#[tokio::test]
async fn an_insert_and_an_update_are_recorded_and_a_delete_event_is_not() {
    let Fixture { kept, handlers, applied } = fixture();
    let vaults = handler(&handlers, &Source::Table("vaults".into()));
    let row = json!({"vault_id": "vlt_1", "actor": "apikey_a"});
    vaults.handle(&change("vaults", ChangeOp::Insert, 10, Some(row.clone()))).await.unwrap();
    vaults.handle(&change("vaults", ChangeOp::Update, 20, Some(row.clone()))).await.unwrap();
    vaults.handle(&change("vaults", ChangeOp::Delete, 30, None)).await.unwrap();
    let ops: Vec<(HistoryOp, u64)> = kept.recorded.lock().iter().map(|r| (r.1, r.2)).collect();
    assert_eq!(ops, [(HistoryOp::Insert, 10), (HistoryOp::Update, 20)]);
    assert!(kept.scrubbed.lock().is_empty(), "only memory rows are scrubbed");
    assert_eq!(*applied.lock(), 2);
}

#[tokio::test]
async fn a_delete_message_is_recorded_with_the_deleter_as_its_actor() {
    let Fixture { kept, handlers, .. } = fixture();
    let content = json!({"table": "vaults", "actor": "adminkey_d", "request_id": "req_d",
                         "row": {"vault_id": "vlt_1", "actor": "apikey_a", "request_id": "req_a"}});
    let messages = handler(&handlers, &Source::Messages);
    messages.handle(&change("agentd-history", ChangeOp::Message, 40, Some(content))).await.unwrap();
    messages.handle(&change("other", ChangeOp::Message, 41, Some(json!({"table": "vaults"})))).await.unwrap();
    let recorded = kept.recorded.lock().clone();
    assert_eq!(recorded.len(), 1);
    let (table, op, lsn, row) = &recorded[0];
    assert_eq!((table.as_str(), *op, *lsn), ("vaults", HistoryOp::Delete, 40));
    assert_eq!(row, &json!({"vault_id": "vlt_1", "actor": "adminkey_d", "request_id": "req_d"}));
}

#[tokio::test]
async fn a_store_failure_is_handled_again_and_an_unrecordable_change_is_passed_over() {
    let Fixture { kept, handlers, .. } = fixture();
    let vaults = handler(&handlers, &Source::Table("vaults".into()));
    *kept.failing.lock() = true;
    let row = Some(json!({"vault_id": "vlt_1"}));
    assert!(vaults.handle(&change("vaults", ChangeOp::Insert, 10, row.clone())).await.is_err());
    *kept.failing.lock() = false;
    let mut unplaced = change("vaults", ChangeOp::Insert, 10, row);
    unplaced.position = None;
    vaults.handle(&unplaced).await.unwrap();
    let messages = handler(&handlers, &Source::Messages);
    let unknown = json!({"table": "work_items", "row": {}});
    messages.handle(&change("agentd-history", ChangeOp::Message, 50, Some(unknown))).await.unwrap();
    messages.handle(&change("agentd-history", ChangeOp::Message, 51, Some(json!("not json")))).await.unwrap();
    assert!(kept.recorded.lock().is_empty());
}

#[tokio::test]
async fn a_memory_version_s_update_is_scrubbed_once_it_is_recorded() {
    let Fixture { kept, handlers, .. } = fixture();
    let versions = handler(&handlers, &Source::Table("memory_versions".into()));
    let row = json!({"workspace_id": "wrkspc_1", "store_id": "memstore_1", "version_id": "memver_1"});
    versions.handle(&change("memory_versions", ChangeOp::Update, 60, Some(row.clone()))).await.unwrap();
    assert_eq!(*kept.recorded.lock(), [("memory_versions".to_owned(), HistoryOp::Update, 60, row)]);
    assert_eq!(
        *kept.scrubbed.lock(),
        [("wrkspc_1".to_owned(), "memstore_1".to_owned(), "memver_1".to_owned())]
    );
}

#[tokio::test]
async fn a_memory_head_s_delete_is_scrubbed_once_it_is_recorded() {
    let Fixture { kept, handlers, .. } = fixture();
    let head = json!({"workspace_id": "wrkspc_1", "store_id": "memstore_1", "head_version_id": "memver_1"});
    let content = json!({"table": "memories", "actor": "apikey_a", "request_id": "req_a", "row": head});
    let messages = handler(&handlers, &Source::Messages);
    messages.handle(&change("agentd-history", ChangeOp::Message, 70, Some(content))).await.unwrap();
    let mut recorded = head;
    recorded["actor"] = json!("apikey_a");
    recorded["request_id"] = json!("req_a");
    assert_eq!(*kept.recorded.lock(), [("memories".to_owned(), HistoryOp::Delete, 70, recorded)]);
    assert_eq!(
        *kept.scrubbed.lock(),
        [("wrkspc_1".to_owned(), "memstore_1".to_owned(), "memver_1".to_owned())]
    );
}

/// The tables V17 makes in the schema `history`.
fn history_tables() -> Vec<String> {
    let dir = concat!(env!("CARGO_MANIFEST_DIR"), "/../../adapters/store-postgres/migrations");
    let mut tables = vec![];
    for entry in std::fs::read_dir(dir).unwrap() {
        let sql = std::fs::read_to_string(entry.unwrap().path()).unwrap();
        for line in sql.lines() {
            if let Some(rest) = line.strip_prefix("create table history.") {
                tables.push(rest.trim_end_matches(" (").to_owned());
            }
        }
    }
    tables.sort();
    tables
}

#[test]
fn debezium_reads_every_history_table() {
    let path = concat!(env!("CARGO_MANIFEST_DIR"), "/../../../docker/debezium/application.properties");
    let properties = std::fs::read_to_string(path).unwrap();
    let list =
        properties.lines().find_map(|l| l.strip_prefix("debezium.source.table.include.list=")).unwrap();
    let included: Vec<&str> = list.split(',').filter_map(|t| t.strip_prefix("public.")).collect();
    let tables = history_tables();
    assert_eq!(tables.len(), 18);
    let missing: Vec<&String> = tables.iter().filter(|t| !included.contains(&t.as_str())).collect();
    assert_eq!(missing, Vec::<&String>::new());
}
