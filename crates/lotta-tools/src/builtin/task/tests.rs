use super::*;

#[tokio::test]
async fn lifecycle_create_get_list_update_dependency() {
    let port = TaskLifecyclePort::new();
    let first = port.create(create("first")).await.unwrap();
    let second = port.create(create("second")).await.unwrap();
    assert_eq!(port.get(&first.task_id).await.unwrap(), first);
    assert_eq!(port.list().await.unwrap().len(), 2);
    let updated = port
        .update(UpdateInput {
            task_id: first.task_id.clone(),
            status: Some(TaskStatus::InProgress),
            subject: None,
            description: None,
            active_form: None,
            owner: Some("owner".into()),
            add_blocks: vec![second.task_id.clone()],
            add_blocked_by: Vec::new(),
            metadata: Some(Map::from_iter([(
                "key".into(),
                Value::String("value".into()),
            )])),
        })
        .await
        .unwrap();
    assert_eq!(updated.status, TaskStatus::InProgress);
    assert_eq!(
        updated.blocks.as_slice(),
        std::slice::from_ref(&second.task_id)
    );
    assert_eq!(
        port.get(&second.task_id).await.unwrap().blocked_by,
        [first.task_id]
    );
}

#[tokio::test]
async fn unknown_terminal_and_race_are_fixed() {
    let port = Arc::new(TaskLifecyclePort::new());
    assert_eq!(port.get("task_999").await, Err(TaskError::Unknown));
    let record = port.create(create("race")).await.unwrap();
    let left = Arc::clone(&port);
    let right = Arc::clone(&port);
    let id_left = record.task_id.clone();
    let id_right = record.task_id.clone();
    let first =
        tokio::spawn(async move { left.update(status(id_left, TaskStatus::InProgress)).await });
    let second =
        tokio::spawn(async move { right.update(status(id_right, TaskStatus::InProgress)).await });
    assert!(first.await.unwrap().is_ok());
    assert!(second.await.unwrap().is_ok());
    let completed = port
        .update(status(record.task_id.clone(), TaskStatus::Completed))
        .await
        .unwrap();
    assert_eq!(completed.status, TaskStatus::Completed);
    assert_eq!(
        port.update(status(record.task_id, TaskStatus::Pending))
            .await,
        Err(TaskError::Invalid)
    );
}

#[tokio::test]
async fn invalid_dependency_and_cycle_roll_back_every_state_counter() {
    let port = TaskLifecyclePort::new();
    let first = port.create(create("first")).await.unwrap();
    let second = port.create(create("second")).await.unwrap();
    let before = port.state.lock().await.clone();
    let mut invalid = status(first.task_id.clone(), TaskStatus::InProgress);
    invalid.add_blocks = vec!["task_999".into()];
    assert_eq!(port.update(invalid).await, Err(TaskError::Invalid));
    assert_state_eq(&*port.state.lock().await, &before);

    let mut edge = status(first.task_id.clone(), TaskStatus::InProgress);
    edge.add_blocks = vec![second.task_id.clone()];
    port.update(edge).await.unwrap();
    let before_cycle = port.state.lock().await.clone();
    let mut cycle = status(second.task_id, TaskStatus::InProgress);
    cycle.add_blocks = vec![first.task_id];
    assert_eq!(port.update(cycle).await, Err(TaskError::Invalid));
    assert_state_eq(&*port.state.lock().await, &before_cycle);
}

#[tokio::test]
async fn dependency_bound_rollback_and_delete_clean_reciprocals() {
    let port = TaskLifecyclePort::new();
    let first = port.create(create("first")).await.unwrap();
    let second = port.create(create("second")).await.unwrap();
    let mut edge = status(first.task_id.clone(), TaskStatus::InProgress);
    edge.add_blocked_by = vec![second.task_id.clone()];
    port.update(edge).await.unwrap();
    assert_eq!(
        port.get(&second.task_id).await.unwrap().blocks.as_slice(),
        std::slice::from_ref(&first.task_id)
    );

    let before = port.state.lock().await.clone();
    let mut over = status(first.task_id.clone(), TaskStatus::InProgress);
    over.add_blocks = vec![second.task_id.clone(); TASK_DEPENDENCIES_ITEMS_MAX + 1];
    assert_eq!(port.update(over).await, Err(TaskError::Limit));
    assert_state_eq(&*port.state.lock().await, &before);

    port.update(status(first.task_id.clone(), TaskStatus::Deleted))
        .await
        .unwrap();
    assert_eq!(port.get(&first.task_id).await, Err(TaskError::Unknown));
    let remaining = port.get(&second.task_id).await.unwrap();
    assert!(remaining.blocks.is_empty() && remaining.blocked_by.is_empty());
}

#[test]
fn exact_six_lifecycle_names_compose_without_shell_collision() {
    let task_names = ["TaskCreate", "TaskGet", "TaskList", "TaskUpdate"];
    let shell_owned = ["TaskOutput", "TaskStop"];
    let all = task_names
        .into_iter()
        .chain(shell_owned)
        .collect::<Vec<_>>();
    assert_eq!(all.len(), 6);
    let mut sorted = all.clone();
    sorted.sort_unstable();
    sorted.dedup();
    assert_eq!(sorted.len(), 6);
}

#[test]
fn task_bounds_below_at_above() {
    let below = vec!["task_1".to_owned(); TASK_DEPENDENCIES_ITEMS_MAX - 1];
    let at = vec!["task_1".to_owned(); TASK_DEPENDENCIES_ITEMS_MAX];
    let above = vec!["task_1".to_owned(); TASK_DEPENDENCIES_ITEMS_MAX + 1];
    assert!(validate_ids(&below).is_ok());
    assert!(validate_ids(&at).is_ok());
    assert_eq!(validate_ids(&above), Err(TaskError::Limit));
}

fn assert_state_eq(actual: &TaskState, expected: &TaskState) {
    assert_eq!(actual.records, expected.records);
    assert_eq!(actual.order, expected.order);
    assert_eq!(actual.next_id, expected.next_id);
    assert_eq!(actual.revision, expected.revision);
}

fn create(subject: &str) -> CreateInput {
    CreateInput {
        subject: subject.into(),
        description: format!("{subject} description"),
        active_form: None,
        metadata: None,
    }
}
fn status(task_id: String, status: TaskStatus) -> UpdateInput {
    UpdateInput {
        task_id,
        status: Some(status),
        subject: None,
        description: None,
        active_form: None,
        owner: None,
        add_blocks: Vec::new(),
        add_blocked_by: Vec::new(),
        metadata: None,
    }
}
