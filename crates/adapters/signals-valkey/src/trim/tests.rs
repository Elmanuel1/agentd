//! Against the test Valkeys (standalone and cluster); skipped without them.

#![allow(clippy::unwrap_used, clippy::indexing_slicing)]

use redis::AsyncCommands;
use redis::streams::{StreamId, StreamRangeReply, StreamReadOptions, StreamReadReply};

use super::{Keep, parse, trim};
use crate::test_valkeys;
use crate::valkey::{Conn, Valkey};

async fn ids(conn: &mut Conn, stream: &str) -> Vec<String> {
    let all: StreamRangeReply = conn.xrange_all(stream).await.unwrap();
    all.ids.into_iter().map(|e| e.id).collect()
}

async fn stream_of(conn: &mut Conn, entries: usize) -> String {
    let stream = format!("agentd-trim-test-{}", uuid::Uuid::new_v4());
    for i in 0..entries {
        let _: String = conn.xadd(&stream, "*", &[("n", i)]).await.unwrap();
    }
    stream
}

#[test]
fn an_id_parses_and_a_malformed_one_keeps_everything() {
    assert_eq!(parse("1794614704349-7"), (1_794_614_704_349, 7));
    assert_eq!(parse("not-an-id"), (0, 0));
}

#[tokio::test]
async fn a_trim_keeps_what_the_group_has_not_acknowledged_and_what_it_was_not_delivered() {
    for valkey in test_valkeys() {
        keeps_what_is_needed(&valkey).await;
    }
}

async fn keeps_what_is_needed(valkey: &Valkey) {
    let mut conn = valkey.connection().await.unwrap();
    let stream = stream_of(&mut conn, 10).await;
    let all = ids(&mut conn, &stream).await;
    let _: () = conn.xgroup_create(&stream, "agentd", "0").await.unwrap();
    let read: StreamReadReply = conn
        .xread_options(&[&stream], &[">"], &StreamReadOptions::default().group("agentd", "c").count(6))
        .await
        .unwrap();
    let delivered: Vec<StreamId> = read.keys.into_iter().flat_map(|k| k.ids).collect();
    let _: usize =
        conn.xack(&stream, "agentd", &[&delivered[0].id, &delivered[1].id, &delivered[3].id]).await.unwrap();

    assert_eq!(trim(valkey, &stream, Keep::Needed).await.unwrap(), 2, "below the oldest pending entry");
    assert_eq!(ids(&mut conn, &stream).await, all[2..]);

    let rest: Vec<&str> = delivered[2..].iter().map(|e| e.id.as_str()).collect();
    let _: usize = conn.xack(&stream, "agentd", &rest).await.unwrap();
    assert_eq!(trim(valkey, &stream, Keep::Needed).await.unwrap(), 3, "below the last delivered entry");
    assert_eq!(ids(&mut conn, &stream).await, all[5..]);
    let _: () = conn.del(&stream).await.unwrap();
}

#[tokio::test]
async fn a_stream_no_group_reads_is_left_whole_unless_it_keeps_its_last_entries() {
    for valkey in test_valkeys() {
        let mut conn = valkey.connection().await.unwrap();
        let stream = stream_of(&mut conn, 5).await;
        assert_eq!(trim(&valkey, &stream, Keep::Needed).await.unwrap(), 0);
        assert_eq!(trim(&valkey, &stream, Keep::Last(2)).await.unwrap(), 3);
        assert_eq!(trim(&valkey, "agentd-trim-test-missing", Keep::Needed).await.unwrap(), 0);
        let _: () = conn.del(&stream).await.unwrap();
    }
}

#[tokio::test]
async fn a_stream_no_group_reads_keeps_only_its_newer_entries() {
    for valkey in test_valkeys() {
        let mut conn = valkey.connection().await.unwrap();
        let stream = stream_of(&mut conn, 5).await;
        let week = Keep::NeededOrNewer(std::time::Duration::from_secs(7 * 24 * 3600));
        assert_eq!(trim(&valkey, &stream, week).await.unwrap(), 0, "every entry is newer than a week");
        tokio::time::sleep(std::time::Duration::from_millis(5)).await;
        let none = Keep::NeededOrNewer(std::time::Duration::ZERO);
        assert_eq!(trim(&valkey, &stream, none).await.unwrap(), 5);
        assert!(ids(&mut conn, &stream).await.is_empty());
    }
}

#[tokio::test]
async fn a_stream_a_group_reads_keeps_what_the_group_needs_however_old() {
    for valkey in test_valkeys() {
        let mut conn = valkey.connection().await.unwrap();
        let stream = stream_of(&mut conn, 3).await;
        let _: () = conn.xgroup_create(&stream, "readers", "0").await.unwrap();
        let none = Keep::NeededOrNewer(std::time::Duration::ZERO);
        assert_eq!(trim(&valkey, &stream, none).await.unwrap(), 0, "the group has read nothing yet");
        assert_eq!(ids(&mut conn, &stream).await.len(), 3);
    }
}
