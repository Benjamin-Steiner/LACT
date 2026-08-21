use adw::prelude::*;
use anyhow::{Context, anyhow};
use lact_client::DaemonClient;
use lact_schema::{
    ClocksTable, DeviceStats, NvidiaVfPoint, NvidiaVoltageBoost,
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

    gpu_offset_states: Vec<u32>,
    mem_offset_states: Vec<u32>,
    vf_points: Vec<NvidiaVfPoint>,
    gpu_offset_current: Option<i32>,
    mem_offset_current: Option<i32>,
    gpu_offset_limits: Option<(i32, i32)>,
    mem_offset_limits: Option<(i32, i32)>,
    gpu_offset_description: String,
    mem_offset_description: String,

    gpu_clock_range: Option<(u32, u32)>,
    memory_clock_range: Option<(u32, u32)>,
    gpu_clock_range_description: String,
    memory_clock_range_description: String,
    voltage_boost: Option<NvidiaVoltageBoost>,
    voltage_boost_description: String,
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
    min_core_clock_input: f64,
    max_core_clock_input: f64,
    min_memory_clock_input: f64,
    max_memory_clock_input: f64,
    voltage_boost_input: f64,

    gpu_offset_states: Vec<u32>,
    mem_offset_states: Vec<u32>,
    vf_points: Vec<NvidiaVfPoint>,
    gpu_offset_supported: bool,
    mem_offset_supported: bool,
    gpu_clock_lock_supported: bool,
    memory_clock_lock_supported: bool,
    voltage_boost_supported: bool,
    core_lock_dirty: bool,
    memory_lock_dirty: bool,

    gpu_offset_description: String,
    mem_offset_description: String,
    gpu_clock_range_description: String,
    memory_clock_range_description: String,
    voltage_boost_description: String,
}

