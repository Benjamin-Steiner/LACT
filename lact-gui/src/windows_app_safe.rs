use crate::windows_log_dir;
use adw::prelude::*;
use anyhow::{Context, anyhow};
use lact_client::DaemonClient;
use lact_schema::DeviceStats;
use relm4::{
    AsyncComponentSender,
    prelude::{AsyncComponent, AsyncComponentParts},
    tokio,
};
use std::{
    fs::OpenOptions,
    os::windows::process::CommandExt as _,
    process::{Command, Stdio},
    time::Duration,
};
use tracing::{error, info, warn};

const CREATE_NO_WINDOW: u32 = 0x0800_0000;
const REFRESH_INTERVAL: Duration = Duration::from_secs(1);

#[derive(Debug, Default)]
struct Snapshot {
    device_name: String,
    driver: String,
    temperature: String,
    utilization: String,
    vram: String,
    gpu_clock: String,
    memory_clock: String,
    extra_clocks: String,
    power: String,
    power_range: String,
    fan: String,
}

pub struct WindowsApp {
    client: Option<DaemonClient>,
    gpu_id: Option<String>,
    connected: bool,
    status: String,
    device_name: String,
    driver: String,
    temperature: String,
    utilization: String,
    vram: String,
    gpu_clock: String,
    memory_clock: String,
    extra_clocks: String,
    power: String,
    power_range: String,
    fan: String,
    log_directory: String,
}

#[derive(Debug)]
pub enum WindowsAppMsg {
    Refresh,
    OpenLogs,
}

#[relm4::component(pub, async)]
impl AsyncComponent for WindowsApp {
    type Init = lact_schema::args::GuiArgs;
    type Input = WindowsAppMsg;
    type Output = ();
    type CommandOutput = ();

