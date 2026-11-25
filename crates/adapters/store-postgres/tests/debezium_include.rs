//! Debezium's include list names every tracked table: each table with a `_history_stamp` trigger.

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic, clippy::indexing_slicing)]

/// The tables the migrations give a `_history_stamp` trigger.
fn tracked_tables() -> Vec<String> {
    let dir = concat!(env!("CARGO_MANIFEST_DIR"), "/migrations");
    let mut tables = vec![];
    for entry in std::fs::read_dir(dir).unwrap() {
        let sql = std::fs::read_to_string(entry.unwrap().path()).unwrap();
        for line in sql.lines() {
            let Some(rest) = line.strip_prefix("create trigger ") else { continue };
            if let Some((_, table)) = rest.split_once("_history_stamp before insert or update on ") {
                tables.push(table.trim().to_owned());
            }
        }
    }
    tables.sort();
    tables
}

#[test]
fn debezium_reads_every_tracked_table() {
    let path = concat!(env!("CARGO_MANIFEST_DIR"), "/../../../docker/debezium/application.properties");
    let properties = std::fs::read_to_string(path).unwrap();
    let list =
        properties.lines().find_map(|l| l.strip_prefix("debezium.source.table.include.list=")).unwrap();
    let included: Vec<&str> = list.split(',').filter_map(|t| t.strip_prefix("public.")).collect();
    let tables = tracked_tables();
    assert_eq!(tables.len(), 18);
    let missing: Vec<&String> = tables.iter().filter(|t| !included.contains(&t.as_str())).collect();
    assert_eq!(missing, Vec::<&String>::new());
}
