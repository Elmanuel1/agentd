//! A thread an interrupt stopped.

use super::*;

#[test]
fn an_interrupt_halts_the_thread_until_the_client_sends_more() {
    let s1 = start();
    let mut interrupt = ev(UserEvent::Interrupt {});
    interrupt.processed_at = Some(shared::Timestamp::now());
    let h = build(&[user("go"), s1.clone(), interrupt.clone()]);
    assert!(h.halted && !h.awaits_model(), "an interrupted turn does not call the model again");
    let h = build(&[user("go"), s1, interrupt, user("again")]);
    assert!(!h.halted && h.awaits_model());
}
