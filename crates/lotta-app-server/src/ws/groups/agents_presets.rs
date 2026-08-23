//! Canonical personality-preset assets and creation policy.
//!
//! The embedded files under `agents_presets/` are byte-exact copies of the
//! pinned baseline's prompt and catalog assets (`letta-code/src/agent/
//! prompts/*`, `letta-code/src/models.json`), so shortcut creation produces
//! the canonical system prompt, memory-block content, descriptions, tags,
//! and model handles instead of paraphrased inline text.

use std::collections::HashSet;
use std::sync::OnceLock;

use lotta_domain::{MemoryBlockInput, NonEmptyString};
use serde::Deserialize;

use super::{GIT_MEMORY_ENABLED_TAG, PersonalityId};

/// Pinned tag marking agents created by Letta Code.
const ORIGIN_TAG: &str = "origin:letta-code";
/// Catalog model id resolved when neither the request nor the preset names one.
const DEFAULT_MODEL_ID: &str = "auto";
/// Canonical rejection detail for unresolvable request models.
const UNKNOWN_MODEL_DETAIL: &str = "Unknown model";
/// Canonical system prompt of the default preset in memfs mode
/// (pinned `buildSystemPrompt("default", "memfs")`, trimmed before use).
const SYSTEM_PROMPT_ASSET: &str = include_str!("agents_presets/letta.md");
/// Onboarding memory asset served to local tutorial agents.
const ONBOARDING_ASSET: &str = include_str!("agents_presets/onboarding_local.mdx");

/// One curated catalog entry used for handle resolution.
#[derive(Deserialize)]
struct CatalogModel {
    /// Short identifier (for example `auto-chat`).
    id: String,
    /// Canonical provider/model handle (for example `letta/auto-chat`).
    handle: String,
}

/// Embedded shape of the bundled catalog snapshot.
#[derive(Deserialize)]
struct Catalog {
    /// Every curated entry.
    models: Vec<CatalogModel>,
}

/// One canonical personality preset.
pub(super) struct PersonalityPreset {
    /// Display name persisted verbatim.
    pub(super) label: &'static str,
    /// Description persisted verbatim.
    pub(super) description: &'static str,
    /// Model id applied when the request omits one.
    pub(super) default_model: Option<&'static str>,
    /// Personality tag stamped only by presets shipping default files.
    pub(super) creation_tag: Option<&'static str>,
    persona_asset: &'static str,
    human_asset: &'static str,
}

/// Returns the canonical preset metadata for one personality.
pub(super) const fn preset(id: PersonalityId) -> PersonalityPreset {
    match id {
        PersonalityId::Memo => PersonalityPreset {
            label: "Letta Code",
            description: "The memory-first agent",
            default_model: None,
            creation_tag: None,
            persona_asset: include_str!("agents_presets/persona_memo.mdx"),
            human_asset: include_str!("agents_presets/human_memo.mdx"),
        },
        PersonalityId::Tutorial => PersonalityPreset {
            label: "Tutor",
            description: "I help with getting started with Letta. I can answer any questions \
                          about Letta, and also help you create and configure agents.",
            default_model: None,
            creation_tag: Some("personality:tutorial"),
            persona_asset: include_str!("agents_presets/persona_tutorial.mdx"),
            human_asset: include_str!("agents_presets/human_tutorial.mdx"),
        },
        PersonalityId::Blank => PersonalityPreset {
            label: "Blank",
            description: "Blank starter — you provide the personality",
            default_model: None,
            creation_tag: None,
            persona_asset: include_str!("agents_presets/persona_blank.mdx"),
            human_asset: include_str!("agents_presets/human.mdx"),
        },
        PersonalityId::Linus => PersonalityPreset {
            label: "Linus",
            description: "Code with a stern hand",
            default_model: None,
            creation_tag: None,
            persona_asset: include_str!("agents_presets/persona_linus.mdx"),
            human_asset: include_str!("agents_presets/human_linus.mdx"),
        },
        PersonalityId::Kawaii => PersonalityPreset {
            label: "Letta-Chan",
            description: "sugoi~ (◕‿◕)✨",
            default_model: Some("auto-chat"),
            creation_tag: None,
            persona_asset: include_str!("agents_presets/persona_kawaii.mdx"),
            human_asset: include_str!("agents_presets/human_kawaii.mdx"),
        },
    }
}

/// Borrows the embedded catalog, parsed once per process.
fn catalog() -> &'static [CatalogModel] {
    static CATALOG: OnceLock<Vec<CatalogModel>> = OnceLock::new();
    CATALOG.get_or_init(|| {
        serde_json::from_str::<Catalog>(include_str!("agents_presets/models.json"))
            .map_or_else(|_| Vec::new(), |catalog| catalog.models)
    })
}

