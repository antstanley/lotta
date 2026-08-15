use crate::config::ServerArgs;
use std::net::TcpListener;

#[test]
fn non_loopback_requires_auth() {
    let reservation = TcpListener::bind("127.0.0.1:0").unwrap_or_else(|_| panic!("reserve"));
    let port = reservation
        .local_addr()
        .unwrap_or_else(|_| panic!("address"))
        .port();
    drop(reservation);
    let args = ServerArgs {
        listen_enabled: true,
        listen: Some(format!("ws://0.0.0.0:{port}")),
        ..ServerArgs::default()
    };
    assert!(args.prepare().is_err());
    let rebound = TcpListener::bind(("127.0.0.1", port));
    assert!(rebound.is_ok());
}
