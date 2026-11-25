use std::time::Duration;

use super::hourly;

#[tokio::test(start_paused = true)]
async fn the_first_run_is_an_hour_after_the_start() {
    let mut every = hourly();
    let early = tokio::time::timeout(Duration::from_secs(3599), every.tick()).await;
    assert!(early.is_err(), "no run in the first hour");
    let first = tokio::time::timeout(Duration::from_secs(2), every.tick()).await;
    assert!(first.is_ok(), "the first run comes at the hour");
}
