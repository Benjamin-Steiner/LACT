#![cfg_attr(windows, windows_subsystem = "windows")]

#[cfg(windows)]
fn main() -> anyhow::Result<()> {
    let bootstrap_log = bootstrap_log_path()?;
    install_panic_logger(bootstrap_log.clone());
    append_log(&bootstrap_log, "LACT Windows GUI bootstrap starting");

    let result = (|| {
        prepare_portable_runtime()?;
        append_log(&bootstrap_log, "Portable GTK runtime environment prepared");
        lact_gui::run(lact_schema::args::GuiArgs::default())
    })();

    match &result {
        Ok(()) => append_log(&bootstrap_log, "LACT Windows GUI exited normally"),
        Err(err) => {
            append_log(&bootstrap_log, &format!("LACT Windows GUI failed: {err:#}"));
            // This binary uses the Windows GUI subsystem, so there is no console
            // to show an early startup error. Open the bootstrap log immediately.
            let _ = std::process::Command::new("notepad.exe")
                .arg(&bootstrap_log)
                .spawn();
        }
    }

    result
}

#[cfg(windows)]
fn prepare_portable_runtime() -> anyhow::Result<()> {
    use anyhow::Context;
    use std::{env, fs, path::Path, process::Command};

    let exe = env::current_exe().context("Could not locate the LACT GUI executable")?;
    let root = exe
        .parent()
        .context("Could not determine the LACT GUI directory")?;

    set_path_env_if_exists(
        "GSETTINGS_SCHEMA_DIR",
        &root.join("share").join("glib-2.0").join("schemas"),
    );
    set_path_env_if_exists("XDG_DATA_DIRS", &root.join("share"));
    set_path_env_if_exists("FONTCONFIG_PATH", &root.join("etc").join("fonts"));

    let pixbuf_root = root
        .join("lib")
        .join("gdk-pixbuf-2.0")
        .join("2.10.0");
    let loaders_dir = pixbuf_root.join("loaders");
    let loaders_cache = pixbuf_root.join("loaders.cache");
    let query_loaders = root.join("gdk-pixbuf-query-loaders.exe");

    if query_loaders.exists() && loaders_dir.exists() {
        if let Ok(output) = Command::new(&query_loaders)
            .env("GDK_PIXBUF_MODULEDIR", &loaders_dir)
            .output()
            && output.status.success()
            && !output.stdout.is_empty()
        {
            let _ = fs::write(&loaders_cache, output.stdout);
        }
    }

    set_path_env_if_exists("GDK_PIXBUF_MODULE_FILE", &loaders_cache);

    Ok(())
}

#[cfg(windows)]
fn bootstrap_log_path() -> anyhow::Result<std::path::PathBuf> {
    use anyhow::Context;

    let base = std::env::var_os("LOCALAPPDATA")
        .map(std::path::PathBuf::from)
        .unwrap_or_else(std::env::temp_dir);
    let dir = base.join("LACT").join("logs");
    std::fs::create_dir_all(&dir).context("Could not create the LACT log directory")?;
    Ok(dir.join("bootstrap.log"))
}

#[cfg(windows)]
fn append_log(path: &std::path::Path, message: &str) {
    use std::io::Write as _;
    use std::time::{SystemTime, UNIX_EPOCH};

    let timestamp = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_secs())
        .unwrap_or_default();

    if let Ok(mut file) = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(path)
    {
        let _ = writeln!(file, "[{timestamp}] {message}");
    }
}

#[cfg(windows)]
fn install_panic_logger(path: std::path::PathBuf) {
    let previous = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        append_log(&path, &format!("PANIC: {info}"));
        previous(info);
    }));
}

#[cfg(windows)]
fn set_path_env_if_exists(key: &str, path: &std::path::Path) {
    if path.exists() {
        // This runs before GTK/Relm4 starts worker threads, so mutating the
        // process environment here is safe for the portable Windows bundle.
        unsafe {
            std::env::set_var(key, path);
        }
    }
}

#[cfg(not(windows))]
fn main() {}
