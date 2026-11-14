//! Against the Valkey at `AGENTD_VALKEY_URL`; skipped without it.

#![allow(clippy::unwrap_used, clippy::indexing_slicing)]

use redis::streams::{StreamId, StreamRangeReply, StreamReadOptions, StreamReadReply};
use redis::AsyncCommands;

use super::{parse, trim, Keep};

fn client() -> Option<redis::Client> {
    let url = std::env::var("AGENTD_VALKEY_URL").ok()?;
    Some(redis::Client::open(url).unwrap())
}

async fn ids(conn: &mut redis::aio::MultiplexedConnection, stream: &str) -> Vec<String> {
    let all: StreamRangeReply = conn.xrange_all(stream).await.unwrap();
    all.ids.into_iter().map(|e| e.id).collect()
}

#[test]
fn an_id_parses_and_a_malformed_one_keeps_everything() {
    assert_eq!(parse("1794614704349-7"), (1_794_614_704_349, 7));
    assert_eq!(parse("not-an-id"), (0, 0));
}

#[tokio::test]
async fn a_trim_keeps_what_the_group_has_not_acknowledged_and_what_it_was_not_delivered() {
    let Some(client) = client() else { return };
    let mut conn = client.get_multiplexed_async_connection().await.unwrap();
    let stream = format!("agentd-trim-test-{}", uuid::Uuid::new_v4());
    for i in 0..10 {
        let _: String = conn.xadd(&stream, "*", &[("n", i)]).await.unwrap();
    }
    let all = ids(&mut conn, &stream).await;
    let _: () = conn.xgroup_create(&stream, "agentd", "0").await.unwrap();
    let read: StreamReadReply = conn
        .xread_options(&[&stream], &[">"], &StreamReadOptions::default().group("agentd", "c").count(6))
        .await
        .unwrap();
    let delivered: Vec<StreamId> = read.keys.into_iter().flat_map(|k| k.ids).collect();
    let _: usize =
        conn.xack(&stream, "agentd", &[&delivered[0].id, &delivered[1].id, &delivered[3].id]).await.unwrap();

    assert_eq!(trim(&client, &stream, Keep::Needed).await.unwrap(), 2, "below the oldest pending entry");
    assert_eq!(ids(&mut conn, &stream).await, all[2..]);

    let rest: Vec<&str> = delivered[2..].iter().map(|e| e.id.as_str()).collect();
    let _: usize = conn.xack(&stream, "agentd", &rest).await.unwrap();
    assert_eq!(trim(&client, &stream, Keep::Needed).await.unwrap(), 3, "below the last delivered entry");
    assert_eq!(ids(&mut conn, &stream).await, all[5..]);
    let _: () = conn.del(&stream).await.unwrap();
}

#[tokio::test]
async fn a_stream_no_group_reads_is_left_whole_unless_it_keeps_its_last_entries() {
    let Some(client) = client() else { return };
    let mut conn = client.get_multiplexed_async_connection().await.unwrap();
    let stream = format!("agentd-trim-test-{}", uuid::Uuid::new_v4());
    for i in 0..5 {
        let _: String = conn.xadd(&stream, "*", &[("n", i)]).await.unwrap();
    }
    assert_eq!(trim(&client, &stream, Keep::Needed).await.unwrap(), 0);
    assert_eq!(trim(&client, &stream, Keep::Last(2)).await.unwrap(), 3);
    assert_eq!(trim(&client, "agentd-trim-test-missing", Keep::Needed).await.unwrap(), 0);
    let _: () = conn.del(&stream).await.unwrap();
}
