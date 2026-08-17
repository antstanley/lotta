use super::capabilities::{CapabilityBrokerTable, CapabilityPort};
use super::host::{FramedModHost, ModHost, ModHostLauncher};
use super::protocol::{RpcMethod, RpcParams, RpcResult};
use super::registrations::{ModRegistrationSnapshot, RegistrationBatch};
use super::registry::{ModPublication, ModRegistries, ModRuntimeSnapshot};
use super::safe_mode::{
    MOD_SKIP_REASON_LOAD_FAILED, ModTrust, ModsStartupMode, SafeModeDiagnostic, skip_reason,
};
use super::types::{
    Capability, ConversationHandle, Generation, MOD_DIAGNOSTIC_BYTES_MAX,
    MOD_DIAGNOSTICS_ITEMS_MAX, ModError, ModId, ModOwner, ModRuntimeScope,
};
use crate::sidecar::SidecarOwnerIdentity;
use crate::sidecar::supervisor::SidecarSupervisor;
use lotta_domain::Clock;
use std::{collections::BTreeMap, sync::Arc, time::Duration};
use tokio::sync::Mutex;
use tokio_util::sync::CancellationToken;

/// Maximum handshake wait for a compatibility child.
pub const MOD_HOST_HANDSHAKE_TIMEOUT: Duration = Duration::from_secs(10);
/// Initial generation used for first activation.
pub const MOD_GENERATION_INITIAL: Generation = Generation(1);
/// Maximum compatibility children the controller retains at once.
pub const MOD_HOSTS_ITEMS_MAX: usize = 64;

/// Complete per-mod startup declaration supplied by the composition root.
#[derive(Clone)]
pub struct ModStartSpec {
    /// Exact owner generation.
    pub owner: ModOwner,
    /// Pinned source trust used by safe mode.
    pub trust: ModTrust,
    /// Exact Task 42 owner identity.
    pub sidecar_owner: SidecarOwnerIdentity,
    /// Declared capabilities granted to this mod only.
    pub capabilities: Vec<Capability>,
    /// Runtime scope retained privately by the broker and never sent to the child.
    pub scope: ModRuntimeScope,
}

/// One start candidate paired with its exact injected launcher.
pub struct ModStartRequest<L> {
    /// Startup declaration.
    pub spec: ModStartSpec,
    /// Launcher used only after supervisor admission.
    pub launcher: L,
}

/// Owner-attributed diagnostics collected over the real host protocol.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ModDiagnostic {
    /// Owner the diagnostics are attributed to.
    pub owner: ModOwner,
    /// Bounded stable entries returned by the host.
    pub entries: Vec<String>,
}

struct ActiveHost {
    spec: ModStartSpec,
    host: Arc<dyn ModHost>,
    handle: ConversationHandle,
    registrations: Arc<ModRegistrationSnapshot>,
}
impl ActiveHost {
    fn publication(&self) -> ModPublication {
        ModPublication {
            owner: self.spec.owner.clone(),
            registrations: Arc::clone(&self.registrations),
            host: Arc::clone(&self.host),
        }
    }
}

/// Public lifecycle controller for external TypeScript compatibility children.
///
/// The controller owns the capability broker table and the injected runtime port. It mints one
/// private opaque handle immediately before accepting a child and revokes it as soon as that exact
/// generation stops being callable, so a child can never resolve a scope it no longer owns.
pub struct ModHostController<C: Clock> {
    clock: Arc<C>,
    registries: Arc<ModRegistries>,
    brokers: Arc<CapabilityBrokerTable>,
    port: Arc<dyn CapabilityPort>,
    supervisors: Mutex<BTreeMap<String, SidecarSupervisor<C>>>,
    active: Mutex<BTreeMap<String, ActiveHost>>,
    startup: Mutex<Vec<SafeModeDiagnostic>>,
}
impl<C: Clock> ModHostController<C> {
    /// Creates an absent controller over shared registries, supervision, and one runtime port.
    #[must_use]
    pub fn new(
        clock: Arc<C>,
        registries: Arc<ModRegistries>,
        port: Arc<dyn CapabilityPort>,
    ) -> Self {
        Self {
            clock,
            registries,
            brokers: Arc::new(CapabilityBrokerTable::new()),
            port,
            supervisors: Mutex::new(BTreeMap::new()),
            active: Mutex::new(BTreeMap::new()),
            startup: Mutex::new(Vec::new()),
        }
    }

