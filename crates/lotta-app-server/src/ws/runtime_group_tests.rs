use super::service::{
    AbortOutcome, DeviceStateOutcome, InputAdmission, RuntimeStartOutcome, ServiceFuture,
    SyncOutcome,
};
use super::test_support::*;
use super::*;
use serde_json::Value;
use serde_json::json;
use std::sync::{Arc, Mutex, atomic::Ordering};
use tokio::sync::Notify;
use tokio::time::{Duration, timeout};

struct BlockingService {
    entered: Arc<Notify>,
    release: Arc<Notify>,
}

impl RuntimeCommandService for BlockingService {
    fn runtime_start(
        &self,
        _: command::RuntimeStartCommand,
    ) -> ServiceFuture<'_, RuntimeStartOutcome> {
        let entered = self.entered.clone();
        let release = self.release.clone();
        Box::pin(async move {
            entered.notify_one();
            release.notified().await;
            Ok(RuntimeStartOutcome {
                runtime: scope(1),
                created_agent: false,
                created_conversation: false,
                agent: None,
                conversation: None,
                broadcasts: events(Vec::new()),
            })
        })
    }
    fn admit_input(&self, _: command::InputCommand) -> ServiceFuture<'_, InputAdmission> {
        panic!("unused")
    }
    fn continue_input(
        &self,
        _: lotta_domain::RuntimeScope,
        _: Option<lotta_domain::BoundedJsonValue>,
        _: Arc<dyn RuntimeEventSink>,
    ) -> ServiceFuture<'_, ()> {
        panic!("unused")
    }
    fn sync(&self, _: command::SyncCommand) -> ServiceFuture<'_, SyncOutcome> {
        panic!("unused")
    }
    fn abort_message(&self, _: command::AbortMessageCommand) -> ServiceFuture<'_, AbortOutcome> {
        panic!("unused")
    }
    fn change_device_state(
        &self,
        _: command::ChangeDeviceStateCommand,
    ) -> ServiceFuture<'_, DeviceStateOutcome> {
        panic!("unused")
    }
}

#[tokio::test]
async fn runtime_start_service_await_does_not_hold_router_lock() {
    let (router, _, _, id) = router();
    let entered = Arc::new(Notify::new());
    let release = Arc::new(Notify::new());
    let service = Arc::new(BlockingService {
        entered: entered.clone(),
        release: release.clone(),
    });
    let command =
        decode_wire(&json!({"type":"runtime_start","request_id":"blocked","agent_id":"a"}));
    let route = tokio::spawn(route_command(router.clone(), service, id, command));
    entered.notified().await;
    let other_router = router.clone();
    timeout(
        Duration::from_millis(100),
        tokio::task::spawn_blocking(move || {
            let mut router = lock_router(&other_router).unwrap_or_else(|e| panic!("lock: {e}"));
            let second = router
                .connections
                .open()
                .unwrap_or_else(|e| panic!("open: {e}"));
            router
                .connections
                .initialize(second)
                .unwrap_or_else(|e| panic!("init: {e}"));
        }),
    )
    .await
    .unwrap_or_else(|_| panic!("router lock held across service await"))
    .unwrap_or_else(|e| panic!("lock task: {e}"));
    release.notify_one();
    route
        .await
        .unwrap_or_else(|e| panic!("join: {e}"))
        .unwrap_or_else(|e| panic!("route: {e}"));
}

