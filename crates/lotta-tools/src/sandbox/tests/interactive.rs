use super::*;

#[cfg(target_os = "macos")]
#[tokio::test]
async fn real_pty_observes_tty_and_echoes_stdin() {
    let fixture = Fixture::new("interactive-pty");
    if fixture.sandbox.backend() == &SandboxBackend::Unsupported {
        return;
    }
    let command = concat!(
        "test -t 0 && test -t 1 && read value && ",
        "printf 'ECHO:%s\\n' \"$value\""
    );
    let request = request(
        &fixture,
        "/bin/sh",
        &["-c", command],
        None,
        4096,
        Duration::from_secs(5),
    );
    let session = fixture
        .sandbox
        .start_session(request, CancellationToken::new())
        .await
        .unwrap();
    let Ok((input, mut events, _, terminal)) = session.into_parts() else {
        panic!("session transfer failed");
    };
    input
        .send(ProcessInput::Bytes(
            ProcessStdin::new(b"pony\n".to_vec()).unwrap(),
        ))
        .await
        .unwrap();
    drop(input);
    let collect = async move {
        let mut output = Vec::new();
        while let Some(event) = events.recv().await {
            match event {
                ProcessEvent::Stdout(chunk) | ProcessEvent::Stderr(chunk) => {
                    output.extend_from_slice(chunk.as_slice());
                }
            }
        }
        output
    };
    let (outcome, output) = tokio::join!(terminal, collect);
    let outcome = outcome.unwrap().unwrap();
    assert_eq!(outcome.exit_code, Some(0));
    assert!(String::from_utf8_lossy(&output).contains("ECHO:pony"));
}