    /// Borrows the broker table the controller owns and hands to every accepted child.
    #[must_use]
    pub fn brokers(&self) -> &Arc<CapabilityBrokerTable> {
        &self.brokers
    }

    /// Starts the exact set of mods the mode retains and publishes them in one transaction.
    ///
    /// `NoMods` launches nothing and removes every mod-owned registration. `SafeMode` skips
    /// third-party sources. A candidate that fails to load is reported as an owner-attributed
    /// diagnostic and never partially published.
    pub async fn start<L: ModHostLauncher>(
        &self,
        mode: ModsStartupMode,
        requests: Vec<ModStartRequest<L>>,
    ) -> Result<Vec<SafeModeDiagnostic>, ModError>
    where
        L::Child: Sync + 'static,
    {
        if requests.len() > MOD_HOSTS_ITEMS_MAX {
            return Err(ModError::InvalidRegistration);
        }
        let mut diagnostics = Vec::new();
        let mut prepared = Vec::new();
        for mut request in requests {
            if let Some(reason) = skip_reason(mode, request.spec.trust) {
                diagnostics.push(diagnostic(&request.spec.owner, reason));
                continue;
            }
            match self
                .prepare(&mut request.launcher, &request.spec, true)
                .await
            {
                Ok(active) => prepared.push(active),
                Err(_) => {
                    diagnostics.push(diagnostic(&request.spec.owner, MOD_SKIP_REASON_LOAD_FAILED));
                }
            }
        }
        self.replace_world(prepared).await?;
        *self.startup.lock().await = diagnostics.clone();
        Ok(diagnostics)
    }

    /// Reloads exactly one owner, leaving every other retained host callable and unchanged.
    ///
    /// A failed candidate is aborted with its handle revoked, and the previous generation keeps its
    /// exact registrations, tool revision, and callability.
    pub async fn reload<L: ModHostLauncher>(
        &self,
        launcher: &mut L,
        id: &ModId,
    ) -> Result<(), ModError>
    where
        L::Child: Sync + 'static,
    {
        let previous = {
            let active = self.active.lock().await;
            let found = active.get(id.as_str()).ok_or(ModError::Unavailable)?;
            (
                found.spec.clone(),
                Arc::clone(&found.host),
                found.handle.clone(),
            )
        };
        let mut spec = previous.0;
        spec.owner.generation = spec.owner.generation.next()?;
        let candidate = self.prepare(launcher, &spec, false).await?;
        let mut publications = self.publications_excluding(id.as_str()).await;
        publications.push(candidate.publication());
        if let Err(error) = self.registries.commit(&publications) {
            self.discard(candidate).await;
            return Err(error);
        }
        self.active
            .lock()
            .await
            .insert(id.as_str().to_owned(), candidate);
        let _ = previous.1.dispose().await;
        self.brokers.revoke(&previous.2)?;
        Ok(())
    }

    /// Disposes exactly one owner, retaining every other host's registrations.
    pub async fn dispose(&self, id: &ModId) -> Result<(), ModError> {
        if !self.active.lock().await.contains_key(id.as_str()) {
            return Err(ModError::Unavailable);
        }
        let publications = self.publications_excluding(id.as_str()).await;
        self.registries.commit(&publications)?;
        let removed = self.active.lock().await.remove(id.as_str());
        if let Some(active) = removed {
            let _ = active.host.dispose().await;
            self.brokers.revoke(&active.handle)?;
        }
        Ok(())
    }

