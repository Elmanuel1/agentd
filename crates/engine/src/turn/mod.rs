//! One model call of a thread's turn: its prompt rebuilt from the log, its
//! tools and their permission policies, and what its reply leads to.

pub mod history;
pub mod policy;
pub mod prompt;
pub mod specs;
