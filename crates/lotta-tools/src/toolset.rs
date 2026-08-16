//! Toolset identifiers and model-based preference resolution.

use serde::{Deserialize, Deserializer, Serialize, Serializer, de};
use std::{fmt, str::FromStr};

/// One concrete model-facing toolset.
#[derive(Clone, Copy, Debug, Deserialize, Eq, Hash, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ToolsetId {
    /// Anthropic-oriented default names.
    Default,
    /// Codex compatibility names.
    Codex,
    /// Snake-case Codex names.
    CodexSnake,
    /// `PascalCase` Gemini names.
    Gemini,
    /// Original Gemini names.
    GeminiSnake,
    /// No built-in tools.
    None,
}

impl ToolsetId {
    /// Every concrete identifier in stable order.
    pub const ALL: [Self; 6] = [
        Self::Default,
        Self::Codex,
        Self::CodexSnake,
        Self::Gemini,
        Self::GeminiSnake,
        Self::None,
    ];

    /// Returns the stable wire name.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Default => "default",
            Self::Codex => "codex",
            Self::CodexSnake => "codex_snake",
            Self::Gemini => "gemini",
            Self::GeminiSnake => "gemini_snake",
            Self::None => "none",
        }
    }
}

impl fmt::Display for ToolsetId {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(self.as_str())
    }
}

/// Error returned for an unknown concrete toolset identifier.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct ParseToolsetIdError;

impl fmt::Display for ParseToolsetIdError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("unknown toolset identifier")
    }
}

impl std::error::Error for ParseToolsetIdError {}

impl FromStr for ToolsetId {
    type Err = ParseToolsetIdError;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        Self::ALL
            .into_iter()
            .find(|candidate| candidate.as_str() == value)
            .ok_or(ParseToolsetIdError)
    }
}

/// User preference before model/provider resolution.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ToolsetPreference {
    /// Resolve a concrete toolset from provider and model identifiers.
    Auto,
    /// Use a concrete toolset unchanged.
    Explicit(ToolsetId),
}

impl Serialize for ToolsetPreference {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        serializer.serialize_str(match self {
            Self::Auto => "auto",
            Self::Explicit(id) => id.as_str(),
        })
    }
}

impl<'de> Deserialize<'de> for ToolsetPreference {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        struct PreferenceVisitor;
        impl de::Visitor<'_> for PreferenceVisitor {
            type Value = ToolsetPreference;

            fn expecting(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
                formatter.write_str("one of seven toolset preference strings")
            }

            fn visit_borrowed_str<E>(self, value: &str) -> Result<Self::Value, E>
            where
                E: de::Error,
            {
                parse_preference(value).ok_or_else(|| E::custom("unknown toolset preference"))
            }

            fn visit_str<E>(self, value: &str) -> Result<Self::Value, E>
            where
                E: de::Error,
            {
                self.visit_borrowed_str(value)
            }
        }
        deserializer.deserialize_str(PreferenceVisitor)
    }
}

#[cfg(test)]
const PREFERENCE_NAMES: [&str; 7] = [
    "auto",
    "default",
    "codex",
    "codex_snake",
    "gemini",
    "gemini_snake",
    "none",
];

fn parse_preference(value: &str) -> Option<ToolsetPreference> {
    if value == "auto" {
        Some(ToolsetPreference::Auto)
    } else {
        value.parse().ok().map(ToolsetPreference::Explicit)
    }
}

impl ToolsetPreference {
    /// Resolves this preference to one concrete identifier.
    #[must_use]
    pub fn resolve(self, provider: Option<&str>, model: Option<&str>) -> ToolsetId {
        match self {
            Self::Explicit(id) => id,
            Self::Auto => resolve_auto(provider, model),
        }
    }
}

fn resolve_auto(provider: Option<&str>, model: Option<&str>) -> ToolsetId {
    if provider.is_some_and(|value| matches!(value, "chatgpt_oauth" | "openai-codex")) {
        return ToolsetId::Codex;
    }
    if model.is_some_and(is_openai_model) {
        ToolsetId::Codex
    } else {
        ToolsetId::Default
    }
}

