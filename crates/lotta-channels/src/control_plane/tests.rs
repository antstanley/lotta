use super::{
    Arc, BufReader, CONTROL_ARRAY_ITEMS_MAX, CONTROL_CORRELATIONS_MAX, CONTROL_FRAME_BYTES_MAX,
    CONTROL_MAP_ENTRIES_MAX, CONTROL_STRING_BYTES_MAX, CONTROL_TIMEOUT_MS_MAX, ChannelState,
    ChildFrame, ControlError, ControlPlane, FrameMetadata, ManagementCapability, ParentFrame,
    RuntimeKey, RuntimeTool, Value, parse_bounded_json, read_line,
};

fn plane() -> ControlPlane {
    let registry = Arc::new(lotta_tools::ToolRegistry::new([]).unwrap());
    let tools = Arc::new(lotta_tools::external::ChannelExternalToolManager::new(
        registry,
    ));
    ControlPlane::new("owner".into(), 1, tools).unwrap()
}

fn runtime() -> RuntimeKey {
    RuntimeKey {
        agent_id: "agent".into(),
        conversation_id: "conversation".into(),
    }
}
fn tool() -> RuntimeTool {
    RuntimeTool {
        name: "channel_send".into(),
        description: "Send a channel reply".into(),
        parameters: serde_json::json!({"type":"object"}),
    }
}

#[test]
fn publish_release_channels_registry_effects() {
    let mut plane = plane();
    let publish = ChildFrame::PublishRuntimeTools {
        metadata: FrameMetadata::new(1),
        request_id: "p1".into(),
        owner: "owner".into(),
        runtime: runtime(),
        tools: vec![tool()],
    };
    assert!(matches!(
        plane.dispatch(publish, &[]),
        Ok(Some(ParentFrame::RuntimeToolsPublished { .. }))
    ));
    assert!(plane.contains_runtime(&runtime()));
    let channels = vec![ChannelState {
        id: "telegram".into(),
        enabled: true,
        accounts: 1,
        routes: 1,
        pending_pairings: 0,
        targets: 1,
    }];
    let slash = ChildFrame::Channels {
        metadata: FrameMetadata::new(1),
        request_id: "c1".into(),
        owner: "owner".into(),
    };
    let slash_result = plane.dispatch(slash, &channels);
    assert!(matches!(
        slash_result,
        Ok(Some(ParentFrame::ChannelsResult { channels: rows, .. })) if rows == channels
    ));
    let release = ChildFrame::ReleaseRuntimeTools {
        metadata: FrameMetadata::new(1),
        request_id: "r1".into(),
        owner: "owner".into(),
        runtime: runtime(),
    };
    assert!(matches!(
        plane.dispatch(release, &[]),
        Ok(Some(ParentFrame::RuntimeToolsReleased { .. }))
    ));
    assert!(!plane.contains_runtime(&runtime()));
}

#[test]
fn validates_version_owner_capability_timeout_before_dispatch() {
    let make = |metadata, owner: &str| ChildFrame::Channels {
        metadata,
        request_id: "ordered".into(),
        owner: owner.into(),
    };
    let mut metadata = FrameMetadata::new(1);
    metadata.version += 1;
    assert!(matches!(
        plane().dispatch(make(metadata, "other"), &[]),
        Err(ControlError::Version)
    ));
    let mut metadata = FrameMetadata::new(2);
    metadata.capability = ManagementCapability::Provider;
    metadata.timeout_ms = 0;
    assert!(matches!(
        plane().dispatch(make(metadata, "other"), &[]),
        Err(ControlError::Owner)
    ));
    let mut metadata = FrameMetadata::new(1);
    metadata.capability = ManagementCapability::Provider;
    metadata.timeout_ms = 0;
    assert!(matches!(
        plane().dispatch(make(metadata, "owner"), &[]),
        Err(ControlError::Capability)
    ));
    let mut metadata = FrameMetadata::new(1);
    metadata.timeout_ms = 0;
    assert!(matches!(
        plane().dispatch(make(metadata, "owner"), &[]),
        Err(ControlError::Timeout)
    ));
    let mut metadata = FrameMetadata::new(1);
    metadata.timeout_ms = CONTROL_TIMEOUT_MS_MAX + 1;
    assert!(matches!(
        plane().dispatch(make(metadata, "owner"), &[]),
        Err(ControlError::Timeout)
    ));
}

