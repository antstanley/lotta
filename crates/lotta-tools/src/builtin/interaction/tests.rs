use super::*;
use serde_json::json;

#[tokio::test]
async fn approval_suspends_and_resumes_exactly() {
    let (port, mut receiver) = InteractionPort::new();
    let request_port = Arc::clone(&port);
    let task = tokio::spawn(async move {
        request_port
            .request_approval(
                "call_1".into(),
                "Write".into(),
                json!({"file_path":"a"}),
                CancellationToken::new(),
                Duration::from_secs(1),
            )
            .await
    });
    tokio::task::yield_now().await;
    assert!(!task.is_finished());
    let request = receiver.recv().await.unwrap();
    let InteractionRequest::Approval(request) = request else {
        panic!("approval expected")
    };
    assert_eq!(request.tool_call_id, "call_1");
    assert_eq!(request.tool_name, "Write");
    port.respond(
        &request.correlation_id,
        InteractionResponse::Approval(ApprovalDecision::Allow),
    )
    .await
    .unwrap();
    assert_eq!(task.await.unwrap(), Ok(ApprovalDecision::Allow));
    assert_eq!(
        port.respond(
            &request.correlation_id,
            InteractionResponse::Approval(ApprovalDecision::Deny)
        )
        .await,
        Err(InteractionError::Unknown)
    );
}

#[tokio::test]
async fn ask_user_suspends_and_resumes_exactly() {
    let (port, mut receiver) = InteractionPort::new();
    let questions = vec![Question {
        question: "Choose?".into(),
        header: "Choice".into(),
        options: vec![
            QuestionOption {
                label: "A".into(),
                description: "first".into(),
            },
            QuestionOption {
                label: "B".into(),
                description: "second".into(),
            },
        ],
        multi_select: false,
    }];
    let ask_port = Arc::clone(&port);
    let cloned = questions.clone();
    let task = tokio::spawn(async move {
        ask_port
            .request(
                PendingKind::AskUser,
                |correlation_id| {
                    InteractionRequest::AskUser(AskUserRequest {
                        correlation_id,
                        questions: cloned,
                    })
                },
                CancellationToken::new(),
                Duration::from_secs(1),
            )
            .await
    });
    tokio::task::yield_now().await;
    assert!(!task.is_finished());
    let InteractionRequest::AskUser(request) = receiver.recv().await.unwrap() else {
        panic!("ask expected")
    };
    assert_eq!(request.questions, questions);
    port.respond(
        &request.correlation_id,
        InteractionResponse::Answers(Map::from_iter([(
            "Choose?".into(),
            Value::String("A".into()),
        )])),
    )
    .await
    .unwrap();
    assert!(matches!(
        task.await.unwrap(),
        Ok(InteractionResponse::Answers(_))
    ));
}

#[test]
fn malformed_unknown_missing_and_free_form_answers() {
    let questions = vec![question("Choose?"), question("Why?")];
    for answers in [
        Map::new(),
        Map::from_iter([("Choose?".into(), Value::String("A".into()))]),
        Map::from_iter([
            ("Choose?".into(), Value::String("A".into())),
            ("Unknown?".into(), Value::String("B".into())),
        ]),
        Map::from_iter([
            ("Choose?".into(), Value::String("A".into())),
            ("Why?".into(), Value::Number(1.into())),
        ]),
    ] {
        assert!(map_ask_response(&questions, Ok(InteractionResponse::Answers(answers))).is_err());
    }
    let free_form = Map::from_iter([
        (
            "Choose?".into(),
            Value::String("Other: custom value".into()),
        ),
        ("Why?".into(), Value::String("because it works".into())),
    ]);
    assert!(matches!(
        map_ask_response(&questions, Ok(InteractionResponse::Answers(free_form))),
        Ok(RawToolOutcome::Success(_))
    ));
}

#[tokio::test]
async fn malformed_response_does_not_consume_pending_correlation() {
    let (port, mut receiver) = InteractionPort::new();
    let request_port = Arc::clone(&port);
    let task = tokio::spawn(async move {
        request_port
            .request_approval(
                "call_1".into(),
                "Write".into(),
                json!({}),
                CancellationToken::new(),
                Duration::from_secs(1),
            )
            .await
    });
    let InteractionRequest::Approval(request) = receiver.recv().await.unwrap() else {
        panic!("approval expected")
    };
    assert_eq!(
        port.respond(
            &request.correlation_id,
            InteractionResponse::Answers(Map::from_iter([(
                "unexpected".into(),
                Value::String("answer".into())
            )]))
        )
        .await,
        Err(InteractionError::Invalid)
    );
    port.respond(
        &request.correlation_id,
        InteractionResponse::Approval(ApprovalDecision::Deny),
    )
    .await
    .unwrap();
    assert_eq!(task.await.unwrap(), Ok(ApprovalDecision::Deny));
}

#[tokio::test]
async fn cancellation_timeout_and_channel_close_are_distinct() {
    let (cancel_port, cancel_receiver) = InteractionPort::new();
    let token = CancellationToken::new();
    token.cancel();
    assert_eq!(
        cancel_port
            .request(
                PendingKind::AskUser,
                |id| InteractionRequest::AskUser(AskUserRequest {
                    correlation_id: id,
                    questions: Vec::new()
                }),
                token,
                Duration::from_secs(1)
            )
            .await,
        Err(InteractionError::Cancelled)
    );
    drop(cancel_receiver);
    let (timeout_port, timeout_receiver) = InteractionPort::new();
    assert_eq!(
        timeout_port
            .request(
                PendingKind::AskUser,
                |id| InteractionRequest::AskUser(AskUserRequest {
                    correlation_id: id,
                    questions: Vec::new()
                }),
                CancellationToken::new(),
                Duration::ZERO
            )
            .await,
        Err(InteractionError::Timeout)
    );
    drop(timeout_receiver);
    let (closed_port, closed_receiver) = InteractionPort::new();
    drop(closed_receiver);
    assert_eq!(
        closed_port
            .request(
                PendingKind::AskUser,
                |id| InteractionRequest::AskUser(AskUserRequest {
                    correlation_id: id,
                    questions: Vec::new()
                }),
                CancellationToken::new(),
                Duration::from_secs(1)
            )
            .await,
        Err(InteractionError::Closed)
    );
}

fn question(text: &str) -> Question {
    Question {
        question: text.into(),
        header: "Header".into(),
        options: vec![
            QuestionOption {
                label: "A".into(),
                description: "first".into(),
            },
            QuestionOption {
                label: "B".into(),
                description: "second".into(),
            },
        ],
        multi_select: false,
    }
}

#[test]
fn question_bounds_below_at_above() {
    assert_eq!(INTERACTION_QUESTIONS_ITEMS_MAX, 4);
    assert_eq!(INTERACTION_OPTIONS_ITEMS_MIN, 2);
    assert_eq!(INTERACTION_OPTIONS_ITEMS_MAX, 4);
}