#[tokio::test]
async fn runtime_start_routes_response_subscribe_initial() {
    let (router, _, _, id) = router();
    let service = Arc::new(RecordingService::default());
    let command = decode_wire(&json!({"type":"runtime_start","request_id":"r1","agent_id":"a"}));
    let (output, _) = route_command(router.clone(), service, id, command)
        .await
        .unwrap_or_else(|e| panic!("route: {e}"));
    assert_eq!(response_json(&output)["type"], "runtime_start_response");
    assert_eq!(output.deliveries.len(), 1);
    assert_eq!(
        lock_router(&router)
            .unwrap_or_else(|e| panic!("lock: {e}"))
            .connections
            .subscription_count(id),
        Some(1)
    );
}
#[tokio::test]
async fn input_routes_ack() {
    let (router, _, _, id) = router();
    lock_router(&router)
        .unwrap_or_else(|e| panic!("lock: {e}"))
        .connections
        .subscribe(id, scope(1))
        .unwrap_or_else(|e| panic!("subscribe: {e}"));
    let command = decode_wire(
        &json!({"type":"input","request_id":"r2","runtime":scope(1),"payload":{"kind":"create_message","messages":[]}}),
    );
    let (output, deferred) =
        route_command(router, Arc::new(RecordingService::default()), id, command)
            .await
            .unwrap_or_else(|e| panic!("route: {e}"));
    assert_eq!(response_json(&output)["disposition"], "started");
    assert!(deferred.is_some());
}
#[tokio::test]
async fn sync_preserves_flags_and_routes() {
    let (router, _, _, id) = router();
    let command = decode_wire(
        &json!({"type":"sync","request_id":"r3","runtime":scope(1),"recover_approvals":false,"force_device_status":true}),
    );
    let RuntimeCommand::Sync(command) = &command else {
        panic!("sync")
    };
    assert_eq!(command.recover_approvals, Some(false));
    assert_eq!(command.force_device_status, Some(true));
    let (output, _) = route_command(
        router,
        Arc::new(RecordingService::default()),
        id,
        command.clone().into(),
    )
    .await
    .unwrap_or_else(|e| panic!("route: {e}"));
    assert_eq!(response_json(&output)["success"], true);
}
#[tokio::test]
async fn abort_optional_null_run_id_routes() {
    let (router, _, _, id) = router();
    let command = decode_wire(
        &json!({"type":"abort_message","request_id":"r4","runtime":scope(1),"run_id":null}),
    );
    let (output, _) = route_command(router, Arc::new(RecordingService::default()), id, command)
        .await
        .unwrap_or_else(|e| panic!("route: {e}"));
    assert_eq!(response_json(&output)["aborted"], true);
}
#[tokio::test]
async fn change_device_payload_routes_without_response() {
    let (router, _, _, id) = router();
    lock_router(&router)
        .unwrap_or_else(|e| panic!("lock: {e}"))
        .connections
        .subscribe(id, scope(1))
        .unwrap_or_else(|e| panic!("subscribe: {e}"));
    let command = decode_wire(
        &json!({"type":"change_device_state","runtime":scope(1),"payload":{"mode":"strict","cwd":"/tmp","agent_id":null}}),
    );
    let (output, _) = route_command(router, Arc::new(RecordingService::default()), id, command)
        .await
        .unwrap_or_else(|e| panic!("route: {e}"));
    assert!(output.responses.is_empty());
    assert_eq!(output.deliveries.len(), 1);
}
async fn conflict(value: Value) {
    let (router, _, _, id) = router();
    let service = Arc::new(RecordingService::default());
    let command = decode_wire(&value);
    assert!(
        route_command(router, service.clone(), id, command)
            .await
            .is_err()
    );
    assert_eq!(service.calls.load(Ordering::SeqCst), 0);
    assert_eq!(service.writes.load(Ordering::SeqCst), 0);
}
#[tokio::test]
async fn rejects_agent_conflict_before_allocation() {
    conflict(
        json!({"type":"runtime_start","request_id":"r","agent_id":"a","create_agent":{"body":{}}}),
    )
    .await;
}
#[tokio::test]
async fn rejects_conversation_conflict_before_allocation() {
    conflict(json!({"type":"runtime_start","request_id":"r","conversation_id":"c","create_conversation":{}})).await;
}
#[tokio::test]
async fn malformed_runtime_start_shapes_never_call_service() {
    for value in [
        json!({"type":"runtime_start","request_id":"r","create_agent":7}),
        json!({"type":"runtime_start","request_id":"r","create_agent":{}}),
        json!({"type":"runtime_start","request_id":"r","workspace_sandbox":{"root":"/x"}}),
        json!({"type":"runtime_start","request_id":"r","client_info":{"title":"x"}}),
        json!({"type":"runtime_start","request_id":"r","skill_sources":["bogus"]}),
        json!({"type":"runtime_start","request_id":"r","external_tools":{}}),
    ] {
        let (router, _, _, id) = router();
        let service = Arc::new(RecordingService::default());
        let frame = crate::framing::decode_text(&value.to_string())
            .unwrap_or_else(|error| panic!("frame: {error:?}"));
        let decoded = command::decode(&frame);
        match decoded {
            Ok(Some(command)) => assert!(
                route_command(router, service.clone(), id, command)
                    .await
                    .is_err()
            ),
            Err(_) => {}
            Ok(None) => panic!("runtime command"),
        }
        assert_eq!(service.calls.load(Ordering::SeqCst), 0);
    }
}

#[tokio::test]
async fn input_without_request_has_no_ack() {
    let (router, _, _, id) = router();
    let command = decode_wire(
        &json!({"type":"input","runtime":scope(1),"payload":{"kind":"create_message","messages":[]}}),
    );
    let (output, _) = route_command(router, Arc::new(RecordingService::default()), id, command)
        .await
        .unwrap_or_else(|e| panic!("route: {e}"));
    assert!(output.responses.is_empty());
}
#[tokio::test]
async fn input_ack_physically_precedes_after_ack_and_continue() {
    let (router, _, _, id) = router();
    lock_router(&router)
        .unwrap_or_else(|e| panic!("lock: {e}"))
        .connections
        .subscribe(id, scope(1))
        .unwrap_or_else(|e| panic!("sub: {e}"));
    let service = Arc::new(RecordingService::default());
    let command = decode_wire(
        &json!({"type":"input","request_id":"r","runtime":scope(1),"payload":{"kind":"create_message","messages":[]}}),
    );
    let (output, deferred) = route_command(router.clone(), service.clone(), id, command)
        .await
        .unwrap_or_else(|e| panic!("route: {e}"));
    let frames = Arc::new(Mutex::new(Vec::<String>::new()));
    if let Ok(mut values) = frames.lock() {
        values.push(
            response_json(&output)["type"]
                .as_str()
                .unwrap_or("?")
                .into(),
        );
        for d in output.deliveries.as_slice() {
            values.push(d.frame.event.discriminant().into());
        }
    }
    let output_frames = frames.clone();
    let sink = Arc::new(RouterEventSink::new(
        router,
        Arc::new(move |d| {
            if let Ok(mut values) = output_frames.lock() {
                for item in d.as_slice() {
                    values.push(item.frame.event.discriminant().into());
                }
            }
            Ok(())
        }),
    ));
    let deferred = deferred.unwrap_or_else(|| panic!("deferred"));
    service
        .continue_input(deferred.scope, deferred.continuation, sink)
        .await
        .unwrap_or_else(|e| panic!("continue: {e}"));
    assert_eq!(
        *frames.lock().unwrap_or_else(|e| panic!("frames: {e}")),
        vec!["input_accepted", "update_queue", "stream_delta"]
    );
    assert_eq!(
        *service
            .stages
            .lock()
            .unwrap_or_else(|e| panic!("stages: {e}")),
        vec!["admit", "continue"]
    );
}