#[tokio::test]
async fn bootstrap_ndjson_round_trip() {
    let bytes = concat!(
        "{\"kind\":\"bootstrap\",\"version\":1,\"generation\":1,",
        "\"capability\":\"channel_management\",\"timeout_ms\":10000,",
        "\"request_id\":\"x\",\"owner\":\"o\",",
        "\"websocket_url\":\"ws://127.0.0.1:9/ws\",\"token\":\"t\",",
        "\"channels_root\":\"/tmp\"}\n"
    )
    .as_bytes();
    let mut reader = BufReader::new(bytes);
    assert!(matches!(
        read_line::<_, ParentFrame>(&mut reader).await,
        Ok(Some(ParentFrame::Bootstrap { .. }))
    ));
}

#[tokio::test]
async fn rejects_partial_multiple_oversize_malformed_unknown_and_replay() {
    let duplicate_owner = concat!(
        "{\"kind\":\"channels\",\"request_id\":\"a\",\"owner\":\"owner\",",
        "\"owner\":\"owner\"}\n"
    );
    let nested_duplicate = concat!(
        "{\"kind\":\"channels\",\"request_id\":\"a\",\"owner\":\"owner\",",
        "\"nested\":{\"x\":1,\"x\":2}}\n"
    );
    for bytes in [
        b"{}".as_slice(),
        b"{} {}\n",
        b"{}\n{}\n",
        b"not-json\n",
        b"{\"kind\":\"unknown\"}\n",
        duplicate_owner.as_bytes(),
        nested_duplicate.as_bytes(),
        b"{\"kind\":\"channels\",\"request_id\":\"a\",\"owner\":\"\xff\"}\n",
    ] {
        let mut reader = BufReader::new(bytes);
        assert!(read_line::<_, ChildFrame>(&mut reader).await.is_err());
    }
    let mut plane = plane();
    let frame = ChildFrame::Channels {
        metadata: FrameMetadata::new(1),
        request_id: "same".into(),
        owner: "owner".into(),
    };
    assert!(plane.dispatch(frame.clone(), &[]).is_ok());
    assert!(matches!(
        plane.dispatch(frame, &[]),
        Err(ControlError::Correlation)
    ));
}

#[test]
fn live_correlations_enforce_capacity_direction_deadline_and_replay_independently() {
    let mut plane = plane();
    for index in 0..CONTROL_CORRELATIONS_MAX {
        plane
            .register_parent_request(&format!("parent-{index}"), 10, 100)
            .unwrap();
    }
    assert_eq!(plane.in_flight_len(), CONTROL_CORRELATIONS_MAX);
    assert_eq!(
        plane.register_parent_request("above", 10, 100),
        Err(ControlError::Correlation)
    );

    let terminal = |id: &str, owner: &str| ChildFrame::Ready {
        metadata: FrameMetadata::new(1),
        correlation_id: id.into(),
        owner: owner.into(),
        pid: 7,
        channels_probe_success: true,
        parent_sibling_probe_denied: true,
    };
    assert!(
        plane
            .dispatch_at(terminal("parent-0", "owner"), &[], 110)
            .is_ok()
    );
    assert_eq!(plane.in_flight_len(), CONTROL_CORRELATIONS_MAX - 1);
    assert!(matches!(
        plane.dispatch_at(terminal("parent-1", "owner"), &[], 111),
        Err(ControlError::Timeout)
    ));
    assert!(matches!(
        plane.dispatch_at(terminal("parent-0", "owner"), &[], 109),
        Err(ControlError::Correlation)
    ));
    assert_eq!(
        plane.register_parent_request("parent-0", 10, 100),
        Err(ControlError::Correlation),
        "replay retention is independent from live completion"
    );
}