fn is_openai_model(model: &str) -> bool {
    [
        "openai/",
        "openai-codex/",
        "chatgpt-plus-pro/",
        "chatgpt_oauth/",
    ]
    .iter()
    .any(|prefix| model.starts_with(prefix))
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use crate::names::rows;
    use serde::Deserialize;
    use std::collections::BTreeMap;

    #[derive(Deserialize)]
    struct Baseline {
        source_commit: String,
        source_path: String,
        source_sha256: String,
        inventory_region: String,
        inventory_sha256: String,
        alias_region: String,
        alias_sha256: String,
        rows: Vec<[String; 3]>,
    }

    #[test]
    fn ids_are_exactly_six() {
        let names: Vec<_> = ToolsetId::ALL.iter().map(ToString::to_string).collect();
        assert_eq!(
            names,
            [
                "default",
                "codex",
                "codex_snake",
                "gemini",
                "gemini_snake",
                "none"
            ]
        );
        assert!("auto".parse::<ToolsetId>().is_err());
    }

    #[test]
    fn auto_resolves() {
        let auto = ToolsetPreference::Auto;
        assert_eq!(
            auto.resolve(Some("chatgpt_oauth"), Some("anthropic/claude")),
            ToolsetId::Codex
        );
        assert_eq!(auto.resolve(Some("openai-codex"), None), ToolsetId::Codex);
        assert_eq!(auto.resolve(None, Some("openai/gpt-5.4")), ToolsetId::Codex);
        assert_eq!(
            auto.resolve(None, Some("openai-codex/gpt-5.5")),
            ToolsetId::Codex
        );
        assert_eq!(
            auto.resolve(None, Some("anthropic/claude-sonnet-4")),
            ToolsetId::Default
        );
        assert_eq!(
            ToolsetPreference::Explicit(ToolsetId::Gemini).resolve(Some("openai-codex"), None),
            ToolsetId::Gemini
        );
    }

    #[test]
    fn preference_wire_is_exactly_seven_strings() {
        let values = [
            ToolsetPreference::Auto,
            ToolsetPreference::Explicit(ToolsetId::Default),
            ToolsetPreference::Explicit(ToolsetId::Codex),
            ToolsetPreference::Explicit(ToolsetId::CodexSnake),
            ToolsetPreference::Explicit(ToolsetId::Gemini),
            ToolsetPreference::Explicit(ToolsetId::GeminiSnake),
            ToolsetPreference::Explicit(ToolsetId::None),
        ];
        for (value, name) in values.into_iter().zip(PREFERENCE_NAMES) {
            let encoded = serde_json::to_string(&value).unwrap();
            assert_eq!(encoded, format!("\"{name}\""));
            assert_eq!(
                serde_json::from_str::<ToolsetPreference>(&encoded).unwrap(),
                value
            );
        }
        for invalid in ["\"unknown\"", "null", "{}", "[]", "1", "true"] {
            assert!(serde_json::from_str::<ToolsetPreference>(invalid).is_err());
        }
    }

    #[test]
    fn unknown_preference_diagnostic_excludes_payload() {
        let sentinel = "ATTACKER_SENTINEL";
        let payload = format!("{sentinel}{}", "x".repeat(4_096));
        let encoded = serde_json::to_string(&payload).unwrap();
        let error = serde_json::from_str::<ToolsetPreference>(&encoded)
            .unwrap_err()
            .to_string();
        assert!(error.contains("unknown toolset preference"));
        assert!(!error.contains(sentinel));
        assert!(!error.contains(&payload));
    }

    #[test]
    fn model_facing_names_match_baseline() {
        let fixture: Baseline =
            serde_json::from_str(include_str!("../tests/fixtures/toolsets-baseline.json")).unwrap();
        assert_eq!(
            fixture.source_commit,
            "300f923f16cc8eee50656d7da732902c1dea2b65"
        );
        assert_eq!(fixture.source_path, "src/tools/manager.ts");
        assert_eq!(
            fixture.source_sha256,
            "56229100170cf38e881749bc4bb8bbf1ec52f10dedf2826cbcfed1dfa3455056"
        );
        assert_eq!(fixture.inventory_region, "370-458");
        assert_eq!(
            fixture.inventory_sha256,
            "ed70a35e4352d9279ae7cd4f6364be182109287fdab55dc7c5aad1b0d1803194"
        );
        assert_eq!(fixture.alias_region, "157-190");
        assert_eq!(
            fixture.alias_sha256,
            "28b29621514dfdba22dca1655fc480912a27a10984fb4d44339ffa3f3ff4529d"
        );
        let production: Vec<[String; 3]> = rows()
            .iter()
            .map(|row| {
                [
                    row.toolset.as_str().to_owned(),
                    row.internal.to_owned(),
                    row.model.to_owned(),
                ]
            })
            .collect();
        assert_eq!(production, fixture.rows);
        let counts: BTreeMap<_, _> = ToolsetId::ALL
            .into_iter()
            .map(|set| {
                (
                    set.as_str(),
                    rows().iter().filter(|row| row.toolset == set).count(),
                )
            })
            .collect();
        assert_eq!(
            counts,
            BTreeMap::from([
                ("codex", 14),
                ("codex_snake", 6),
                ("default", 17),
                ("gemini", 15),
                ("gemini_snake", 14),
                ("none", 0),
            ])
        );
    }

    #[test]
    fn production_names_are_unique_and_valid() {
        for toolset in ToolsetId::ALL {
            let selected: Vec<_> = rows().iter().filter(|row| row.toolset == toolset).collect();
            for (index, row) in selected.iter().enumerate() {
                assert!(lotta_runtime::ports::ModelFacingToolName::new(row.model.into()).is_ok());
                assert!(
                    !selected[..index]
                        .iter()
                        .any(|prior| prior.internal == row.internal)
                );
                assert!(
                    !selected[..index]
                        .iter()
                        .any(|prior| prior.model == row.model)
                );
            }
        }
    }
}
