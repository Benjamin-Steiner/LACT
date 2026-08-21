#[cfg(unix)]
mod app;
#[cfg(unix)]
mod config;
#[cfg(unix)]
mod service_setup;
#[cfg(windows)]
mod windows_app;

use anyhow::Context;
use lact_schema::args::GuiArgs;
use relm4::RelmApp;
use tracing::metadata::LevelFilter;
use tracing_subscriber::EnvFilter;

#[cfg(unix)]
use std::{
    panic,
    sync::{LazyLock, atomic::AtomicBool, atomic::Ordering},
};

#[cfg(unix)]
use app::{APP_BROKER, AppModel, msg::AppMsg};
#[cfg(unix)]
use config::UiConfig;
#[cfg(unix)]
use i18n_embed::fluent::{FluentLanguageLoader, fluent_language_loader};
#[cfg(unix)]
use lact_schema::i18n;
#[cfg(unix)]
use relm4::{
    SharedState,
    gtk::{glib, glib::MainContext},
};
#[cfg(unix)]
use rust_embed::RustEmbed;

#[cfg(unix)]
static CONFIG: SharedState<UiConfig> = SharedState::new();
#[cfg(unix)]
static PANICKED: AtomicBool = AtomicBool::new(false);

#[cfg(unix)]
const GUI_VERSION: &str = env!("CARGO_PKG_VERSION");
pub const APP_ID: &str = "io.github.ilya_zlobintsev.LACT";
#[cfg(unix)]
pub const REPO_URL: &str = "https://github.com/ilya-zlobintsev/LACT";

#[cfg(unix)]
pub(crate) static I18N: LazyLock<FluentLanguageLoader> = LazyLock::new(|| {
    i18n::loader(
        fluent_language_loader!(),
        &Localizations,
        cfg!(test).then(|| vec!["en-US".parse().unwrap()]),
    )
});

#[cfg(unix)]
#[derive(RustEmbed)]
#[folder = "i18n"]
pub struct Localizations;

fn log_filter(args: &GuiArgs) -> anyhow::Result<EnvFilter> {
    EnvFilter::builder()
        .with_default_directive(LevelFilter::INFO.into())
        .parse(args.log_level.as_deref().unwrap_or_default())
        .context("Invalid log level")
}

#[cfg(unix)]
fn init_logging(args: &GuiArgs) -> anyhow::Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(log_filter(args)?)
        .init();
    Ok(())
}

#[cfg(windows)]
fn init_logging(args: &GuiArgs) -> anyhow::Result<()> {
    use std::{fs::OpenOptions, sync::Mutex};

    let log_dir = windows_log_dir()?;
    let log_file = OpenOptions::new()
        .create(true)
        .append(true)
        .open(log_dir.join("gui.log"))
        .context("Could not open the LACT Windows GUI log")?;

    tracing_subscriber::fmt()
        .with_env_filter(log_filter(args)?)
        .with_ansi(false)
        .with_writer(Mutex::new(log_file))
        .init();
    tracing::info!("LACT Windows GUI logging initialized");
    tracing::info!(log_directory = %log_dir.display(), "Windows diagnostic log directory");
    Ok(())
}

#[cfg(windows)]
pub(crate) fn windows_log_dir() -> anyhow::Result<std::path::PathBuf> {
    let base = std::env::var_os("LOCALAPPDATA")
        .map(std::path::PathBuf::from)
        .unwrap_or_else(std::env::temp_dir);
    let dir = base.join("LACT").join("logs");
    std::fs::create_dir_all(&dir).context("Could not create the LACT Windows log directory")?;
    Ok(dir)
}

#[cfg(unix)]
pub fn run(args: GuiArgs) -> anyhow::Result<()> {
    init_logging(&args)?;

    let old_hook = panic::take_hook();
    panic::set_hook(Box::new(move |info| {
        old_hook(info);

        if PANICKED.swap(true, Ordering::SeqCst) {
            return;
        }

        let panic_msg = if let Some(msg) = info.payload().downcast_ref::<&str>() {
            msg.to_string()
        } else if let Some(msg) = info.payload().downcast_ref::<String>() {
            msg.clone()
        } else {
            "Unknown panic".to_string()
        };

        let location = info
            .location()
            .map(|l| format!("{}:{}:{}", l.file(), l.line(), l.column()))
            .unwrap_or_else(|| "unknown location".to_string());

        let full_msg = format!("Application panicked at {location}:\n{panic_msg}");

        let main_context = MainContext::default();
        if main_context.is_owner() {
            APP_BROKER.send(AppMsg::Crash(full_msg));
            let loop_ = glib::MainLoop::new(Some(&main_context), false);
            glib::idle_add_local_once(move || {
                loop_.run();
            });
        } else {
            main_context.invoke_with_priority(glib::Priority::HIGH, move || {
                APP_BROKER.send(AppMsg::Crash(full_msg));
            });
        }
    }));

    LazyLock::force(&I18N);
    LazyLock::force(&lact_schema::i18n::LANGUAGE_LOADER);

    if let Some(existing_config) = UiConfig::load() {
        *CONFIG.write() = existing_config;
    }

    RelmApp::new(APP_ID)
        .with_broker(&APP_BROKER)
        .with_args(vec![])
        .run_async::<AppModel>(args);
    Ok(())
}

#[cfg(windows)]
pub fn run(args: GuiArgs) -> anyhow::Result<()> {
    init_logging(&args)?;
    RelmApp::new(APP_ID)
        .with_args(vec![])
        .run_async::<windows_app::WindowsApp>(args);
    Ok(())
}