    view! {
        #[root]
        adw::ApplicationWindow::builder()
            .default_width(900)
            .default_height(720)
            .title("LACT - Windows Safe Preview")
            .build() {
                #[wrap(Some)]
                set_content = &adw::ToolbarView {
                    add_top_bar = &adw::HeaderBar {
                        #[wrap(Some)]
                        set_title_widget = &adw::WindowTitle {
                            set_title: "LACT",
                            set_subtitle: "Windows / NVIDIA safe monitoring preview",
                        },

                        pack_end = &gtk::Button {
                            set_label: "Logs",
                            set_tooltip_text: Some("Open the LACT diagnostic log directory"),
                            connect_clicked => WindowsAppMsg::OpenLogs,
                        },

                        pack_end = &gtk::Button {
                            set_icon_name: "view-refresh-symbolic",
                            set_tooltip_text: Some("Refresh now"),
                            connect_clicked => WindowsAppMsg::Refresh,
                        },
                    },

                    #[wrap(Some)]
                    set_content = &gtk::ScrolledWindow {
                        set_hscrollbar_policy: gtk::PolicyType::Never,

                        #[wrap(Some)]
                        set_child = &adw::Clamp {
                            set_maximum_size: 840,

                            #[wrap(Some)]
                            set_child = &adw::PreferencesPage {
                                set_margin_top: 12,
                                set_margin_bottom: 24,

                                add = &adw::PreferencesGroup {
                                    set_title: "Safety",
                                    set_description: Some("This test build is intentionally fail-closed."),

                                    adw::ActionRow {
                                        set_title: "Windows Safe Read-Only Mode",
                                        set_subtitle: "ACTIVE - this GUI contains no GPU write controls. It only reads telemetry and driver information.",
                                        add_prefix = &gtk::Image {
                                            set_icon_name: Some("security-high-symbolic"),
                                        },
                                    },

                                    adw::ActionRow {
                                        set_title: "GPU changes",
                                        set_subtitle: "Disabled: clocks, voltage, fan, power limit, profiles and driver state are not changed by this preview.",
                                    },
                                },

                                add = &adw::PreferencesGroup {
                                    set_title: "GPU",

                                    adw::ActionRow {
                                        set_title: "Connection",
                                        #[watch]
                                        set_subtitle: &model.status,
                                        add_prefix = &gtk::Image {
                                            #[watch]
                                            set_icon_name: Some(if model.connected {
                                                "emblem-ok-symbolic"
                                            } else {
                                                "dialog-warning-symbolic"
                                            }),
                                        },
                                    },

                                    adw::ActionRow {
                                        set_title: "GPU",
                                        #[watch]
                                        set_subtitle: &model.device_name,
                                    },

                                    adw::ActionRow {
                                        set_title: "Driver",
                                        #[watch]
                                        set_subtitle: &model.driver,
                                    },
                                },

                                add = &adw::PreferencesGroup {
                                    set_title: "Live telemetry",
                                    set_description: Some("Read-only values refresh once per second through NVIDIA NVML."),

                                    adw::ActionRow {
                                        set_title: "Temperature",
                                        #[watch]
                                        set_subtitle: &model.temperature,
                                    },
                                    adw::ActionRow {
                                        set_title: "GPU utilization",
                                        #[watch]
                                        set_subtitle: &model.utilization,
                                    },
                                    adw::ActionRow {
                                        set_title: "VRAM",
                                        #[watch]
                                        set_subtitle: &model.vram,
                                    },
                                    adw::ActionRow {
                                        set_title: "GPU clock",
                                        #[watch]
                                        set_subtitle: &model.gpu_clock,
                                    },
                                    adw::ActionRow {
                                        set_title: "Memory clock",
                                        #[watch]
                                        set_subtitle: &model.memory_clock,
                                    },
                                    adw::ActionRow {
                                        set_title: "Other clocks",
                                        #[watch]
                                        set_subtitle: &model.extra_clocks,
                                    },
                                    adw::ActionRow {
                                        set_title: "Power",
                                        #[watch]
                                        set_subtitle: &model.power,
                                    },
                                    adw::ActionRow {
                                        set_title: "Power-limit range",
                                        #[watch]
                                        set_subtitle: &model.power_range,
                                    },
                                    adw::ActionRow {
                                        set_title: "Fan",
                                        #[watch]
                                        set_subtitle: &model.fan,
                                    },
                                },

                                add = &adw::PreferencesGroup {
                                    set_title: "Diagnostics",

                                    adw::ActionRow {
                                        set_title: "Log directory",
                                        #[watch]
                                        set_subtitle: &model.log_directory,
                                    },

                                    adw::ActionRow {
                                        set_title: "Files",
                                        set_subtitle: "bootstrap.log - gui.log - daemon.log",
                                    },
                                },
                            },
                        },
                    },
                },
            }
    }

    async fn init(
        _args: Self::Init,
        root: Self::Root,
        sender: AsyncComponentSender<Self>,
    ) -> AsyncComponentParts<Self> {
        let log_directory = windows_log_dir()
            .map(|path| path.display().to_string())
            .unwrap_or_else(|_| "%LOCALAPPDATA%\\LACT\\logs".to_owned());

        let mut model = Self {
            client: None,
            gpu_id: None,
            connected: false,
            status: "Starting local Windows backend...".to_owned(),
            device_name: "No GPU detected".to_owned(),
            driver: "-".to_owned(),
            temperature: "-".to_owned(),
            utilization: "-".to_owned(),
            vram: "-".to_owned(),
            gpu_clock: "-".to_owned(),
            memory_clock: "-".to_owned(),
            extra_clocks: "-".to_owned(),
            power: "-".to_owned(),
            power_range: "-".to_owned(),
            fan: "-".to_owned(),
            log_directory,
        };

        match connect_or_start_backend().await {
            Ok(client) => {
                info!("Connected to LACT Windows backend");
                model.client = Some(client.clone());
                model.connected = true;
                model.status = "Connected - Safe Read-Only Mode".to_owned();

                match client.list_devices().await {
                    Ok(devices) => {
                        if let Some(device) = devices.first() {
                            model.gpu_id = Some(device.id.clone());
                            model.device_name = device.to_string();
                            match fetch_snapshot(&client, &device.id).await {
                                Ok(snapshot) => model.apply_snapshot(snapshot),
                                Err(err) => {
                                    error!("Initial telemetry fetch failed: {err:#}");
                                    model.status = format!("Connected, telemetry error: {err:#}");
                                }
                            }
                        } else {
                            warn!("NVML returned no NVIDIA GPUs");
                            model.status = "Connected, but NVML returned no NVIDIA GPU".to_owned();
                        }
                    }
                    Err(err) => {
                        error!("GPU enumeration failed: {err:#}");
                        model.status = format!("Could not enumerate GPUs: {err:#}");
                    }
                }
            }
            Err(err) => {
                error!("Windows backend startup failed: {err:#}");
                model.status = format!("Could not start/connect to Windows backend: {err:#}");
            }
        }

        let widgets = view_output!();
        root.present();

        let input = sender.input_sender().clone();
        relm4::spawn_local(async move {
            loop {
                tokio::time::sleep(REFRESH_INTERVAL).await;
                if input.send(WindowsAppMsg::Refresh).is_err() {
                    break;
                }
            }
        });

        AsyncComponentParts { model, widgets }
    }

