use super::host::ModHost;
use super::registrations::{ModRegistrationSnapshot, RegistrationBatch, ToolRegistration};
use super::registry::{ModPublication, ModRegistries};
use super::test_support::*;
use super::types::{ModError, ModOwner};
use lotta_runtime::ports::ToolExecutionOwner;
use lotta_tools::registry::ToolRegistry;
use lotta_tools::toolset::ToolsetId;
use serde_json::json;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Barrier};

fn stores() -> (Arc<ToolRegistry>, Arc<ModRegistries>) {
    let tools = Arc::new(ToolRegistry::new([]).expect("empty builtins"));
    let registries = Arc::new(ModRegistries::new(Arc::clone(&tools)));
    (tools, registries)
}

fn publication(mod_owner: &ModOwner, prefix: &str) -> ModPublication {
    let snapshot = ModRegistrationSnapshot::from_batch(mod_owner, batch(mod_owner, prefix))
        .expect("validated batch");
    ModPublication {
        owner: mod_owner.clone(),
        registrations: Arc::new(snapshot),
        host: StubHost::new() as Arc<dyn ModHost>,
    }
}

fn pair() -> Vec<ModPublication> {
    vec![
        publication(&owner("project:left.ts", 1), "left"),
        publication(&owner("project:right.ts", 1), "right"),
    ]
}

fn mod_tools(snapshot: &lotta_tools::registry::RegistrySnapshot) -> usize {
    snapshot
        .model_names()
        .iter()
        .filter(|value| value.ends_with("_tool"))
        .count()
}

fn assert_complete(registrations: &ModRegistrationSnapshot) {
    let count = registrations.tools.len();
    assert!(count == 0 || count == 2, "partial mod world: {count}");
    assert_eq!(registrations.commands.len(), count);
    assert_eq!(registrations.providers.len(), count);
    assert_eq!(registrations.permissions.len(), count);
    assert_eq!(registrations.lifecycle_events.len(), count);
    assert_eq!(registrations.ui_metadata.len(), count);
    assert_eq!(registrations.len(), count * 6);
}

#[test]
fn commit_publishes_registrations_and_tools_in_one_transaction() {
    let (tools, registries) = stores();
    let before = tools.revision().unwrap();

    registries.commit(&pair()).unwrap();

    assert_eq!(tools.revision().unwrap(), before + 1);
    assert_eq!(registries.snapshot().unwrap().len(), 12);
    assert_eq!(mod_tools(&tools.snapshot().unwrap()), 2);
    assert_eq!(registries.generations().len().unwrap(), 2);
}

#[test]
fn a_losing_transaction_leaves_snapshot_revision_and_generations_unchanged() {
    let (tools, registries) = stores();
    registries.commit(&pair()).unwrap();
    let settled = tools.revision().unwrap();
    let gate = Arc::new(Barrier::new(2));

    let writer_registries = Arc::clone(&registries);
    let writer_gate = Arc::clone(&gate);
    let writer = std::thread::spawn(move || {
        let replacement = vec![publication(&owner("project:only.ts", 1), "only")];
        writer_registries.commit_with_barrier(&replacement, || {
            writer_gate.wait();
            writer_gate.wait();
        })
    });

    gate.wait();
    // A competing Task 32 publication advances the revision inside the only window that can
    // still invalidate the composed candidate.
    tools
        .update(
            ToolsetId::None,
            &[runtime_tool("native", ToolExecutionOwner::Rust)],
            None,
        )
        .unwrap();
    gate.wait();

    assert_eq!(writer.join().unwrap(), Err(ModError::Publication));
    assert_eq!(tools.revision().unwrap(), settled + 1);
    assert_eq!(registries.snapshot().unwrap().len(), 12);
    assert_eq!(registries.generations().len().unwrap(), 2);
    assert!(
        registries
            .generations()
            .current(&owner("project:only.ts", 1).id)
            .unwrap()
            .is_none()
    );
}

