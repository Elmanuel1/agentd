//! Each record is handed to its reader while the consumer keeps polling, so a
//! long attempt or a retry keeps the consumer in its group and no second
//! replica reads its partition (DS-13). Every assigned partition is paused
//! meanwhile, so no record behind it is read, and a revoked partition ends the
//! attempt with nothing committed.

use rdkafka::Message;
use rdkafka::consumer::{Consumer, StreamConsumer};
use rdkafka::error::KafkaResult;
use rdkafka::message::BorrowedMessage;
use rdkafka::{Offset, TopicPartitionList};

use crate::{BACKOFF, Reader};

/// Hand `record` to `reader` until it takes it: `true` once it does, `false`
/// when its partition was revoked, so its next owner reads it again.
pub(crate) async fn until_taken(
    consumer: &StreamConsumer,
    record: &BorrowedMessage<'_>,
    reader: &Reader,
    id: &str,
) -> KafkaResult<bool> {
    consumer.pause(&consumer.assignment()?)?;
    let taken = keep_trying(consumer, record, reader, id).await;
    consumer.resume(&consumer.assignment()?)?;
    taken
}

async fn keep_trying(
    consumer: &StreamConsumer,
    record: &BorrowedMessage<'_>,
    reader: &Reader,
    id: &str,
) -> KafkaResult<bool> {
    let value = record.payload_view::<str>().and_then(Result::ok);
    loop {
        // Polling serves the group's rebalances and keeps max.poll.interval.ms from expiring; a paused
        // partition delivers nothing, so a record here is from one assigned again meanwhile. A revoked
        // partition drops the attempt, so its result never reaches the commit.
        let attempt = reader.consume(value, id);
        tokio::pin!(attempt);
        let mut check = tokio::time::interval(BACKOFF);
        let taken = loop {
            tokio::select! {
                taken = &mut attempt => break taken,
                polled = consumer.recv() => hold_back(consumer, record, &polled?)?,
                _ = check.tick() => if revoked(consumer, record, id)? {
                    return Ok(false);
                },
            }
        };
        // A partition revoked while the attempt ran is the next owner's to commit; the handlers are idempotent,
        // as the feed delivers at least once.
        if taken {
            return Ok(!revoked(consumer, record, id)?);
        }
        if let Ok(polled) = tokio::time::timeout(BACKOFF, consumer.recv()).await {
            hold_back(consumer, record, &polled?)?;
        }
    }
}

/// Whether `record`'s partition is no longer assigned to this consumer.
fn revoked(consumer: &StreamConsumer, record: &BorrowedMessage<'_>, id: &str) -> KafkaResult<bool> {
    let gone = consumer.assignment()?.find_partition(record.topic(), record.partition()).is_none();
    if gone {
        tracing::warn!(%id, "changes: the record's partition was revoked; its next owner reads it again");
    }
    Ok(gone)
}

/// Pause `other`'s partition and move it back so it is read after the retry:
/// to `other` itself, or past `retrying` when `other` is from its partition
/// (`retrying` again, or a record behind it), which the retry commits.
fn hold_back(
    consumer: &StreamConsumer,
    retrying: &BorrowedMessage<'_>,
    other: &BorrowedMessage<'_>,
) -> KafkaResult<()> {
    let mut partition = TopicPartitionList::new();
    partition.add_partition(other.topic(), other.partition());
    consumer.pause(&partition)?;
    let same = (other.topic(), other.partition()) == (retrying.topic(), retrying.partition());
    let from = match same {
        true => retrying.offset() + 1,
        false => other.offset(),
    };
    consumer.seek(other.topic(), other.partition(), Offset::Offset(from), BACKOFF)
}
