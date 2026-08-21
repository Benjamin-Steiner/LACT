use adw::prelude::*;
use anyhow::{Context, anyhow};
use lact_client::DaemonClient;
use lact_schema::{
    ClocksTable, DeviceStats,
    args::GuiArgs,
    request::{ClockspeedType, SetClocksCommand},
};
use relm4::{
    AsyncComponentSender, RelmWidgetExt,
    prelude::{AsyncComponent, AsyncComponentParts},
    tokio,
};
use std::{
    os::windows::process::CommandExt as _,
    process::Command,
    time::Duration,
};

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
    power_limit: Option<f64>,
    fan: String,
    gpu_pstate: Option<u32>,
    gpu_offset: Option<i32>,
    gpu_offset_range: String,
    mem_pstate: Option<u32>,
    mem_offset: Option<i32>,
    mem_offset_range: String,
}

pub struct WindowsApp {
    client: Option<DaemonClient>,
    gpu_id: Option<String>,
    connected: bool,
    controls_initialized: bool,

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

    power_limit_input: f64,
    gpu_offset_input: f64,
    mem_offset_input: f64,
    gpu_pstate: Option<u32>,
    mem_pstate: Option<u32>,
    gpu_offset_range: String,
    mem_offset_range: String,
}

#[derive(Debug)]
pub enum WindowsAppMsg {
    Refresh,
    PowerDraft(f64),
    GpuOffsetDraft(f64),
    MemOffsetDraft(f64),
    ApplyPower,
    ResetPower,
    ApplyGpuOffset,
    ApplyMemOffset,
    ResetClocks,
}

#[relm4::component(pub, async)]
impl AsyncComponent for WindowsApp {
    type Init = GuiArgs;
    type Input = WindowsAppMsg;
    type Output = ();
    type CommandOutput = ();

