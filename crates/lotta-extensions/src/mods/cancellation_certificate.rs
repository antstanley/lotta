use super::capabilities::{CapabilityBrokerTable, CapabilityPort};
use super::host::{FramedModHost, ModHost};
use super::protocol::{RpcMethod, RpcParams, RpcResult};
use super::test_support::*;
use super::types::{Capability, ModError, ModOwner};
use serde_json::json;
use std::sync::Arc;
use std::sync::atomic::Ordering;
use std::time::Duration;
use tokio_util::sync::CancellationToken;

/// Per-call bound used by these certificates; paused time advances it deterministically.
const CALL_TIMEOUT: Duration = Duration::from_millis(200);

type Host = Arc<FramedModHost<ScriptedChild>>;

async fn accept(plan: ChildPlan, mod_owner: &ModOwner) -> (Host, Arc<ChildCounters>) {
    let sidecar = sidecar_owner(mod_owner.id.as_str());
    let child = ScriptedChild::new(plan, sidecar.clone());
    let counters = child.counters();
    let host = FramedModHost::accept(child, mod_owner.clone(), sidecar, CALL_TIMEOUT)
        .await
        .expect("handshake accepted");
    (host, counters)
}

fn tool_call(mod_owner: &ModOwner) -> RpcParams {
    RpcParams::ToolCall {
        owner: mod_owner.clone(),
        name: "probe_tool".into(),
        tool_call_id: "call-1".into(),
        input: json!({}),
    }
}

fn command_call(mod_owner: &ModOwner) -> RpcParams {
    RpcParams::CommandCall {
        owner: mod_owner.clone(),
        name: "probe_command".into(),
        arguments: json!({}),
    }
}

fn cancelled_token() -> CancellationToken {
    let token = CancellationToken::new();
    token.cancel();
    token
}

#[tokio::test(start_paused = true)]
async fn cancelled_call_settles_distinct_cancel_and_original_responses() {
    let mod_owner = owner("project:cancel.ts", 1);
    let sidecar = sidecar_owner(mod_owner.id.as_str());
    let responder = cancel_aware_responder(sidecar, batch(&mod_owner, "probe"), true);
    let (host, counters) = accept(ChildPlan::new(responder), &mod_owner).await;

    let result = host
        .call(
            &mod_owner,
            RpcMethod::ToolCall,
            tool_call(&mod_owner),
            cancelled_token(),
        )
        .await;
    assert_eq!(result, Err(ModError::Cancelled));

    // Both responses arrived, so the session is still coherent and remains callable.
    let survivor = host
        .call(
            &mod_owner,
            RpcMethod::CommandCall,
            command_call(&mod_owner),
            CancellationToken::new(),
        )
        .await
        .unwrap();
    assert_eq!(survivor, RpcResult::Value { value: json!("ok") });
    assert_eq!(counters.stops.load(Ordering::SeqCst), 0);
    host.dispose().await.unwrap();
}

#[tokio::test(start_paused = true)]
async fn unsettled_cancellation_is_terminal_and_rejects_the_queue() {
    let mod_owner = owner("project:stuck.ts", 1);
    let sidecar = sidecar_owner(mod_owner.id.as_str());
    let responder = cancel_aware_responder(sidecar, batch(&mod_owner, "probe"), false);
    let (host, counters) = accept(ChildPlan::new(responder), &mod_owner).await;

    let result = host
        .call(
            &mod_owner,
            RpcMethod::ToolCall,
            tool_call(&mod_owner),
            cancelled_token(),
        )
        .await;
    assert_eq!(result, Err(ModError::Cancelled));

    let rejected = host
        .call(
            &mod_owner,
            RpcMethod::CommandCall,
            command_call(&mod_owner),
            CancellationToken::new(),
        )
        .await;
    assert_eq!(rejected, Err(ModError::Unavailable));
    assert_eq!(counters.stops.load(Ordering::SeqCst), 1);
    assert_eq!(counters.joins.load(Ordering::SeqCst), 1);
}

#[tokio::test(start_paused = true)]
async fn call_timeout_is_terminal_even_when_the_child_settles() {
    let mod_owner = owner("project:slow.ts", 1);
    let sidecar = sidecar_owner(mod_owner.id.as_str());
    let responder = cancel_aware_responder(sidecar, batch(&mod_owner, "probe"), true);
    let (host, counters) = accept(ChildPlan::new(responder), &mod_owner).await;

    let result = host
        .call(
            &mod_owner,
            RpcMethod::ToolCall,
            tool_call(&mod_owner),
            CancellationToken::new(),
        )
        .await;
    assert_eq!(result, Err(ModError::Timeout));

    let rejected = host
        .call(
            &mod_owner,
            RpcMethod::CommandCall,
            command_call(&mod_owner),
            CancellationToken::new(),
        )
        .await;
    assert_eq!(rejected, Err(ModError::Unavailable));
    assert_eq!(counters.stops.load(Ordering::SeqCst), 1);
}

#[tokio::test(start_paused = true)]
async fn malformed_response_is_terminal() {
    let mod_owner = owner("project:garbage.ts", 1);
    let sidecar = sidecar_owner(mod_owner.id.as_str());
    let responder = malformed_tool_responder(sidecar, batch(&mod_owner, "probe"));
    let (host, counters) = accept(ChildPlan::new(responder), &mod_owner).await;

    let result = host
        .call(
            &mod_owner,
            RpcMethod::ToolCall,
            tool_call(&mod_owner),
            CancellationToken::new(),
        )
        .await;
    assert_eq!(result, Err(ModError::Protocol));

    let rejected = host
        .call(
            &mod_owner,
            RpcMethod::CommandCall,
            command_call(&mod_owner),
            CancellationToken::new(),
        )
        .await;
    assert_eq!(rejected, Err(ModError::Unavailable));
    assert_eq!(counters.stops.load(Ordering::SeqCst), 1);
    assert_eq!(counters.joins.load(Ordering::SeqCst), 1);
}

