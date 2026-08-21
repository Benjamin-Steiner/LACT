#![cfg_attr(windows, windows_subsystem = "windows")]

#[cfg(windows)]
fn main() -> anyhow::Result<()> {
    prepare_portable_runtime()?;
    lact_gui::run(lact_schema::args::GuiArgs::default())
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
