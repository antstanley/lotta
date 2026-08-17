use super::types::{MOD_REGISTRATIONS_ITEMS_MAX, ModError, ModOwner, RegistrationName};
use lotta_runtime::hooks::HookEvent;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::{BTreeMap, BTreeSet};

/// One mod tool registration matching the pinned API surface.
#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct ToolRegistration {
    /// Stable tool name.
    pub name: RegistrationName,
    /// Bounded description.
    pub description: String,
    /// JSON Schema accepted by the tool.
    pub input_schema: Value,
    /// Exact owner generation.
    pub owner: ModOwner,
}
/// One callable command registration.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct CommandRegistration {
    /// Command identifier.
    pub id: RegistrationName,
    /// Bounded description.
    pub description: String,
    /// Optional argument synopsis.
    pub args: Option<String>,
    /// Exact owner generation.
    pub owner: ModOwner,
}
/// One accepted provider declaration.
#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct ProviderRegistration {
    /// Provider name.
    pub name: RegistrationName,
    /// Dependency-neutral configuration metadata.
    pub config: Value,
    /// Exact owner generation.
    pub owner: ModOwner,
}
/// One accepted permission declaration.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct PermissionRegistration {
    /// Permission identifier.
    pub id: RegistrationName,
    /// Human-readable bounded description.
    pub description: String,
    /// Exact owner generation.
    pub owner: ModOwner,
}
/// One lifecycle registration adapted to Task 44's exact event set.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct LifecycleRegistration {
    /// Registration identifier.
    pub id: RegistrationName,
    /// Reused Task 44 event discriminant.
    pub event: HookEvent,
    /// Exact owner generation.
    pub owner: ModOwner,
}
/// Accepted headless UI metadata.
#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct UiRegistration {
    /// Panel identifier.
    pub id: RegistrationName,
    /// Panel title.
    pub title: String,
    /// Opaque metadata consumed by later transports.
    pub metadata: Value,
    /// Exact owner generation.
    pub owner: ModOwner,
}

/// Complete wire registration batch from one owner generation.
#[derive(Clone, Debug, Default, Deserialize, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct RegistrationBatch {
    /// Tool registrations.
    pub tools: Vec<ToolRegistration>,
    /// Command registrations.
    pub commands: Vec<CommandRegistration>,
    /// Provider registrations.
    pub providers: Vec<ProviderRegistration>,
    /// Permission registrations.
    pub permissions: Vec<PermissionRegistration>,
    /// Lifecycle registrations.
    pub lifecycle_events: Vec<LifecycleRegistration>,
    /// UI registrations.
    pub ui_metadata: Vec<UiRegistration>,
}

