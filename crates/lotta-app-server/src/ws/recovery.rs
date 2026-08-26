//! Authenticated reconnect recovery behavior.
//!
//! The stable identity is supplied only after upgrade authentication. A live
//! identity owns its lease exclusively; transient close suspends its ordinal,
//! subscriptions, and last emitted sequence for one later same-identity open.

#[cfg(test)]
mod reconnect {
    use crate::ws::{RuntimeEvent, RuntimeRouter, connection::ReconnectIdentity, test_support::*};

    fn resumed() -> (RuntimeRouter, crate::ws::ConnectionId) {
        let clock = std::sync::Arc::new(TestClock::new());
        let ids = std::sync::Arc::new(TestIds::new());
        let mut router = RuntimeRouter::new(clock, ids);
        let identity = ReconnectIdentity {
            listener_instance: "listener".into(),
            principal: "principal".into(),
            client_id: "device".into(),
        };
        let original = router
            .connections
            .open_authenticated(Some(&identity))
            .expect("open");
        router.connections.initialize(original).expect("initialize");
        router
            .connections
            .subscribe(original, scope(1))
            .expect("subscribe");
        router.connections.set_event_seq(original, 4);
        router.connections.suspend(original);
        let resumed = router
            .connections
            .open_authenticated(Some(&identity))
            .expect("resume");
        router.connections.initialize(resumed).expect("initialize");
        (router, resumed)
    }

    #[test]
    fn restores_subscriptions() {
        let (router, connection) = resumed();
        assert_eq!(router.connections.subscriptions_of(connection), [scope(1)]);
    }

    #[test]
    fn sequence_continues() {
        let (mut router, connection) = resumed();
        let delivery = router
            .broadcast(&scope(1), &status_event("idle"))
            .expect("broadcast");
        assert_eq!(delivery.as_slice()[0].connection_id, connection);
        assert_eq!(delivery.as_slice()[0].frame.event_seq, 5);
    }

    #[test]
    fn replays_pending_approvals() {
        let (mut router, connection) = resumed();
        let event = RuntimeEvent::ControlRequest {
            request_id: text("approval-1"),
            request: approval_request(),
            agent_id: None,
            conversation_id: None,
        };
        let delivery = router
            .broadcast(&scope(1), &event)
            .expect("approval replay");
        assert_eq!(delivery.as_slice()[0].connection_id, connection);
        assert_eq!(
            delivery.as_slice()[0].frame.event.discriminant(),
            "control_request"
        );
    }

    #[test]
    fn rejects_live_identity_takeover() {
        let (router, _, _, _) = router();
        let identity = ReconnectIdentity {
            listener_instance: "listener".into(),
            principal: "principal".into(),
            client_id: "device".into(),
        };
        let mut locked = router.lock().expect("router");
        locked.connections.close(1);
        locked
            .connections
            .open_authenticated(Some(&identity))
            .expect("authenticated open");
        let error = locked
            .connections
            .open_authenticated(Some(&identity))
            .expect_err("live identity cannot be replaced");
        assert!(matches!(error, crate::error::AppServerError::Forbidden));
    }

    #[test]
    fn deterministic_clock_expires_suspended_lease_at_ttl() {
        use chrono::Duration;
        use lotta_domain::{Clock, Timestamp};
        use lotta_testkit::clock::FakeClock;
        let start = Timestamp::parse_persisted_rfc3339("2026-08-14T12:00:00Z")
            .unwrap_or_else(|error| panic!("timestamp: {error}"));
        let clock = std::sync::Arc::new(FakeClock::new(start));
        let mut connections = crate::ws::connection::RuntimeConnections::with_clock(
            clock.clone() as std::sync::Arc<dyn Clock + Send + Sync>
        );
        let identity = ReconnectIdentity {
            listener_instance: "listener".into(),
            principal: "principal".into(),
            client_id: "device".into(),
        };
        let original = connections
            .open_authenticated(Some(&identity))
            .unwrap_or_else(|error| panic!("open: {error}"));
        connections.set_event_seq(original, 7);
        connections.suspend(original);
        clock
            .advance(Duration::seconds(
                crate::ws::connection::SUSPENDED_CONNECTION_TTL_SECONDS,
            ))
            .unwrap_or_else(|error| panic!("advance: {error}"));
        let renewed = connections
            .open_authenticated(Some(&identity))
            .unwrap_or_else(|error| panic!("renew: {error}"));
        assert_eq!(connections.event_seq(renewed), Some(0));
    }
}

#[cfg(test)]
mod repairs_missing_tool_end {
    use crate::ws::{RuntimeEvent, RuntimeRouter, test_support::*};
    #[test]
    fn next_loop_snapshot_repairs_client_tool_state() {
        let (router, _, _, connection) = router();
        let mut router: std::sync::MutexGuard<'_, RuntimeRouter> = router.lock().expect("router");
        router
            .connections
            .subscribe(connection, scope(1))
            .expect("subscribe");
        let start = RuntimeEvent::StreamDelta {
            delta: crate::ws::event::StreamDelta::ClientToolStart(
                crate::ws::event::ClientToolStart {
                    id: text("start-1"),
                    date: text("2026-08-14T00:00:00Z"),
                    message_type: crate::ws::event::ClientToolStartType::ClientToolStart,
                    run_id: None,
                    tool_call_id: text("tool-1"),
                    tool_name: Some(text("shell")),
                    tool_args: Some("{}".into()),
                },
            ),
            subagent_id: None,
        };
        let started = router.broadcast(&scope(1), &start).expect("tool start");
        assert_eq!(started.as_slice()[0].frame.event_seq, 1);

        let repair = RuntimeEvent::UpdateLoopStatus {
            loop_status: loop_state(crate::ws::event::LoopStatus::WaitingOnInput),
        };
        let repaired = router
            .broadcast(&scope(1), &repair)
            .expect("repair snapshot");
        assert_eq!(repaired.as_slice()[0].frame.event_seq, 2);
        let RuntimeEvent::UpdateLoopStatus { loop_status } = &repaired.as_slice()[0].frame.event
        else {
            panic!("loop snapshot");
        };
        assert_eq!(
            loop_status.status,
            crate::ws::event::LoopStatus::WaitingOnInput
        );
        assert!(loop_status.executing_tool_call_ids.is_empty());
    }
}