/// Resolves one model identifier exactly like the pinned catalog resolver.
///
/// A catalog id or full handle maps to the canonical handle; any other
/// slash-bearing value passes through for self-hosted inventories.
pub(super) fn resolve_model(identifier: &str) -> Option<String> {
    let models = catalog();
    if let Some(found) = models.iter().find(|model| model.id == identifier) {
        return Some(found.handle.clone());
    }
    if let Some(found) = models.iter().find(|model| model.handle == identifier) {
        return Some(found.handle.clone());
    }
    identifier.contains('/').then(|| identifier.to_owned())
}

/// Resolves the model applied when neither the request nor the preset names
/// one (pinned `getDefaultModel`).
fn default_model_handle() -> Option<String> {
    resolve_model(DEFAULT_MODEL_ID)
}

/// Resolves the shortcut's effective model before any side effect runs.
///
/// # Errors
/// Rejects explicit identifiers that no catalog entry or slash passthrough
/// resolves, with the pinned `Unknown model "…"` detail.
pub(super) fn resolve_request_model(
    requested: Option<&str>,
    preset: &PersonalityPreset,
) -> Result<String, String> {
    let Some(identifier) = requested.or(preset.default_model) else {
        return default_model_handle().ok_or_else(|| "No models available".to_owned());
    };
    resolve_model(identifier).ok_or_else(|| format!("{UNKNOWN_MODEL_DETAIL} \"{identifier}\""))
}

/// Returns the canonical system prompt for created agents (memfs variant).
pub(super) fn system_prompt() -> String {
    SYSTEM_PROMPT_ASSET.trim().to_owned()
}

/// Returns the deduplicated creation tag list in pinned stamping order:
/// origin tag, Git-memory tag, preset personality tag, then caller tags.
pub(super) fn creation_tags(preset: &PersonalityPreset, extra: Option<&[String]>) -> Vec<String> {
    let mut tags = vec![ORIGIN_TAG.to_owned(), GIT_MEMORY_ENABLED_TAG.to_owned()];
    if let Some(tag) = preset.creation_tag {
        tags.push(tag.to_owned());
    }
    if let Some(extra) = extra {
        tags.extend(extra.iter().cloned());
    }
    let mut unique = Vec::with_capacity(tags.len());
    let mut seen = HashSet::with_capacity(tags.len());
    for tag in tags {
        if seen.insert(tag.clone()) {
            unique.push(tag);
        }
    }
    unique
}

/// Builds the initial memory blocks every preset seeds: the persona and
/// human blocks rendered from the preset's canonical assets plus the local
/// onboarding block for onboarding personalities.
///
/// # Errors
/// Fails only if an embedded label were ever emptied, which the type system
/// cannot express for `'static` literals.
pub(super) fn memory_blocks(preset: &PersonalityPreset) -> Result<Vec<MemoryBlockInput>, ()> {
    let mut blocks = vec![
        asset_block(preset.persona_asset)?,
        asset_block(preset.human_asset)?,
    ];
    if preset.creation_tag.is_some() {
        blocks.push(asset_block(ONBOARDING_ASSET)?);
    }
    Ok(blocks)
}

/// Renders one memory block from one embedded MDX asset.
fn asset_block(content: &str) -> Result<MemoryBlockInput, ()> {
    let (frontmatter, body) = split_frontmatter(content);
    Ok(MemoryBlockInput {
        label: NonEmptyString::new(frontmatter_property(frontmatter, "label").ok_or(())?)
            .map_err(|_| ())?,
        value: prompt_body(body),
        description: frontmatter_description(frontmatter).map(Some),
    })
}

/// Splits one MDX asset into frontmatter text and body like the pinned
/// `parseMdxFrontmatter`; assets always carry wellformed frontmatter.
fn split_frontmatter(content: &str) -> (&str, &str) {
    content.strip_prefix("---\n").map_or(("", content), |rest| {
        rest.find("\n---\n")
            .map_or(("", content), |end| (&rest[..end], &rest[end + 5..]))
    })
}

/// Reads one simple `key: value` frontmatter property.
fn frontmatter_property(frontmatter: &str, wanted: &str) -> Option<String> {
    for line in frontmatter.lines() {
        match line.split_once(':') {
            Some((key, value)) if !key.trim().is_empty() && key.trim() == wanted => {
                return Some(value.trim().to_owned());
            }
            _ => {}
        }
    }
    None
}
/// Reads the frontmatter `description` property.
fn frontmatter_description(frontmatter: &str) -> Option<String> {
    frontmatter_property(frontmatter, "description")
}

/// Renders one block value exactly like the pinned `getPromptBody`:
/// trimmed content with exactly one trailing line feed.
fn prompt_body(body: &str) -> String {
    format!("{}\n", body.trim())
}
