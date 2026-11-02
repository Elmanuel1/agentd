//! Audit actors — live shape on memory versions:
//! `{"api_key_id":"apikey_01…","type":"api_actor"}`

use serde::{Deserialize, Serialize};

/// Who performed a mutation.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum Actor {
    /// Direct API call.
    ApiActor {
        /// The API key that made the call.
        api_key_id: String,
    },
    /// A running session (agent-side write, e.g. through a memory mount).
    SessionActor {
        /// The originating session.
        session_id: String,
    },
    /// A human console user.
    UserActor {
        /// The user id.
        user_id: String,
    },
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn api_actor_matches_live_fixture() {
        // Live: "created_by": {"api_key_id":"apikey_01Wnw5ximjHVpuyEXdDxFe5x","type":"api_actor"}
        let a = Actor::ApiActor { api_key_id: "apikey_01Wnw5ximjHVpuyEXdDxFe5x".into() };
        let v = serde_json::to_value(&a).unwrap();
        assert_eq!(v["type"], "api_actor");
        assert_eq!(v["api_key_id"], "apikey_01Wnw5ximjHVpuyEXdDxFe5x");
    }

    #[test]
    fn roundtrip_all_variants() {
        for a in [
            Actor::ApiActor { api_key_id: "apikey_x".into() },
            Actor::SessionActor { session_id: "sesn_x".into() },
            Actor::UserActor { user_id: "user_x".into() },
        ] {
            let s = serde_json::to_string(&a).unwrap();
            let b: Actor = serde_json::from_str(&s).unwrap();
            assert_eq!(a, b);
        }
    }
}
