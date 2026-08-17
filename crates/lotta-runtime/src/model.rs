//! Dependency-neutral four-level model precedence selection.

/// A model-selection level ordered from highest to lowest precedence.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum PrecedenceLevel {
    /// Request-scoped override.
    Request,
    /// Conversation-scoped override.
    Conversation,
    /// Agent-scoped selection.
    Agent,
    /// Local configured default.
    LocalDefault,
}

impl PrecedenceLevel {
    /// Converts an index in the canonical precedence array to its level.
    #[must_use]
    pub const fn from_index(index: usize) -> Self {
        match index {
            0 => Self::Request,
            1 => Self::Conversation,
            2 => Self::Agent,
            _ => Self::LocalDefault,
        }
    }

    /// Returns this level's index in the canonical precedence array.
    #[must_use]
    pub const fn index(self) -> usize {
        match self {
            Self::Request => 0,
            Self::Conversation => 1,
            Self::Agent => 2,
            Self::LocalDefault => 3,
        }
    }
}

/// A selected value and the level that supplied it.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct PrecedenceSelection<T> {
    /// Winning level.
    pub level: PrecedenceLevel,
    /// Winning value.
    pub value: T,
}

/// Selects the first present value in request, conversation, agent, default order.
#[must_use]
pub fn select_precedence<T>(levels: [Option<T>; 4]) -> Option<PrecedenceSelection<T>> {
    levels.into_iter().enumerate().find_map(|(index, value)| {
        value.map(|value| PrecedenceSelection {
            level: PrecedenceLevel::from_index(index),
            value,
        })
    })
}

#[cfg(test)]
mod resolution_order {
    use super::*;

    fn selected(levels: [Option<&'static str>; 4]) -> PrecedenceSelection<&'static str> {
        select_precedence(levels).unwrap_or_else(|| panic!("a selection was expected"))
    }

    #[test]
    fn request_wins_all_lower_levels() {
        let value = selected([
            Some("request"),
            Some("conversation"),
            Some("agent"),
            Some("default"),
        ]);
        assert_eq!(value.level, PrecedenceLevel::Request);
        assert_eq!(value.value, "request");
    }

    #[test]
    fn conversation_wins_agent_and_default() {
        let value = selected([None, Some("conversation"), Some("agent"), Some("default")]);
        assert_eq!(value.level, PrecedenceLevel::Conversation);
        assert_eq!(value.value, "conversation");
    }

    #[test]
    fn agent_wins_default() {
        let value = selected([None, None, Some("agent"), Some("default")]);
        assert_eq!(value.level, PrecedenceLevel::Agent);
        assert_eq!(value.value, "agent");
    }

    #[test]
    fn local_default_wins_when_alone() {
        let value = selected([None, None, None, Some("default")]);
        assert_eq!(value.level, PrecedenceLevel::LocalDefault);
        assert_eq!(value.value, "default");
    }

    #[test]
    fn no_level_returns_none() {
        assert_eq!(select_precedence::<u8>([None; 4]), None);
    }
}
