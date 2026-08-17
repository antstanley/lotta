use super::snapshot::*;

fn snapshot(bytes: usize) -> SubagentSnapshot {
    SubagentSnapshot {
        task_id: 1,
        owner_agent_id: "owner".into(),
        state: TaskState::Running,
        task_count: 1,
        active_task_count: 1,
        events: if bytes == 0 {
            Vec::new()
        } else {
            vec![StreamEvent {
                task_id: 1,
                sequence: 1,
                bytes: vec![b'x'; bytes],
            }]
        },
    }
}

#[test]
fn snapshot_is_bounded_below() {
    let bytes = serialize_snapshot(&snapshot(32)).unwrap();
    assert!(bytes.len() < SUBAGENT_SNAPSHOT_BYTES_MAX);
}

#[test]
fn snapshot_is_bounded_at_structural_event_limit() {
    let mut value = snapshot(1);
    value.events = (1..=SUBAGENT_SNAPSHOT_EVENTS_MAX)
        .map(|sequence| StreamEvent {
            task_id: 1,
            sequence: sequence as u64,
            bytes: vec![b'x'],
        })
        .collect();
    assert!(serialize_snapshot(&value).is_ok());
}

#[test]
fn snapshot_is_bounded_above() {
    let mut value = snapshot(1);
    value.events = (1..=SUBAGENT_SNAPSHOT_EVENTS_MAX + 1)
        .map(|sequence| StreamEvent {
            task_id: 1,
            sequence: sequence as u64,
            bytes: vec![b'x'],
        })
        .collect();
    assert_eq!(serialize_snapshot(&value), Err(StatusError::Bound));
}

#[test]
fn stream_event_order_and_bounds_are_real() {
    let events = [
        StreamEvent {
            task_id: 2,
            sequence: 1,
            bytes: b"one".to_vec(),
        },
        StreamEvent {
            task_id: 2,
            sequence: 2,
            bytes: b"two".to_vec(),
        },
    ];
    let wire: Vec<Vec<u8>> = events
        .iter()
        .map(|event| serialize_event(event).unwrap())
        .collect();
    let decoded: Vec<StreamEvent> = wire
        .iter()
        .map(|bytes| serde_json::from_slice(bytes).unwrap())
        .collect();
    assert_eq!(decoded, events);
    assert_eq!(
        serialize_event(&StreamEvent {
            task_id: 2,
            sequence: 3,
            bytes: vec![b'x'; SUBAGENT_STREAM_EVENT_BYTES_MAX + 1],
        }),
        Err(StatusError::Bound)
    );
}

#[tokio::test]
async fn filled_channel_blocks_release_resumes_and_cancellation_releases() {
    let (port, mut receiver) = ChannelStatusPort::channel();
    for index in 0..SUBAGENT_STATUS_QUEUE_ITEMS_MAX {
        port.update_subagent_state(vec![u8::try_from(index).unwrap()])
            .await
            .unwrap();
    }
    let blocked = port.update_subagent_state(b"terminal".to_vec());
    tokio::pin!(blocked);
    assert!(
        tokio::time::timeout(std::time::Duration::from_millis(20), &mut blocked)
            .await
            .is_err()
    );
    assert_eq!(receiver.recv().await.unwrap(), vec![0]);
    blocked.await.unwrap();
    for expected in 1..SUBAGENT_STATUS_QUEUE_ITEMS_MAX {
        assert_eq!(
            receiver.recv().await.unwrap(),
            vec![u8::try_from(expected).unwrap()]
        );
    }
    assert_eq!(receiver.recv().await.unwrap(), b"terminal");

    for _ in 0..SUBAGENT_STATUS_QUEUE_ITEMS_MAX {
        port.update_subagent_state(vec![1]).await.unwrap();
    }
    let cancelled = port.update_subagent_state(vec![2]);
    assert!(
        tokio::time::timeout(std::time::Duration::from_millis(20), cancelled)
            .await
            .is_err()
    );
    assert_eq!(receiver.recv().await.unwrap(), vec![1]);
    assert!(
        tokio::time::timeout(
            std::time::Duration::from_millis(100),
            port.update_subagent_state(vec![3]),
        )
        .await
        .unwrap()
        .is_ok()
    );
}