    async fn update(
        &mut self,
        msg: Self::Input,
        _sender: AsyncComponentSender<Self>,
        _root: &Self::Root,
    ) {
        match msg {
            WindowsAppMsg::Refresh => self.refresh().await,
            WindowsAppMsg::OpenLogs => open_logs(),
        }
    }
}

impl WindowsApp {
    async fn refresh(&mut self) {
        let (Some(client), Some(id)) = (&self.client, &self.gpu_id) else {
            return;
        };

        match fetch_snapshot(client, id).await {
            Ok(snapshot) => {
                self.connected = true;
                self.apply_snapshot(snapshot);
                self.status = "Connected - Safe Read-Only Mode".to_owned();
            }
            Err(err) => {
                warn!("Telemetry refresh failed: {err:#}");
                self.connected = false;
                self.status = format!("Telemetry refresh failed: {err:#}");
            }
        }
    }

    fn apply_snapshot(&mut self, snapshot: Snapshot) {
        self.device_name = snapshot.device_name;
        self.driver = snapshot.driver;
        self.temperature = snapshot.temperature;
        self.utilization = snapshot.utilization;
        self.vram = snapshot.vram;
        self.gpu_clock = snapshot.gpu_clock;
        self.memory_clock = snapshot.memory_clock;
        self.extra_clocks = snapshot.extra_clocks;
        self.power = snapshot.power;
        self.power_range = snapshot.power_range;
        self.fan = snapshot.fan;
    }
}

async fn connect_or_start_backend() -> anyhow::Result<DaemonClient> {
    if let Ok(client) = DaemonClient::connect_with_reconnect(false).await {
        return Ok(client);
    }

    let current_exe = std::env::current_exe().context("Could not locate LACT GUI executable")?;
    let backend_exe = current_exe
        .parent()
        .map(|parent| parent.join("lact.exe"))
        .filter(|path| path.exists())
        .context("The packaged lact.exe Windows backend was not found beside LACT-GUI.exe")?;

    info!(backend = %backend_exe.display(), "Starting bundled Windows backend");
    let mut command = Command::new(&backend_exe);
    command.arg("daemon").creation_flags(CREATE_NO_WINDOW);

    if let Ok(log_dir) = windows_log_dir()
        && let Ok(stdout) = OpenOptions::new()
            .create(true)
            .append(true)
            .open(log_dir.join("daemon.log"))
    {
        if let Ok(stderr) = stdout.try_clone() {
            command.stderr(Stdio::from(stderr));
        }
        command.stdout(Stdio::from(stdout));
    }

    command
        .spawn()
        .with_context(|| format!("Could not launch {} daemon", backend_exe.display()))?;

    let mut last_error = None;
    for _ in 0..60 {
        tokio::time::sleep(Duration::from_millis(100)).await;
        match DaemonClient::connect_with_reconnect(false).await {
            Ok(client) => return Ok(client),
            Err(err) => last_error = Some(err),
        }
    }

    Err(last_error.unwrap_or_else(|| anyhow!("Windows daemon did not become ready")))
}

