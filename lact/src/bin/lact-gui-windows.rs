#![cfg_attr(windows, windows_subsystem = "windows")]

#[cfg(windows)]
fn main() -> anyhow::Result<()> {
    lact_gui::run(lact_schema::args::GuiArgs::default())
}

#[cfg(not(windows))]
fn main() {}
