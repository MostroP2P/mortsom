//! The two actors every run drives: Alice and Bob.

use serde::{Deserialize, Serialize};

/// Actor slots of a run: one isolated client each.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Actor {
    /// Alice.
    Alice,
    /// Bob.
    Bob,
}

impl Actor {
    /// Both actors, in order.
    pub const ALL: [Actor; 2] = [Actor::Alice, Actor::Bob];

    /// Stable lowercase name.
    pub fn as_str(self) -> &'static str {
        match self {
            Actor::Alice => "alice",
            Actor::Bob => "bob",
        }
    }

    /// Zero-based slot index.
    pub fn index(self) -> u16 {
        match self {
            Actor::Alice => 0,
            Actor::Bob => 1,
        }
    }
}

impl std::fmt::Display for Actor {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn actors_have_stable_names_slots_and_serde_form() {
        assert_eq!(Actor::ALL, [Actor::Alice, Actor::Bob]);
        assert_eq!(Actor::Alice.as_str(), "alice");
        assert_eq!(Actor::Bob.to_string(), "bob");
        assert_eq!(Actor::Alice.index(), 0);
        assert_eq!(Actor::Bob.index(), 1);
        assert_eq!(
            serde_json::to_string(&Actor::Bob).ok().as_deref(),
            Some("\"bob\"")
        );
    }
}