#[derive(Debug)]
pub enum WindowsAppMsg {
    Refresh,
    PowerDraft(f64),
    GpuOffsetDraft(f64),
    MemOffsetDraft(f64),
    MinCoreClockDraft(f64),
    MaxCoreClockDraft(f64),
    MinMemoryClockDraft(f64),
    MaxMemoryClockDraft(f64),
    VoltageBoostDraft(f64),
    ApplyPower,
    ResetPower,
    ApplyClocks,
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
            .default_width(900)
            .default_height(820)
            .title("LACT - Windows NVIDIA Tuning")
            .build() {
                #[wrap(Some)]
                set_content = &adw::ToolbarView {
                    add_top_bar = &adw::HeaderBar {
                        #[wrap(Some)]
                        set_title_widget = &adw::WindowTitle {
                            set_title: "LACT",
                            set_subtitle: "Windows / NVIDIA tuning preview",
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
                                    set_description: Some("Updated every second through the local Windows daemon."),

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
                                    set_title: "Clock tuning",
                                    set_description: Some("Core offset uses the native NVIDIA NVAPI V/F curve when available. Memory offset is applied to every writable NVIDIA memory P-state."),

                                    adw::ActionRow {
                                        set_title: "Core clock offset",
                                        #[watch]
                                        set_subtitle: &model.gpu_offset_description,

                                        add_suffix = &gtk::SpinButton {
                                            set_range: (-5000.0, 5000.0),
                                            set_increments: (5.0, 25.0),
                                            set_digits: 0,
                                            set_numeric: true,
                                            set_width_request: 120,
                                            set_tooltip_text: Some("Core offset in MHz"),
                                            set_value: model.gpu_offset_input,
                                            #[watch]
                                            set_sensitive: model.connected && model.gpu_offset_supported,
                                            connect_value_changed[sender] => move |spin| {
                                                sender.input(WindowsAppMsg::GpuOffsetDraft(spin.value()));
                                            },
                                        },
                                    },

                                    adw::ActionRow {
                                        set_title: "Memory clock offset",
                                        #[watch]
                                        set_subtitle: &model.mem_offset_description,

                                        add_suffix = &gtk::SpinButton {
                                            set_range: (-5000.0, 5000.0),
                                            set_increments: (5.0, 25.0),
                                            set_digits: 0,
                                            set_numeric: true,
                                            set_width_request: 120,
                                            set_tooltip_text: Some("Memory offset in MHz"),
                                            set_value: model.mem_offset_input,
                                            #[watch]
                                            set_sensitive: model.connected && model.mem_offset_supported,
                                            connect_value_changed[sender] => move |spin| {
                                                sender.input(WindowsAppMsg::MemOffsetDraft(spin.value()));
                                            },
                                        },
                                    },

                                    adw::ActionRow {
                                        set_title: "Minimum core clock",
                                        #[watch]
                                        set_subtitle: &model.gpu_clock_range_description,

                                        add_suffix = &gtk::SpinButton {
                                            set_range: (0.0, 10000.0),
                                            set_increments: (15.0, 100.0),
                                            set_digits: 0,
                                            set_numeric: true,
                                            set_width_request: 120,
                                            set_tooltip_text: Some("Minimum locked core clock in MHz"),
                                            set_value: model.min_core_clock_input,
                                            #[watch]
                                            set_sensitive: model.connected && model.gpu_clock_lock_supported,
                                            connect_value_changed[sender] => move |spin| {
                                                sender.input(WindowsAppMsg::MinCoreClockDraft(spin.value()));
                                            },
                                        },
                                    },

                                    adw::ActionRow {
                                        set_title: "Maximum core clock",
                                        set_subtitle: "Applied together with minimum core clock when either value is edited.",

                                        add_suffix = &gtk::SpinButton {
                                            set_range: (0.0, 10000.0),
                                            set_increments: (15.0, 100.0),
                                            set_digits: 0,
                                            set_numeric: true,
                                            set_width_request: 120,
                                            set_tooltip_text: Some("Maximum locked core clock in MHz"),
                                            set_value: model.max_core_clock_input,
                                            #[watch]
                                            set_sensitive: model.connected && model.gpu_clock_lock_supported,
                                            connect_value_changed[sender] => move |spin| {
                                                sender.input(WindowsAppMsg::MaxCoreClockDraft(spin.value()));
                                            },
                                        },
                                    },

                                    adw::ActionRow {
                                        set_title: "Minimum memory clock",
                                        #[watch]
                                        set_subtitle: &model.memory_clock_range_description,

                                        add_suffix = &gtk::SpinButton {
                                            set_range: (0.0, 20000.0),
                                            set_increments: (15.0, 100.0),
                                            set_digits: 0,
                                            set_numeric: true,
                                            set_width_request: 120,
                                            set_tooltip_text: Some("Minimum locked memory clock in MHz"),
                                            set_value: model.min_memory_clock_input,
                                            #[watch]
                                            set_sensitive: model.connected && model.memory_clock_lock_supported,
                                            connect_value_changed[sender] => move |spin| {
                                                sender.input(WindowsAppMsg::MinMemoryClockDraft(spin.value()));
                                            },
                                        },
                                    },

                                    adw::ActionRow {
                                        set_title: "Maximum memory clock",
                                        set_subtitle: "Applied together with minimum memory clock when either value is edited.",

                                        add_suffix = &gtk::SpinButton {
                                            set_range: (0.0, 20000.0),
                                            set_increments: (15.0, 100.0),
                                            set_digits: 0,
                                            set_numeric: true,
                                            set_width_request: 120,
                                            set_tooltip_text: Some("Maximum locked memory clock in MHz"),
                                            set_value: model.max_memory_clock_input,
                                            #[watch]
                                            set_sensitive: model.connected && model.memory_clock_lock_supported,
                                            connect_value_changed[sender] => move |spin| {
                                                sender.input(WindowsAppMsg::MaxMemoryClockDraft(spin.value()));
                                            },
                                        },
                                    },

                                    adw::ActionRow {
                                        set_title: "Voltage boost",
                                        #[watch]
                                        set_subtitle: &model.voltage_boost_description,

                                        add_suffix = &gtk::SpinButton {
                                            set_range: (0.0, 100.0),
                                            set_increments: (1.0, 5.0),
                                            set_digits: 0,
                                            set_numeric: true,
                                            set_width_request: 120,
                                            set_tooltip_text: Some("NVIDIA voltage boost percentage"),
                                            set_value: model.voltage_boost_input,
                                            #[watch]
                                            set_sensitive: model.connected && model.voltage_boost_supported,
                                            connect_value_changed[sender] => move |spin| {
                                                sender.input(WindowsAppMsg::VoltageBoostDraft(spin.value()));
                                            },
                                        },
                                    },

                                    adw::ActionRow {
                                        set_title: "Clock settings",
                                        set_subtitle: "Apply sends all supported offsets in one request. Clock locks are changed only after you edit their min/max fields.",

                                        add_suffix = &gtk::Button {
                                            set_label: "Apply clocks",
                                            add_css_class: "suggested-action",
                                            #[watch]
                                            set_sensitive: model.connected && (model.gpu_offset_supported || model.mem_offset_supported || model.gpu_clock_lock_supported || model.memory_clock_lock_supported || model.voltage_boost_supported),
                                            connect_clicked => WindowsAppMsg::ApplyClocks,
                                        },

                                        add_suffix = &gtk::Button {
                                            set_label: "Reset all",
                                            add_css_class: "destructive-action",
                                            #[watch]
                                            set_sensitive: model.connected,
                                            connect_clicked => WindowsAppMsg::ResetClocks,
                                        },
                                    },
                                },

                                add = &adw::PreferencesGroup {
                                    set_title: "Power limit",

                                    adw::ActionRow {
                                        set_title: "GPU power limit",
                                        #[watch]
                                        set_subtitle: &model.power_range,

                                        add_suffix = &gtk::SpinButton {
                                            set_range: (1.0, 1000.0),
                                            set_increments: (1.0, 10.0),
                                            set_digits: 0,
                                            set_numeric: true,
                                            set_width_request: 120,
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
                                },

                                add = &adw::PreferencesGroup {
                                    set_title: "v1 scope",

                                    adw::ActionRow {
                                        set_title: "NVIDIA tuning first",
                                        set_subtitle: "v1 exposes core V/F frequency offset, memory P-state offset, core/memory clock locks, voltage boost, power limit and live telemetry. Direct V/F voltage editing remains disabled because NVIDIA exposes those point voltages as immutable on this path.",
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
            WindowsAppMsg::MinCoreClockDraft(value) => {
                self.min_core_clock_input = value;
                self.core_lock_dirty = true;
            }
            WindowsAppMsg::MaxCoreClockDraft(value) => {
                self.max_core_clock_input = value;
                self.core_lock_dirty = true;
            }
            WindowsAppMsg::MinMemoryClockDraft(value) => {
                self.min_memory_clock_input = value;
                self.memory_lock_dirty = true;
            }
            WindowsAppMsg::MaxMemoryClockDraft(value) => {
                self.max_memory_clock_input = value;
                self.memory_lock_dirty = true;
            }
            WindowsAppMsg::VoltageBoostDraft(value) => self.voltage_boost_input = value,
            WindowsAppMsg::ApplyPower => self.apply_power().await,
            WindowsAppMsg::ResetPower => self.reset_power().await,
            WindowsAppMsg::ApplyClocks => self.apply_clocks().await,
            WindowsAppMsg::ResetClocks => self.reset_clocks().await,
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
            min_core_clock_input: 0.0,
            max_core_clock_input: 0.0,
            min_memory_clock_input: 0.0,
            max_memory_clock_input: 0.0,
            voltage_boost_input: 0.0,
            gpu_offset_states: vec![],
            mem_offset_states: vec![],
            vf_points: vec![],
            gpu_offset_supported: false,
            mem_offset_supported: false,
            gpu_clock_lock_supported: false,
            memory_clock_lock_supported: false,
            voltage_boost_supported: false,
            core_lock_dirty: false,
            memory_lock_dirty: false,
            gpu_offset_description: "Not exposed by driver".to_owned(),
            mem_offset_description: "Not exposed by driver".to_owned(),
            gpu_clock_range_description: "Not exposed by driver".to_owned(),
            memory_clock_range_description: "Not exposed by driver".to_owned(),
            voltage_boost_description: "NVAPI voltage boost unavailable".to_owned(),
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

    async fn apply_power(&mut self) {
        if let (Some(client), Some(id)) = (&self.client, &self.gpu_id) {
            match client.set_power_cap(id, Some(self.power_limit_input)).await {
                Ok(_) => {
                    self.refresh().await;
                    self.status = format!("Applied {:.0} W power limit", self.power_limit_input);
                }
                Err(err) => self.status = format!("Power-limit write failed: {err:#}"),
            }
        }
    }

    async fn reset_power(&mut self) {
        if let (Some(client), Some(id)) = (&self.client, &self.gpu_id) {
            match client.set_power_cap(id, None).await {
                Ok(_) => {
                    self.controls_initialized = false;
                    self.refresh().await;
                    self.status = "Restored NVIDIA default power limit".to_owned();
                }
                Err(err) => self.status = format!("Power-limit reset failed: {err:#}"),
            }
        }
    }

    async fn apply_clocks(&mut self) {
        let (Some(client), Some(id)) = (&self.client, &self.gpu_id) else {
            return;
        };

        let mut commands = Vec::new();
        let gpu_offset = self.gpu_offset_input.round() as i32;
        let mem_offset = self.mem_offset_input.round() as i32;

        if self.gpu_offset_supported {
            if !self.vf_points.is_empty() {
                for point in &self.vf_points {
                    let target = i64::from(point.base_freq) + i64::from(gpu_offset);
                    if target <= 0 || target > i64::from(i32::MAX) {
                        self.status = format!(
                            "Core offset {gpu_offset:+} MHz produces an invalid target for V/F point {}",
                            point.index
                        );
                        return;
                    }
                    commands.push(SetClocksCommand {
                        r#type: ClockspeedType::GpuVfCurveClock(point.index),
                        value: Some(target as i32),
                    });
                }
            } else {
                for state in &self.gpu_offset_states {
                    commands.push(SetClocksCommand {
                        r#type: ClockspeedType::GpuClockOffset(*state),
                        value: Some(gpu_offset),
                    });
                }
            }
        }

        if self.mem_offset_supported {
            for state in &self.mem_offset_states {
                commands.push(SetClocksCommand {
                    r#type: ClockspeedType::MemClockOffset(*state),
                    value: Some(mem_offset),
                });
            }
        }

        if self.core_lock_dirty && self.gpu_clock_lock_supported {
            let min = self.min_core_clock_input.round() as i32;
            let max = self.max_core_clock_input.round() as i32;
            if min > max {
                self.status = "Minimum core clock cannot exceed maximum core clock".to_owned();
                return;
            }
            commands.extend([
                SetClocksCommand {
                    r#type: ClockspeedType::MinCoreClock,
                    value: Some(min),
                },
                SetClocksCommand {
                    r#type: ClockspeedType::MaxCoreClock,
                    value: Some(max),
                },
            ]);
        }

        if self.memory_lock_dirty && self.memory_clock_lock_supported {
            let min = self.min_memory_clock_input.round() as i32;
            let max = self.max_memory_clock_input.round() as i32;
            if min > max {
                self.status = "Minimum memory clock cannot exceed maximum memory clock".to_owned();
                return;
            }
            commands.extend([
                SetClocksCommand {
                    r#type: ClockspeedType::MinMemoryClock,
                    value: Some(min),
                },
                SetClocksCommand {
                    r#type: ClockspeedType::MaxMemoryClock,
                    value: Some(max),
                },
            ]);
        }

        if self.voltage_boost_supported {
            commands.push(SetClocksCommand {
                r#type: ClockspeedType::VoltageBoost,
                value: Some(self.voltage_boost_input.round() as i32),
            });
        }

        if commands.is_empty() {
            self.status = "The NVIDIA driver did not expose any writable clock controls".to_owned();
            return;
        }

        match client.batch_set_clocks_value(id, commands).await {
            Ok(_) => {
                self.core_lock_dirty = false;
                self.memory_lock_dirty = false;
                self.refresh().await;
                self.status = format!(
                    "Applied clocks - core {gpu_offset:+} MHz, memory {mem_offset:+} MHz"
                );
            }
            Err(err) => self.status = format!("Clock write failed: {err:#}"),
        }
    }

    async fn reset_clocks(&mut self) {
        let (Some(client), Some(id)) = (&self.client, &self.gpu_id) else {
            return;
        };

        match client.set_clocks_value(id, SetClocksCommand::reset()).await {
            Ok(_) => {
                self.controls_initialized = false;
                self.core_lock_dirty = false;
                self.memory_lock_dirty = false;
                self.refresh().await;
                self.status = "Reset NVIDIA offsets, V/F curve, clock locks and voltage boost".to_owned();
            }
            Err(err) => self.status = format!("Clock reset failed: {err:#}"),
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

        self.gpu_offset_states = snapshot.gpu_offset_states;
        self.mem_offset_states = snapshot.mem_offset_states;
        self.vf_points = snapshot.vf_points;
        self.gpu_offset_supported = snapshot.gpu_offset_limits.is_some()
            && (!self.vf_points.is_empty() || !self.gpu_offset_states.is_empty());
        self.mem_offset_supported = snapshot.mem_offset_limits.is_some() && !self.mem_offset_states.is_empty();
        self.gpu_clock_lock_supported = snapshot.gpu_clock_range.is_some();
        self.memory_clock_lock_supported = snapshot.memory_clock_range.is_some();
        self.voltage_boost_supported = snapshot.voltage_boost.is_some();
        self.gpu_offset_description = snapshot.gpu_offset_description;
        self.mem_offset_description = snapshot.mem_offset_description;
        self.gpu_clock_range_description = snapshot.gpu_clock_range_description;
        self.memory_clock_range_description = snapshot.memory_clock_range_description;
        self.voltage_boost_description = snapshot.voltage_boost_description;

        if initialize_controls || !self.controls_initialized {
            if let Some(limit) = snapshot.power_limit {
                self.power_limit_input = limit;
            }
            self.gpu_offset_input = f64::from(snapshot.gpu_offset_current.unwrap_or(0));
            self.mem_offset_input = f64::from(snapshot.mem_offset_current.unwrap_or(0));

            if let Some((min, max)) = snapshot.gpu_clock_range {
                self.min_core_clock_input = f64::from(min);
                self.max_core_clock_input = f64::from(max);
            }
            if let Some((min, max)) = snapshot.memory_clock_range {
                self.min_memory_clock_input = f64::from(min);
                self.max_memory_clock_input = f64::from(max);
            }
            if let Some(boost) = snapshot.voltage_boost {
                self.voltage_boost_input = f64::from(boost.current);
            }
            self.controls_initialized = true;
        }

        self.status = if self.vf_points.is_empty() {
            "Connected - NVML tuning active".to_owned()
        } else {
            format!(
                "Connected - NVAPI V/F tuning active ({} editable points)",
                self.vf_points.len()
            )
        };
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

    let mut snapshot = Snapshot {
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
        gpu_offset_description: "Not exposed by driver".to_owned(),
        mem_offset_description: "Not exposed by driver".to_owned(),
        gpu_clock_range_description: "Not exposed by driver".to_owned(),
        memory_clock_range_description: "Not exposed by driver".to_owned(),
        voltage_boost_description: "NVAPI voltage boost unavailable".to_owned(),
        ..Default::default()
    };

    if let Some(ClocksTable::Nvidia(table)) = clocks.and_then(|value| value.table) {
        snapshot.gpu_offset_states = table.gpu_offsets.keys().copied().collect();
        snapshot.mem_offset_states = table.mem_offsets.keys().copied().collect();
        snapshot.vf_points = table.gpu_vf_curve;
        snapshot.gpu_clock_range = table.gpu_clock_range;
        snapshot.memory_clock_range = table.vram_clock_range;
        snapshot.voltage_boost = table.voltage_boost;

        snapshot.gpu_offset_limits = intersect_offset_limits(table.gpu_offsets.values());
        snapshot.mem_offset_limits = intersect_offset_limits(table.mem_offsets.values());

        snapshot.gpu_offset_current = if !snapshot.vf_points.is_empty() {
            uniform_vf_offset(&snapshot.vf_points)
        } else {
            uniform_offset(table.gpu_offsets.values().map(|offset| offset.current))
        };
        snapshot.mem_offset_current =
            uniform_offset(table.mem_offsets.values().map(|offset| offset.current));

        snapshot.gpu_offset_description = match snapshot.gpu_offset_limits {
            Some((min, max)) if !snapshot.vf_points.is_empty() => format!(
                "NVAPI V/F curve - {} editable points - allowed {min:+} to {max:+} MHz",
                snapshot.vf_points.len()
            ),
            Some((min, max)) => format!(
                "{} writable GPU P-states - allowed {min:+} to {max:+} MHz",
                snapshot.gpu_offset_states.len()
            ),
            None => "No writable core offset range reported".to_owned(),
        };
        snapshot.mem_offset_description = match snapshot.mem_offset_limits {
            Some((min, max)) => format!(
                "{} writable memory P-states - allowed {min:+} to {max:+} MHz",
                snapshot.mem_offset_states.len()
            ),
            None => "No writable memory offset range reported".to_owned(),
        };
        snapshot.gpu_clock_range_description = snapshot.gpu_clock_range.map_or_else(
            || "GPU clock locking unavailable".to_owned(),
            |(min, max)| format!("Driver range {min} to {max} MHz - edit min/max to enable lock"),
        );
        snapshot.memory_clock_range_description = snapshot.memory_clock_range.map_or_else(
            || "Memory clock locking unavailable".to_owned(),
            |(min, max)| format!("Driver range {min} to {max} MHz - edit min/max to enable lock"),
        );
        snapshot.voltage_boost_description = snapshot.voltage_boost.map_or_else(
            || "NVAPI voltage boost unavailable".to_owned(),
            |boost| format!(
                "Current {}% - allowed {} to {}%",
                boost.current, boost.min, boost.max
            ),
        );
    }

    Ok(snapshot)
}

fn intersect_offset_limits<'a>(
    offsets: impl Iterator<Item = &'a lact_schema::NvidiaClockOffset>,
) -> Option<(i32, i32)> {
    let mut min = i32::MIN;
    let mut max = i32::MAX;
    let mut any = false;
    for offset in offsets {
        min = min.max(offset.min);
        max = max.min(offset.max);
        any = true;
    }
    (any && min <= max).then_some((min, max))
}

fn uniform_offset(values: impl Iterator<Item = i32>) -> Option<i32> {
    let mut values = values;
    let first = values.next()?;
    values.all(|value| value == first).then_some(first)
}

fn uniform_vf_offset(points: &[NvidiaVfPoint]) -> Option<i32> {
    let mut values = points.iter().map(|point| {
        i64::from(point.freq).saturating_sub(i64::from(point.base_freq))
    });
    let first = values.next()?;
    if values.all(|value| value == first) {
        i32::try_from(first).ok()
    } else {
        None
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
