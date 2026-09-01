//! Lotta composition root.
//!
//! This binary parses startup configuration, composes the concrete clock and listener, and owns
//! process lifecycle. Business logic belongs in workspace library crates.

#![forbid(unsafe_code)]
#![deny(missing_docs)]

use std::{io::Write, sync::Arc};

use lotta_app_server::{
    config::{ServerArgs, parse_cli},
    listener::start_listener_with_runtime_service_controller_observer_and_bridges,
    observer::InertRuntimeBroadcastObserver,
};
use lotta_domain::{Clock, DomainError, Timestamp};
use production_components::ProductionComponents;

mod cli;
mod production_components;
/// Concrete production turn-setup composition used by application controllers.
pub mod production_setup;

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

async fn run() -> Result<(), cli::CliError> {
    let arguments = std::env::args().skip(1).collect::<Vec<_>>();
    if arguments.as_slice() == ["__channel-host"] {
        return lotta_channels::host::run().await.map_err(Into::into);
    }
    match cli::parse(arguments)? {
        cli::Command::Server(arguments) => run_server(arguments).await,
        cli::Command::Migrate {
            storage_dir,
            dry_run,
        } => cli::migrate(storage_dir, dry_run),
        cli::Command::Verify { storage_dir } => cli::verify(storage_dir),
    }
}

async fn run_server(arguments: Vec<String>) -> Result<(), cli::CliError> {
    let prepared = parse_cli(arguments)?.prepare()?;
    let storage_dir = prepared.storage_dir.clone();
    let workspace_dir = prepared.workspace_dir.clone();
    let clock: Arc<dyn Clock + Send + Sync> = Arc::new(SystemClock);
    let components = ProductionComponents::from_server(&prepared, Arc::clone(&clock))?;
    let mut handle = start_listener_with_runtime_service_controller_observer_and_bridges(
        prepared,
        Arc::clone(&clock),
        components.runtime_service(),
        components.turn_controller(),
        Arc::new(InertRuntimeBroadcastObserver),
        components.shared_bridges(),
    )
    .await?;
    println!("Base URL: {}", handle.base_url());
    println!("WebSocket URL: {}", handle.websocket_url());
    if let Some(url) = handle.openai_url() {
        println!("OpenAI URL: {url}");
    }
    let channel_store = lotta_channels::topology::ChannelStore::from_parent_environment()?;
    let channels = if lotta_channels::topology::channels_enabled(&channel_store)? {
        Some(
            start_channels(
                &components,
                Arc::clone(&clock),
                storage_dir,
                workspace_dir,
                channel_store,
            )
            .await?,
        )
    } else {
        None
    };
    std::io::stdout()
        .flush()
        .map_err(|_| lotta_app_server::error::AppServerError::Listener)?;
    tokio::signal::ctrl_c()
        .await
        .map_err(|_| lotta_app_server::error::AppServerError::Listener)?;
    if let Some((supervisor, mut listener)) = channels {
        supervisor.shutdown().await?;
        listener.shutdown();
        listener.wait().await?;
    }
    handle.shutdown();
    handle.wait().await?;
    Ok(())
}

async fn start_channels(
    components: &ProductionComponents,
    clock: Arc<dyn Clock + Send + Sync>,
    storage_dir: std::path::PathBuf,
    workspace_dir: std::path::PathBuf,
    store: lotta_channels::topology::ChannelStore,
) -> Result<
    (
        lotta_channels::supervisor::ChannelSupervisor,
        lotta_app_server::listener::ListenerHandle,
    ),
    cli::CliError,
> {
    let owner_prefix = format!("channel-host-{}", std::process::id());
    let authenticator = lotta_app_server::auth::channel_session::ChannelSessionAuthenticator::new();
    let args = ServerArgs {
        listen: Some("ws://127.0.0.1:0/channel-runtime".into()),
        listen_enabled: true,
        storage_dir: Some(storage_dir),
        workspace_dir: Some(workspace_dir),
        ..ServerArgs::default()
    };
    let channel_prepared = args.prepare()?.for_channel_session(authenticator.clone())?;
    let listener = start_listener_with_runtime_service_controller_observer_and_bridges(
        channel_prepared,
        clock,
        components.runtime_service(),
        components.turn_controller(),
        Arc::new(InertRuntimeBroadcastObserver),
        components.shared_bridges(),
    )
    .await?;
    let executable = channel_executable()?;
    let tools = components.channel_tools();
    let config = lotta_channels::supervisor::ChannelLaunchConfig {
        executable,
        store,
        websocket_url: listener.websocket_url().to_owned(),
        owner_prefix,
        authenticator,
        tools,
    };
    match lotta_channels::supervisor::ChannelSupervisor::start(config).await {
        Ok(supervisor) => {
            println!(
                "Channel host PID: {}",
                supervisor.pid().map_or(0, |pid| pid)
            );
            Ok((supervisor, listener))
        }
        Err(error) => {
            let mut listener = listener;
            listener.shutdown();
            let _ = listener.wait().await;
            Err(error.into())
        }
    }
}

fn channel_executable() -> Result<std::path::PathBuf, cli::CliError> {
    if let Some(path) = std::env::var_os(lotta_channels::topology::CHANNEL_HOST_EXECUTABLE_ENV) {
        let path = std::path::PathBuf::from(path);
        if path.is_absolute() {
            return Ok(path);
        }
        return Err(cli::CliError::ChannelExecutable);
    }
    std::env::current_exe().map_err(|_| cli::CliError::ChannelExecutable)
}