async fn fetch_snapshot(client: &DaemonClient, id: &str) -> anyhow::Result<Snapshot> {
    let info = client
        .get_device_info(id, Some(false))
        .await
        .context("Could not fetch device info")?;
    let stats = client
        .get_device_stats(id)
        .await
        .context("Could not fetch device stats")?;

    let device_name = info
        .drm_info
        .as_ref()
        .and_then(|drm| drm.device_name.clone())
        .unwrap_or_else(|| id.to_owned());

    let temperature = stats
        .temps
        .values()
        .find(|entry| entry.primary)
        .or_else(|| stats.temps.values().next())
        .and_then(|entry| entry.value.current)
        .map(|value| format!("{value:.0} C"))
        .unwrap_or_else(|| "Unavailable".to_owned());

    let utilization = stats
        .busy_percent
        .map(|value| format!("{value}%"))
        .unwrap_or_else(|| "Unavailable".to_owned());

    let vram = match (stats.vram.used, stats.vram.total) {
        (Some(used), Some(total)) => format!(
            "{:.2} / {:.2} GiB ({:.0}%)",
            bytes_to_gib(used),
            bytes_to_gib(total),
            (used as f64 / total.max(1) as f64) * 100.0
        ),
        _ => "Unavailable".to_owned(),
    };

    let gpu_clock = format_clock(stats.clockspeed.gpu_clockspeed);
    let memory_clock = format_clock(stats.clockspeed.vram_clockspeed);
    let extra_clocks = if stats.clockspeed.sensors.is_empty() {
        "Unavailable".to_owned()
    } else {
        let mut values = stats
            .clockspeed
            .sensors
            .iter()
            .map(|(name, value)| format!("{name}: {value} MHz"))
            .collect::<Vec<_>>();
        values.sort();
        values.join(" | ")
    };

    let power = format_power(&stats);
    let power_range = match (stats.power.cap_min, stats.power.cap_max, stats.power.cap_current) {
        (Some(min), Some(max), Some(current)) => {
            format!("Current {current:.0} W - allowed {min:.0} to {max:.0} W")
        }
        (Some(min), Some(max), None) => format!("Allowed {min:.0} to {max:.0} W"),
        _ => "Driver power-limit range unavailable".to_owned(),
    };

    let fan = match (stats.fan.percent(), stats.fan.speed_current) {
        (Some(percent), Some(rpm)) => format!("{percent}% - {rpm} RPM"),
        (Some(percent), None) => format!("{percent}%"),
        (None, Some(rpm)) => format!("{rpm} RPM"),
        _ => "Unavailable".to_owned(),
    };

    Ok(Snapshot {
        device_name,
        driver: info.driver,
        temperature,
        utilization,
        vram,
        gpu_clock,
        memory_clock,
        extra_clocks,
        power,
        power_range,
        fan,
    })
}

fn open_logs() {
    match windows_log_dir() {
        Ok(path) => {
            info!(path = %path.display(), "Opening Windows log directory");
            if let Err(err) = Command::new("explorer.exe").arg(path).spawn() {
                error!("Could not open Windows log directory: {err}");
            }
        }
        Err(err) => error!("Could not resolve Windows log directory: {err:#}"),
    }
}

fn bytes_to_gib(bytes: u64) -> f64 {
    bytes as f64 / 1024.0 / 1024.0 / 1024.0
}

fn format_clock(value: Option<u64>) -> String {
    value
        .map(|value| format!("{value} MHz"))
        .unwrap_or_else(|| "Unavailable".to_owned())
}

fn format_power(stats: &DeviceStats) -> String {
    let draw = stats
        .power
        .current
        .or(stats.power.average)
        .map(|value| format!("{value:.1} W"))
        .unwrap_or_else(|| "Unavailable".to_owned());

    match stats.power.cap_current {
        Some(cap) => format!("{draw} draw - {cap:.0} W limit"),
        None => draw,
    }
}