/// Immutable atomically published six-kind registration snapshot.
#[derive(Clone, Debug, Default)]
pub struct ModRegistrationSnapshot {
    /// Tools indexed by stable name.
    pub tools: BTreeMap<RegistrationName, ToolRegistration>,
    /// Commands indexed by identifier.
    pub commands: BTreeMap<RegistrationName, CommandRegistration>,
    /// Providers indexed by name.
    pub providers: BTreeMap<RegistrationName, ProviderRegistration>,
    /// Permissions indexed by identifier.
    pub permissions: BTreeMap<RegistrationName, PermissionRegistration>,
    /// Lifecycle registrations indexed by identifier.
    pub lifecycle_events: BTreeMap<RegistrationName, LifecycleRegistration>,
    /// UI metadata indexed by identifier.
    pub ui_metadata: BTreeMap<RegistrationName, UiRegistration>,
}
impl ModRegistrationSnapshot {
    /// Validates a complete owner-generation batch before constructing a snapshot.
    pub fn from_batch(owner: &ModOwner, batch: RegistrationBatch) -> Result<Self, ModError> {
        if batch_len(&batch)? > MOD_REGISTRATIONS_ITEMS_MAX {
            return Err(ModError::InvalidRegistration);
        }
        let mut names = BTreeSet::new();
        validate_batch(owner, &batch, &mut names)?;
        Ok(Self {
            tools: index(batch.tools, |item| item.name.clone()),
            commands: index(batch.commands, |item| item.id.clone()),
            providers: index(batch.providers, |item| item.name.clone()),
            permissions: index(batch.permissions, |item| item.id.clone()),
            lifecycle_events: index(batch.lifecycle_events, |item| item.id.clone()),
            ui_metadata: index(batch.ui_metadata, |item| item.id.clone()),
        })
    }
    /// Number of mod-owned registrations across all six stores.
    #[must_use]
    pub fn len(&self) -> usize {
        self.tools.len()
            + self.commands.len()
            + self.providers.len()
            + self.permissions.len()
            + self.lifecycle_events.len()
            + self.ui_metadata.len()
    }
    /// Whether every mod store is empty.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }
    /// Removes every registration belonging to an owner identity.
    #[must_use]
    pub fn without_owner(&self, owner_id: &str) -> Self {
        Self {
            tools: retain(&self.tools, owner_id, |item| &item.owner),
            commands: retain(&self.commands, owner_id, |item| &item.owner),
            providers: retain(&self.providers, owner_id, |item| &item.owner),
            permissions: retain(&self.permissions, owner_id, |item| &item.owner),
            lifecycle_events: retain(&self.lifecycle_events, owner_id, |item| &item.owner),
            ui_metadata: retain(&self.ui_metadata, owner_id, |item| &item.owner),
        }
    }
}

fn batch_len(batch: &RegistrationBatch) -> Result<usize, ModError> {
    let counts = [
        batch.tools.len(),
        batch.commands.len(),
        batch.providers.len(),
        batch.permissions.len(),
        batch.lifecycle_events.len(),
        batch.ui_metadata.len(),
    ];
    counts
        .into_iter()
        .try_fold(0_usize, usize::checked_add)
        .ok_or(ModError::InvalidRegistration)
}
fn validate_batch(
    owner: &ModOwner,
    batch: &RegistrationBatch,
    names: &mut BTreeSet<String>,
) -> Result<(), ModError> {
    for (name, actual, description) in batch
        .tools
        .iter()
        .map(|v| (&v.name, &v.owner, &v.description))
        .chain(
            batch
                .commands
                .iter()
                .map(|v| (&v.id, &v.owner, &v.description)),
        )
        .chain(
            batch
                .permissions
                .iter()
                .map(|v| (&v.id, &v.owner, &v.description)),
        )
    {
        validate_item(owner, actual, name, description, names)?;
    }
    for (name, actual) in batch
        .providers
        .iter()
        .map(|v| (&v.name, &v.owner))
        .chain(batch.lifecycle_events.iter().map(|v| (&v.id, &v.owner)))
        .chain(batch.ui_metadata.iter().map(|v| (&v.id, &v.owner)))
    {
        validate_item(owner, actual, name, "metadata", names)?;
    }
    for tool in &batch.tools {
        if !tool.input_schema.is_object() {
            return Err(ModError::InvalidRegistration);
        }
    }
    Ok(())
}
fn validate_item(
    owner: &ModOwner,
    actual: &ModOwner,
    name: &RegistrationName,
    description: &str,
    names: &mut BTreeSet<String>,
) -> Result<(), ModError> {
    if owner != actual || description.len() > 16_384 || !names.insert(name.as_str().to_owned()) {
        return Err(ModError::InvalidRegistration);
    }
    Ok(())
}
fn index<T>(items: Vec<T>, name: impl Fn(&T) -> RegistrationName) -> BTreeMap<RegistrationName, T> {
    items.into_iter().map(|item| (name(&item), item)).collect()
}
fn retain<T: Clone>(
    values: &BTreeMap<RegistrationName, T>,
    owner_id: &str,
    owner: impl Fn(&T) -> &ModOwner,
) -> BTreeMap<RegistrationName, T> {
    values
        .iter()
        .filter(|(_, value)| owner(value).id.as_str() != owner_id)
        .map(|(name, value)| (name.clone(), value.clone()))
        .collect()
}