#[test]
fn the_barrier_runs_before_the_registry_write_lock() {
    let (tools, registries) = stores();
    let observed = Arc::new(AtomicU64::new(u64::MAX));
    let before = tools.revision().unwrap();

    let sampled = Arc::clone(&observed);
    let registry_handle = Arc::clone(&tools);
    registries
        .commit_with_barrier(&pair(), move || {
            // Reading the revision here would deadlock if the write lock were already held.
            let revision = registry_handle
                .revision()
                .expect("registry readable at the barrier");
            sampled.store(revision, Ordering::SeqCst);
        })
        .unwrap();

    assert_eq!(observed.load(Ordering::SeqCst), before);
    assert_eq!(tools.revision().unwrap(), before + 1);
}

#[test]
fn a_rejected_candidate_changes_nothing() {
    let (tools, registries) = stores();
    registries.commit(&pair()).unwrap();
    let settled = tools.revision().unwrap();
    let duplicate = owner("project:clash.ts", 1);
    let colliding = ModPublication {
        owner: duplicate.clone(),
        registrations: Arc::new(
            ModRegistrationSnapshot::from_batch(&duplicate, batch(&duplicate, "left")).unwrap(),
        ),
        host: StubHost::new() as Arc<dyn ModHost>,
    };
    let mut candidate = pair();
    candidate.push(colliding);

    assert_eq!(
        registries.commit(&candidate),
        Err(ModError::InvalidRegistration)
    );

    assert_eq!(tools.revision().unwrap(), settled);
    assert_eq!(registries.snapshot().unwrap().len(), 12);
    assert_eq!(registries.generations().len().unwrap(), 2);
    assert!(
        registries
            .generations()
            .current(&duplicate.id)
            .unwrap()
            .is_none()
    );
}

#[test]
fn an_invalid_tool_candidate_leaves_the_previous_world_intact() {
    let (tools, registries) = stores();
    registries.commit(&pair()).unwrap();
    let settled = tools.revision().unwrap();
    let broken = owner("project:broken.ts", 1);
    // The mod contract accepts any JSON object as a schema; Task 32 additionally requires an
    // object-typed schema, so this candidate can only fail while composing the tool registration.
    let batch = RegistrationBatch {
        tools: vec![ToolRegistration {
            name: name("broken_tool"),
            description: "broken".into(),
            input_schema: json!({"type":"array"}),
            owner: broken.clone(),
        }],
        ..RegistrationBatch::default()
    };
    let candidate = vec![ModPublication {
        owner: broken.clone(),
        registrations: Arc::new(ModRegistrationSnapshot::from_batch(&broken, batch).unwrap()),
        host: StubHost::new() as Arc<dyn ModHost>,
    }];

    assert!(registries.commit(&candidate).is_err());

    assert_eq!(tools.revision().unwrap(), settled);
    assert_eq!(registries.snapshot().unwrap().len(), 12);
    assert_eq!(mod_tools(&tools.snapshot().unwrap()), 2);
}

#[test]
fn concurrent_readers_never_observe_a_partial_world() {
    let (tools, registries) = stores();
    let done = Arc::new(AtomicBool::new(false));
    let samples = Arc::new(AtomicU64::new(0));
    let start = Arc::new(Barrier::new(2));

    let reader_registries = Arc::clone(&registries);
    let reader_tools = Arc::clone(&tools);
    let reader_done = Arc::clone(&done);
    let reader_samples = Arc::clone(&samples);
    let reader_start = Arc::clone(&start);
    let reader = std::thread::spawn(move || {
        reader_start.wait();
        while !reader_done.load(Ordering::Acquire) {
            assert_complete(&reader_registries.snapshot().unwrap());
            let published = mod_tools(&reader_tools.snapshot().unwrap());
            assert!(published == 0 || published == 2, "partial toolset");
            reader_samples.fetch_add(1, Ordering::Release);
        }
    });

    start.wait();
    while samples.load(Ordering::Acquire) == 0 {
        std::thread::yield_now();
    }
    for index in 0..100 {
        if index % 2 == 0 {
            registries.commit(&pair()).unwrap();
        } else {
            registries.clear_mods().unwrap();
        }
    }
    done.store(true, Ordering::Release);
    reader.join().unwrap();

    assert!(samples.load(Ordering::Acquire) > 0);
    assert_complete(&registries.snapshot().unwrap());
}