#[tokio::test(flavor = "current_thread", start_paused = true)]
async fn queued_calls_are_rejected_after_a_terminal_failure() {
    let mod_owner = owner("project:queue.ts", 1);
    let sidecar = sidecar_owner(mod_owner.id.as_str());
    let responder = malformed_tool_responder(sidecar, batch(&mod_owner, "probe"));
    let (host, _counters) = accept(ChildPlan::new(responder), &mod_owner).await;

    let terminal_host = Arc::clone(&host);
    let terminal_owner = mod_owner.clone();
    let terminal = tokio::spawn(async move {
        terminal_host
            .call(
                &terminal_owner,
                RpcMethod::ToolCall,
                tool_call(&terminal_owner),
                CancellationToken::new(),
            )
            .await
    });
    tokio::task::yield_now().await;
    let queued_host = Arc::clone(&host);
    let queued_owner = mod_owner.clone();
    let queued = tokio::spawn(async move {
        queued_host
            .call(
                &queued_owner,
                RpcMethod::CommandCall,
                command_call(&queued_owner),
                CancellationToken::new(),
            )
            .await
    });
    tokio::task::yield_now().await;

    assert_eq!(terminal.await.unwrap(), Err(ModError::Protocol));
    assert_eq!(queued.await.unwrap(), Err(ModError::Unavailable));
}

#[tokio::test(start_paused = true)]
async fn inbound_capability_is_dispatched_while_settling_a_cancellation() {
    let mod_owner = owner("project:inbound.ts", 1);
    let sidecar = sidecar_owner(mod_owner.id.as_str());
    let port = RecordingPort::new();
    let brokers = Arc::new(CapabilityBrokerTable::new());
    let handle = brokers
        .mint(
            [Capability::Tools],
            mod_owner.clone(),
            scope("agent-a", "conversation-a"),
            Arc::clone(&port) as Arc<dyn CapabilityPort>,
        )
        .unwrap();
    let responder = cancel_with_inbound_responder(
        mod_owner.clone(),
        sidecar.clone(),
        batch(&mod_owner, "probe"),
        Capability::Tools,
    );
    let child = ScriptedChild::new(ChildPlan::new(responder), sidecar.clone());
    let host = FramedModHost::accept_with_brokers(
        child,
        mod_owner.clone(),
        sidecar,
        CALL_TIMEOUT,
        Arc::clone(&brokers),
    )
    .await
    .unwrap();

    host.call(
        &mod_owner,
        RpcMethod::Initialize,
        RpcParams::Initialize {
            owner: mod_owner.clone(),
            capabilities: vec![Capability::Tools],
            conversation_handle: handle,
        },
        CancellationToken::new(),
    )
    .await
    .unwrap();
    let result = host
        .call(
            &mod_owner,
            RpcMethod::ToolCall,
            tool_call(&mod_owner),
            cancelled_token(),
        )
        .await;

    assert_eq!(result, Err(ModError::Cancelled));
    let observed = port.observed();
    assert_eq!(observed.len(), 1);
    assert_eq!(observed[0].1, Capability::Tools);
    host.dispose().await.unwrap();
}

#[tokio::test(start_paused = true)]
async fn a_blocked_join_falls_back_to_the_process_group_kill_switch() {
    let mod_owner = owner("project:wedged.ts", 1);
    let sidecar = sidecar_owner(mod_owner.id.as_str());
    let responder = standard_responder(sidecar.clone(), batch(&mod_owner, "probe"), Vec::new());
    let plan = ChildPlan::new(responder).blocking_join();
    let (host, counters) = accept(plan, &mod_owner).await;

    host.dispose().await.unwrap();

    assert_eq!(counters.stops.load(Ordering::SeqCst), 1);
    assert_eq!(counters.joins.load(Ordering::SeqCst), 1);
    assert_eq!(counters.kills.load(Ordering::SeqCst), 1);
}

#[tokio::test(start_paused = true)]
async fn dropping_an_undisposed_host_kills_without_blocking() {
    let mod_owner = owner("project:dropped.ts", 1);
    let sidecar = sidecar_owner(mod_owner.id.as_str());
    let responder = standard_responder(sidecar.clone(), batch(&mod_owner, "probe"), Vec::new());
    let (host, counters) = accept(ChildPlan::new(responder), &mod_owner).await;

    drop(host);

    assert_eq!(counters.kills.load(Ordering::SeqCst), 1);
    assert_eq!(counters.stops.load(Ordering::SeqCst), 0);
}

#[tokio::test(start_paused = true)]
async fn a_disposed_host_is_not_killed_again_on_drop() {
    let mod_owner = owner("project:clean.ts", 1);
    let sidecar = sidecar_owner(mod_owner.id.as_str());
    let responder = standard_responder(sidecar.clone(), batch(&mod_owner, "probe"), Vec::new());
    let (host, counters) = accept(ChildPlan::new(responder), &mod_owner).await;

    host.dispose().await.unwrap();
    drop(host);

    assert_eq!(counters.stops.load(Ordering::SeqCst), 1);
    assert_eq!(counters.joins.load(Ordering::SeqCst), 1);
    assert_eq!(counters.kills.load(Ordering::SeqCst), 0);
}
