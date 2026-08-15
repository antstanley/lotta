//! Lotta composition root.
//!
//! This binary parses startup configuration, composes the concrete clock and listener, and owns
//! process lifecycle. Business logic belongs in workspace library crates.

#![forbid(unsafe_code)]
#![deny(missing_docs)]

use std::{io::Write, sync::Arc};

use lotta_app_server::{config::parse_cli, listener::start_listener};
use lotta_domain::{Clock, DomainError, Timestamp};

struct SystemClock;

impl Clock for SystemClock {
    fn now(&self) -> Timestamp {
        Timestamp::from_utc(chrono::Utc::now())
    }

    fn parse_timestamp(&self, value: &str) -> Result<Timestamp, DomainError> {
        Timestamp::parse_persisted_rfc3339(value)
    }
}

#[tokio::main]
async fn main() {
    if let Err(error) = run().await {
        eprintln!("{}: {error}", error.code());
        std::process::exit(2);
    }
}

async fn run() -> Result<(), lotta_app_server::error::AppServerError> {
    let args = parse_cli(std::env::args().skip(1))?;
    let prepared = args.prepare()?;
    let mut handle = start_listener(prepared, Arc::new(SystemClock)).await?;
    println!("Base URL: {}", handle.base_url());
    println!("WebSocket URL: {}", handle.websocket_url());
    if let Some(url) = handle.openai_url() {
        println!("OpenAI URL: {url}");
    }
    std::io::stdout()
        .flush()
        .map_err(|_| lotta_app_server::error::AppServerError::Listener)?;
    tokio::signal::ctrl_c()
        .await
        .map_err(|_| lotta_app_server::error::AppServerError::Listener)?;
    handle.shutdown();
    handle.wait().await
}