#[test]
fn child_request_completes_only_with_exact_parent_response() {
    let mut plane = plane();
    let request = ChildFrame::Channels {
        metadata: FrameMetadata {
            timeout_ms: 5,
            ..FrameMetadata::new(1)
        },
        request_id: "child-live".into(),
        owner: "owner".into(),
    };
    let response = plane.dispatch_at(request, &[], 50).unwrap().unwrap();
    assert_eq!(plane.in_flight_len(), 1);
    assert_eq!(plane.complete_parent_response_at(&response, 55), Ok(()));
    assert_eq!(plane.in_flight_len(), 0);
    assert_eq!(
        plane.complete_parent_response_at(&response, 55),
        Err(ControlError::Correlation)
    );

    let late = ChildFrame::Channels {
        metadata: FrameMetadata {
            timeout_ms: 5,
            ..FrameMetadata::new(1)
        },
        request_id: "child-late".into(),
        owner: "owner".into(),
    };
    let response = plane.dispatch_at(late, &[], 50).unwrap().unwrap();
    assert_eq!(
        plane.complete_parent_response_at(&response, 56),
        Err(ControlError::Timeout)
    );
}

#[tokio::test]
async fn exact_ndjson_structure_frame_and_order_boundaries() {
    for length in [CONTROL_STRING_BYTES_MAX - 1, CONTROL_STRING_BYTES_MAX] {
        let text = serde_json::to_string(&"x".repeat(length)).unwrap();
        assert!(parse_bounded_json(&text).is_ok());
    }
    let text = serde_json::to_string(&"x".repeat(CONTROL_STRING_BYTES_MAX + 1)).unwrap();
    assert_eq!(parse_bounded_json(&text), Err(ControlError::Bound));

    for length in [CONTROL_ARRAY_ITEMS_MAX - 1, CONTROL_ARRAY_ITEMS_MAX] {
        let text = serde_json::to_string(&vec![0; length]).unwrap();
        assert!(parse_bounded_json(&text).is_ok());
    }
    let text = serde_json::to_string(&vec![0; CONTROL_ARRAY_ITEMS_MAX + 1]).unwrap();
    assert_eq!(parse_bounded_json(&text), Err(ControlError::Bound));

    for length in [CONTROL_MAP_ENTRIES_MAX - 1, CONTROL_MAP_ENTRIES_MAX] {
        let map = (0..length)
            .map(|index| (format!("k{index}"), 0))
            .collect::<std::collections::BTreeMap<_, _>>();
        assert!(parse_bounded_json(&serde_json::to_string(&map).unwrap()).is_ok());
    }
    let map = (0..=CONTROL_MAP_ENTRIES_MAX)
        .map(|index| (format!("k{index}"), 0))
        .collect::<std::collections::BTreeMap<_, _>>();
    assert_eq!(
        parse_bounded_json(&serde_json::to_string(&map).unwrap()),
        Err(ControlError::Bound)
    );

    for length in [CONTROL_FRAME_BYTES_MAX - 1, CONTROL_FRAME_BYTES_MAX] {
        let mut line = b"{}".to_vec();
        line.resize(length, b' ');
        line.push(b'\n');
        let mut reader = BufReader::new(line.as_slice());
        assert!(read_line::<_, Value>(&mut reader).await.is_ok());
    }
    let mut above = b"{}".to_vec();
    above.resize(CONTROL_FRAME_BYTES_MAX + 1, b' ');
    above.push(b'\n');
    let mut reader = BufReader::new(above.as_slice());
    assert_eq!(
        read_line::<_, Value>(&mut reader).await,
        Err(ControlError::Bound)
    );

    let first = serde_json::to_string(&ChildFrame::Channels {
        metadata: FrameMetadata::new(1),
        request_id: "first".into(),
        owner: "owner".into(),
    })
    .unwrap();
    let ordered = format!("{first}\n{}\n", first.replace("first", "second"));
    let mut reader = BufReader::new(ordered.as_bytes());
    for expected in ["first", "second"] {
        let Some(ChildFrame::Channels { request_id, .. }) =
            read_line::<_, ChildFrame>(&mut reader).await.unwrap()
        else {
            panic!("ordered channel frame");
        };
        assert_eq!(request_id, expected);
    }
    assert!(
        read_line::<_, ChildFrame>(&mut reader)
            .await
            .unwrap()
            .is_none()
    );
}