    /// Disposes every retained host and removes every mod-owned registration.
    pub async fn dispose_all(&self) -> Result<(), ModError> {
        self.replace_world(Vec::new()).await
    }

    /// Returns the aggregated generation-checking runtime view over all retained hosts.
    pub fn snapshot(&self) -> Result<ModRuntimeSnapshot, ModError> {
        self.registries.runtime()
    }

    /// Returns the exact owners currently retained, in stable order.
    pub async fn active_owners(&self) -> Vec<ModOwner> {
        self.active
            .lock()
            .await
            .values()
            .map(|active| active.spec.owner.clone())
            .collect()
    }

    /// Collects owner-attributed diagnostics from every retained host over real RPC.
    ///
    /// A host that refuses or fails its diagnostics call is reported with no entries rather than
    /// failing the whole collection, because diagnostics are observability, not control flow.
    pub async fn diagnostics(&self) -> Vec<ModDiagnostic> {
        let owners: Vec<_> = {
            let active = self.active.lock().await;
            active
                .values()
                .map(|active| (active.spec.owner.clone(), Arc::clone(&active.host)))
                .collect()
        };
        let mut collected = Vec::with_capacity(owners.len());
        for (owner, host) in owners {
            let entries = request_diagnostics(host.as_ref(), &owner)
                .await
                .unwrap_or_default();
            collected.push(ModDiagnostic { owner, entries });
        }
        collected
    }

    /// Collects diagnostics for exactly one retained owner over real RPC.
    pub async fn diagnostics_for(&self, id: &ModId) -> Result<ModDiagnostic, ModError> {
        let found = {
            let active = self.active.lock().await;
            let active = active.get(id.as_str()).ok_or(ModError::Unavailable)?;
            (active.spec.owner.clone(), Arc::clone(&active.host))
        };
        let entries = request_diagnostics(found.1.as_ref(), &found.0).await?;
        Ok(ModDiagnostic {
            owner: found.0,
            entries,
        })
    }

    /// Returns the retained startup diagnostics from the last `start`.
    pub async fn startup_diagnostics(&self) -> Vec<SafeModeDiagnostic> {
        self.startup.lock().await.clone()
    }

    async fn replace_world(&self, prepared: Vec<ActiveHost>) -> Result<(), ModError> {
        let publications: Vec<_> = prepared.iter().map(ActiveHost::publication).collect();
        if let Err(error) = self.registries.commit(&publications) {
            for candidate in prepared {
                self.discard(candidate).await;
            }
            return Err(error);
        }
        let mut next = BTreeMap::new();
        for candidate in prepared {
            next.insert(candidate.spec.owner.id.as_str().to_owned(), candidate);
        }
        let previous = std::mem::replace(&mut *self.active.lock().await, next);
        for (_, active) in previous {
            let _ = active.host.dispose().await;
            self.brokers.revoke(&active.handle)?;
        }
        Ok(())
    }

    async fn prepare<L: ModHostLauncher>(
        &self,
        launcher: &mut L,
        spec: &ModStartSpec,
        initial: bool,
    ) -> Result<ActiveHost, ModError>
    where
        L::Child: Sync + 'static,
    {
        self.admit(&spec.owner, initial).await?;
        let child = launcher.launch().await?;
        let handle = self.brokers.mint(
            spec.capabilities.iter().copied(),
            spec.owner.clone(),
            spec.scope.clone(),
            Arc::clone(&self.port),
        )?;
        let accepted = FramedModHost::accept_with_brokers(
            child,
            spec.owner.clone(),
            spec.sidecar_owner.clone(),
            MOD_HOST_HANDSHAKE_TIMEOUT,
            Arc::clone(&self.brokers),
        )
        .await;
        let host = match accepted {
            Ok(host) => host,
            Err(error) => {
                self.brokers.revoke(&handle)?;
                return Err(error);
            }
        };
        match introduce(host.as_ref(), spec, &handle).await {
            Ok(registrations) => Ok(ActiveHost {
                spec: spec.clone(),
                host,
                handle,
                registrations,
            }),
            Err(error) => {
                let _ = host.abort().await;
                self.brokers.revoke(&handle)?;
                Err(error)
            }
        }
    }

