#![warn(clippy::pedantic)]
#![allow(clippy::missing_panics_doc)]

#[cfg(unix)]
mod bindings;
#[cfg(unix)]
mod config;
#[cfg(unix)]
mod server;
#[cfg(unix)]
mod socket;
#[cfg(unix)]
mod suspend;
#[cfg(unix)]
mod system;
#[cfg(all(test, unix))]
mod tests;
#[cfg(windows)]
mod windows;

#[cfg(unix)]
use anyhow::Context;
#[cfg(unix)]
use config::Config;
#[cfg(unix)]
use futures::future::select_all;
#[cfg(unix)]
use server::{Server, handle_stream, handler::Handler};
#[cfg(unix)]
use std::os::unix::net::UnixStream as StdUnixStream;
#[cfg(unix)]
use std::sync::Arc;
#[cfg(unix)]
use std::time::Duration;
#[cfg(unix)]
use tokio::net::UnixStream;
#[cfg(unix)]
use tokio::runtime::LocalOptions;
#[cfg(unix)]
use tokio::sync::Notify;
#[cfg(unix)]
use tokio::{
    runtime,
    signal::unix::{SignalKind, signal},
};
#[cfg(unix)]
use tracing::level_filters::LevelFilter;
#[cfg(unix)]
use tracing::{Instrument, debug_span, error, info, warn};
#[cfg(unix)]
use tracing_subscriber::EnvFilter;

#[cfg(unix)]
use crate::server::ClientContext;
#[cfg(unix)]
pub use system::BASE_MODULE_CONF_PATH;

#[cfg(unix)]
const DRM_EVENT_COLLECT_DURATION: Duration = Duration::from_millis(500);

#[cfg(unix)]
const SHUTDOWN_SIGNALS: [SignalKind; 4] = [
    SignalKind::terminate(),
    SignalKind::interrupt(),
    SignalKind::quit(),
    SignalKind::hangup(),
];

/// Run the daemon using the native local transport for the current platform.
///
/// # Errors
/// Returns an error when the daemon cannot initialize.
#[cfg(unix)]
pub fn run() -> anyhow::Result<()> {
    let rt = runtime::Builder::new_current_thread()
        .enable_all()
        .build_local(LocalOptions::default())
        .expect("Could not initialize tokio runtime");
    rt.block_on(async {
        let config = Config::load_or_create()?;

        let env_filter = EnvFilter::builder()
            .with_default_directive(LevelFilter::INFO.into())
            .parse(&config.daemon.log_level)
            .context("Invalid log level")?;
        tracing_subscriber::fmt().with_env_filter(env_filter).init();

        let server = Server::new(config).await?;
        let handler = server.handler.clone();

        tokio::task::spawn_local(listen_config_changes(handler.clone()));
        tokio::task::spawn_local(listen_exit_signals(handler.clone()));
        tokio::task::spawn_local(listen_device_events(handler.clone()));
        tokio::task::spawn_local(suspend::listen_events(handler));

        server.run().await;
        Ok(())
    })
}

#[cfg(windows)]
pub fn run() -> anyhow::Result<()> {
    windows::run()
}

/// Run the daemon with a given `UnixStream`.
/// This will NOT bind to a socket by itself, and the daemon will only be accessible via the given stream.
///
/// # Errors
/// Returns an error when the daemon cannot initialize.
#[cfg(unix)]
pub fn run_embedded(stream: StdUnixStream) -> anyhow::Result<()> {
    let rt = runtime::Builder::new_current_thread()
        .enable_all()
        .build_local(LocalOptions::default())
        .expect("Could not initialize tokio runtime");
    rt.block_on(async {
        let config = Config::default();
        let handler = Handler::new(config).await?;
        let stream = UnixStream::try_from(stream)?;
        let ctx = ClientContext::default();

        handle_stream(stream, handler, ctx).await
    })
}

#[cfg(unix)]
async fn listen_exit_signals(handler: Handler) {
    let mut signals = SHUTDOWN_SIGNALS
        .map(|signal_kind| signal(signal_kind).expect("Could not listen to shutdown signal"));
    let signal_futures = signals.iter_mut().map(|signal| Box::pin(signal.recv()));
    select_all(signal_futures).await;

    info!("cleaning up and shutting down...");
    async {
        handler.cleanup().await;
        socket::cleanup();
    }
    .instrument(debug_span!("shutdown_cleanup"))
    .await;
    std::process::exit(0);
}

#[cfg(unix)]
async fn listen_config_changes(handler: Handler) {
    let mut rx = config::start_watcher(handler.config_last_saved.clone());
    while let Some(new_config) = rx.recv().await {
        info!("config file was changed, reloading");
        *handler.config.write().await = new_config;
        match handler.apply_current_config().await {
            Ok(()) => {
                info!("configuration reloaded");
            }
            Err(err) => {
                error!("could not apply new config: {err:#}");
            }
        }
    }
}

#[cfg(unix)]
async fn listen_device_events(handler: Handler) {
    let notify = Arc::new(Notify::new());
    let task_notify = notify.clone();
    tokio::task::spawn_blocking(move || {
        if let Err(err) = system::listen_netlink_kernel_event(&task_notify) {
            error!("kernel event listener error: {err:#}");
        }
    });

    loop {
        notify.notified().await;

        info!("got kernel drm subsystem event, queueing GPU reload");
        handler.notify_reload_gpus(DRM_EVENT_COLLECT_DURATION).await;
    }
}
