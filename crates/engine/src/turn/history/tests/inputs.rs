//! Inputs the Messages API refuses as they are.

use super::*;

#[test]
fn an_input_with_no_text_reaches_the_model_as_no_text() {
    // The API refuses an empty text block; an image-only message still asks.
    let h = build(&[user("")]);
    assert_eq!(h.messages, vec![Message { role: Role::User, content: vec![Block::text("(no text)")] }]);
    assert!(h.awaits_model());
}