    view! {
        #[root]
        adw::ApplicationWindow::builder()
            .default_width(880)
            .default_height(720)
            .title("LACT - Windows Preview")
            .build() {
                #[wrap(Some)]
                set_content = &adw::ToolbarView {
                    add_top_bar = &adw::HeaderBar {
                        #[wrap(Some)]
                        set_title_widget = &adw::WindowTitle {
                            set_title: "LACT",
                            set_subtitle: "Windows / NVIDIA preview",
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
                            set_maximum_size: 820,

                            #[wrap(Some)]
                            set_child = &adw::PreferencesPage {
                                set_margin_top: 12,
                                set_margin_bottom: 24,

                                add = &adw::PreferencesGroup {
                                    set_title: "Connection",

                                    adw::ActionRow {
                                        set_title: "Backend",
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
                                    set_description: Some("Updated every second through the local LACT named-pipe daemon."),

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
                                        set_title: "Fan",
                                        #[watch]
                                        set_subtitle: &model.fan,
                                    },
                                },

                                add = &adw::PreferencesGroup {
                                    set_title: "NVIDIA controls",
                                    set_description: Some("These controls use the capabilities exposed by the installed NVIDIA driver/NVML. Run LACT as Administrator if the driver rejects a write."),

                                    adw::ActionRow {
                                        set_title: "Power limit",
                                        #[watch]
                                        set_subtitle: &model.power_range,

                                        add_suffix = &gtk::SpinButton {
                                            set_range: (1.0, 1000.0),
                                            set_increments: (1.0, 10.0),
                                            set_digits: 0,
                                            set_numeric: true,
                                            set_width_request: 110,
                                            set_value: model.power_limit_input,
                                            #[watch]
                                            set_sensitive: model.connected,
                                            connect_value_changed[sender] => move |spin| {
                                                sender.input(WindowsAppMsg::PowerDraft(spin.value()));
                                            },
                                        },

                                        add_suffix = &gtk::Button {
                                            set_label: "Apply",
                                            #[watch]
                                            set_sensitive: model.connected,
                                            connect_clicked => WindowsAppMsg::ApplyPower,
                                        },

                                        add_suffix = &gtk::Button {
                                            set_label: "Default",
                                            #[watch]
                                            set_sensitive: model.connected,
                                            connect_clicked => WindowsAppMsg::ResetPower,
                                        },
                                    },

                                    adw::ActionRow {
                                        set_title: "GPU clock offset",
                                        #[watch]
                                        set_subtitle: &model.gpu_offset_range,

                                        add_suffix = &gtk::SpinButton {
                                            set_range: (-5000.0, 5000.0),
                                            set_increments: (5.0, 25.0),
                                            set_digits: 0,
                                            set_numeric: true,
                                            set_width_request: 110,
                                            set_value: model.gpu_offset_input,
                                            #[watch]
                                            set_sensitive: model.connected && model.gpu_pstate.is_some(),
                                            connect_value_changed[sender] => move |spin| {
                                                sender.input(WindowsAppMsg::GpuOffsetDraft(spin.value()));
                                            },
                                        },

                                        add_suffix = &gtk::Button {
                                            set_label: "Apply",
                                            #[watch]
                                            set_sensitive: model.connected && model.gpu_pstate.is_some(),
                                            connect_clicked => WindowsAppMsg::ApplyGpuOffset,
                                        },
                                    },

                                    adw::ActionRow {
                                        set_title: "Memory clock offset",
                                        #[watch]
                                        set_subtitle: &model.mem_offset_range,

                                        add_suffix = &gtk::SpinButton {
                                            set_range: (-5000.0, 5000.0),
                                            set_increments: (5.0, 25.0),
                                            set_digits: 0,
                                            set_numeric: true,
                                            set_width_request: 110,
                                            set_value: model.mem_offset_input,
                                            #[watch]
                                            set_sensitive: model.connected && model.mem_pstate.is_some(),
                                            connect_value_changed[sender] => move |spin| {
                                                sender.input(WindowsAppMsg::MemOffsetDraft(spin.value()));
                                            },
                                        },

                                        add_suffix = &gtk::Button {
                                            set_label: "Apply",
                                            #[watch]
                                            set_sensitive: model.connected && model.mem_pstate.is_some(),
                                            connect_clicked => WindowsAppMsg::ApplyMemOffset,
                                        },
                                    },

                                    adw::ActionRow {
                                        set_title: "Clock offsets",
                                        set_subtitle: "Reset supported NVIDIA GPU/memory offsets and locked clocks.",

                                        add_suffix = &gtk::Button {
                                            set_label: "Reset clocks",
                                            add_css_class: "destructive-action",
                                            #[watch]
                                            set_sensitive: model.connected,
                                            connect_clicked => WindowsAppMsg::ResetClocks,
                                        },
                                    },
                                },

                                add = &adw::PreferencesGroup {
                                    set_title: "Preview limitations",

                                    adw::ActionRow {
                                        set_title: "Windows port in progress",
                                        set_subtitle: "Current focus is NVIDIA monitoring, power limit and clock offsets. Profiles, fan-curve writes, display controls, Windows Service installation and NVAPI V/F curve editing are still being ported.",
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
        let mut model = Self::empty();

        match connect_or_start_backend().await {
            Ok(client) => {
                model.client = Some(client.clone());
                model.connected = true;
                model.status = "Connected to local Windows daemon".to_owned();

                match client.list_devices().await {
                    Ok(devices) => {
                        if let Some(device) = devices.first() {
                            model.gpu_id = Some(device.id.clone());
                            model.device_name = device.to_string();
                            match fetch_snapshot(&client, &device.id).await {
                                Ok(snapshot) => model.apply_snapshot(snapshot, true),
                                Err(err) => model.status = format!("Connected, telemetry error: {err:#}"),
                            }
                        } else {
                            model.status = "Connected, but no NVIDIA GPU was returned by NVML".to_owned();
                        }
                    }
                    Err(err) => model.status = format!("Could not enumerate GPUs: {err:#}"),
                }
            }
            Err(err) => {
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
            WindowsAppMsg::PowerDraft(value) => self.power_limit_input = value,
            WindowsAppMsg::GpuOffsetDraft(value) => self.gpu_offset_input = value,
            WindowsAppMsg::MemOffsetDraft(value) => self.mem_offset_input = value,
            WindowsAppMsg::ApplyPower => {
                if let (Some(client), Some(id)) = (&self.client, &self.gpu_id) {
                    match client.set_power_cap(id, Some(self.power_limit_input)).await {
                        Ok(_) => {
                            self.status = format!("Applied {:.0} W power limit", self.power_limit_input);
                            self.refresh().await;
                        }
                        Err(err) => self.status = format!("Power-limit write failed: {err:#}"),
                    }
                }
            }
            WindowsAppMsg::ResetPower => {
                if let (Some(client), Some(id)) = (&self.client, &self.gpu_id) {
                    match client.set_power_cap(id, None).await {
                        Ok(_) => {
                            self.status = "Restored NVIDIA default power limit".to_owned();
                            self.refresh().await;
                        }
                        Err(err) => self.status = format!("Power-limit reset failed: {err:#}"),
                    }
                }
            }
            WindowsAppMsg::ApplyGpuOffset => {
                self.apply_offset(true).await;
            }
            WindowsAppMsg::ApplyMemOffset => {
                self.apply_offset(false).await;
            }
            WindowsAppMsg::ResetClocks => {
                if let (Some(client), Some(id)) = (&self.client, &self.gpu_id) {
                    match client.set_clocks_value(id, SetClocksCommand::reset()).await {
                        Ok(_) => {
                            self.status = "Reset NVIDIA clock offsets".to_owned();
                            self.refresh().await;
                        }
                        Err(err) => self.status = format!("Clock reset failed: {err:#}"),
                    }
                }
            }
        }
    }
}

impl WindowsApp {
    fn empty() -> Self {
        Self {
            client: None,
            gpu_id: None,
            connected: false,
            controls_initialized: false,
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
            power_range: "Driver power-limit range unavailable".to_owned(),
            fan: "-".to_owned(),
            power_limit_input: 100.0,
            gpu_offset_input: 0.0,
            mem_offset_input: 0.0,
            gpu_pstate: None,
            mem_pstate: None,
            gpu_offset_range: "Not exposed by driver".to_owned(),
            mem_offset_range: "Not exposed by driver".to_owned(),
        }
    }

    async fn refresh(&mut self) {
        let (Some(client), Some(id)) = (&self.client, &self.gpu_id) else {
            return;
        };

        match fetch_snapshot(client, id).await {
            Ok(snapshot) => {
                self.connected = true;
                self.apply_snapshot(snapshot, false);
            }
            Err(err) => {
                self.connected = false;
                self.status = format!("Telemetry refresh failed: {err:#}");
            }
        }
    }

    async fn apply_offset(&mut self, gpu: bool) {
        let (Some(client), Some(id)) = (&self.client, &self.gpu_id) else {
            return;
        };

        let (state, value, kind) = if gpu {
            (self.gpu_pstate, self.gpu_offset_input, "GPU")
        } else {
            (self.mem_pstate, self.mem_offset_input, "memory")
        };

        let Some(state) = state else {
            self.status = format!("The NVIDIA driver did not expose a writable {kind} offset P-state");
            return;
        };

        let clock_type = if gpu {
            ClockspeedType::GpuClockOffset(state)
        } else {
            ClockspeedType::MemClockOffset(state)
        };

        let command = SetClocksCommand {
            r#type: clock_type,
            value: Some(value.round() as i32),
        };

        match client.set_clocks_value(id, command).await {
            Ok(_) => {
                self.status = format!("Applied {kind} offset {value:.0} MHz on P{state}");
                self.refresh().await;
            }
            Err(err) => self.status = format!("{kind} offset write failed: {err:#}"),
        }
    }

    fn apply_snapshot(&mut self, snapshot: Snapshot, initialize_controls: bool) {
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
        self.gpu_pstate = snapshot.gpu_pstate;
        self.mem_pstate = snapshot.mem_pstate;
        self.gpu_offset_range = snapshot.gpu_offset_range;
        self.mem_offset_range = snapshot.mem_offset_range;

        if initialize_controls || !self.controls_initialized {
            if let Some(limit) = snapshot.power_limit {
                self.power_limit_input = limit;
            }
            if let Some(offset) = snapshot.gpu_offset {
                self.gpu_offset_input = f64::from(offset);
            }
            if let Some(offset) = snapshot.mem_offset {
                self.mem_offset_input = f64::from(offset);
            }
            self.controls_initialized = true;
        }

        self.status = "Connected - live telemetry active".to_owned();
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
        .unwrap_or(current_exe);

    Command::new(&backend_exe)
        .arg("daemon")
        .creation_flags(CREATE_NO_WINDOW)
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
    let clocks = client.get_device_clocks_info(id).await.ok();

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

    let mut gpu_pstate = None;
    let mut gpu_offset = None;
    let mut gpu_offset_range = "Not exposed by driver".to_owned();
    let mut mem_pstate = None;
    let mut mem_offset = None;
    let mut mem_offset_range = "Not exposed by driver".to_owned();

    if let Some(ClocksTable::Nvidia(table)) = clocks.and_then(|value| value.table) {
        if let Some((state, offset)) = table.gpu_offsets.iter().next() {
            gpu_pstate = Some(*state);
            gpu_offset = Some(offset.current);
            gpu_offset_range = format!(
                "P{state}: current {:+} MHz - allowed {:+} to {:+} MHz",
                offset.current, offset.min, offset.max
            );
        }
        if let Some((state, offset)) = table.mem_offsets.iter().next() {
            mem_pstate = Some(*state);
            mem_offset = Some(offset.current);
            mem_offset_range = format!(
                "P{state}: current {:+} MHz - allowed {:+} to {:+} MHz",
                offset.current, offset.min, offset.max
            );
        }
    }

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
        power_limit: stats.power.cap_current.or(stats.power.cap_default),
        fan,
        gpu_pstate,
        gpu_offset,
        gpu_offset_range,
        mem_pstate,
        mem_offset,
        mem_offset_range,
    })
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