    async fn discard(&self, candidate: ActiveHost) {
        let _ = candidate.host.abort().await;
        let _ = self.brokers.revoke(&candidate.handle);
    }

    async fn publications_excluding(&self, id: &str) -> Vec<ModPublication> {
        self.active
            .lock()
            .await
            .iter()
            .filter(|(key, _)| key.as_str() != id)
            .map(|(_, active)| active.publication())
            .collect()
    }

    async fn admit(&self, owner: &ModOwner, initial: bool) -> Result<(), ModError> {
        let mut supervisors = self.supervisors.lock().await;
        let key = owner.id.as_str().to_owned();
        // A brand-new owner consumes its one free initial launch; every later admission for that
        // identity, including a fresh `start`, consumes rolling restart quota.
        let fresh = !supervisors.contains_key(&key);
        let supervisor = supervisors
            .entry(key)
            .or_insert_with(|| SidecarSupervisor::new(Arc::clone(&self.clock)));
        let admitted = if fresh && initial {
            supervisor.admit_initial()
        } else {
            supervisor.admit_restart()
        };
        admitted.map_err(|_| ModError::Unavailable)
    }
}

fn diagnostic(owner: &ModOwner, reason: &'static str) -> SafeModeDiagnostic {
    SafeModeDiagnostic {
        owner: owner.clone(),
        reason,
    }
}

async fn introduce(
    host: &dyn ModHost,
    spec: &ModStartSpec,
    handle: &ConversationHandle,
) -> Result<Arc<ModRegistrationSnapshot>, ModError> {
    initialize(host, &spec.owner, spec.capabilities.clone(), handle.clone()).await?;
    let batch = request_registrations(host, &spec.owner).await?;
    let snapshot = ModRegistrationSnapshot::from_batch(&spec.owner, batch)?;
    Ok(Arc::new(snapshot))
}

async fn request_diagnostics(
    host: &dyn ModHost,
    owner: &ModOwner,
) -> Result<Vec<String>, ModError> {
    let params = RpcParams::Diagnostics {
        owner: owner.clone(),
    };
    let result = host
        .call(
            owner,
            RpcMethod::Diagnostics,
            params,
            CancellationToken::new(),
        )
        .await?;
    let RpcResult::Diagnostics { mut diagnostics } = result else {
        return Err(ModError::Protocol);
    };
    diagnostics.truncate(MOD_DIAGNOSTICS_ITEMS_MAX);
    for item in &mut diagnostics {
        if item.len() > MOD_DIAGNOSTIC_BYTES_MAX {
            item.truncate(MOD_DIAGNOSTIC_BYTES_MAX);
        }
    }
    Ok(diagnostics)
}

async fn request_registrations(
    host: &dyn ModHost,
    owner: &ModOwner,
) -> Result<RegistrationBatch, ModError> {
    let params = RpcParams::Register {
        owner: owner.clone(),
        registrations: RegistrationBatch::default(),
    };
    let result = host
        .call(owner, RpcMethod::Register, params, CancellationToken::new())
        .await?;
    match result {
        RpcResult::RegistrationBatch { registrations } => Ok(registrations),
        _ => Err(ModError::Protocol),
    }
}

async fn initialize(
    host: &dyn ModHost,
    owner: &ModOwner,
    capabilities: Vec<Capability>,
    handle: ConversationHandle,
) -> Result<(), ModError> {
    let params = RpcParams::Initialize {
        owner: owner.clone(),
        capabilities,
        conversation_handle: handle,
    };
    host.call(
        owner,
        RpcMethod::Initialize,
        params,
        CancellationToken::new(),
    )
    .await?;
    Ok(())
}
