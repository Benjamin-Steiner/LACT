use amdgpu_sysfs::hw_mon::Temperature;
use anyhow::{Context, anyhow};
use lact_schema::{
    ClockspeedStats, DeviceInfo, DeviceListEntry, DeviceStats, DeviceType, DrmInfo, FanStats,
    LinkInfo, Pong, PowerStats, Request, Response, SystemInfo, TemperatureEntry, VersionInfo,
    VramStats,
};
use nvml_wrapper::{
    Device, Nvml,
    enum_wrappers::device::{Clock, TemperatureSensor, TemperatureThreshold},
};
use serde::Serialize;
use std::{collections::HashMap, env, fmt::Debug};
use tokio::{
    io::{AsyncBufReadExt, AsyncRead, AsyncWrite, AsyncWriteExt, BufReader},
    net::windows::named_pipe::ServerOptions,
    runtime,
};
use tracing::{error, info, trace, warn};

const DEFAULT_PIPE_NAME: &str = r"\\.\pipe\lactd";
const NVIDIA_ID_PREFIX: &str = "nvidia:";

pub fn run() -> anyhow::Result<()> {
    tracing_subscriber::fmt().init();

    let rt = runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .context("Could not initialize Tokio runtime")?;

    rt.block_on(run_server())
}

async fn run_server() -> anyhow::Result<()> {
    let pipe_name = env::var("LACT_DAEMON_PIPE").unwrap_or_else(|_| DEFAULT_PIPE_NAME.to_owned());
    info!("Windows daemon listening on {pipe_name}");

    let mut first_instance = true;
    loop {
        let mut options = ServerOptions::new();
        if first_instance {
            options.first_pipe_instance(true);
        }

        let server = options
            .create(&pipe_name)
            .with_context(|| format!("Could not create named pipe {pipe_name}"))?;
        first_instance = false;

        server.connect().await.context("Named pipe connect failed")?;
        info!("Windows client connected");

        if let Err(err) = handle_stream(server).await {
            error!("Windows client connection failed: {err:#}");
        }
    }
}

async fn handle_stream<T>(stream: T) -> anyhow::Result<()>
where
    T: AsyncRead + AsyncWrite + Unpin,
{
    let mut stream = BufReader::new(stream);
    let mut buf = String::new();

    while stream.read_line(&mut buf).await? != 0 {
        trace!("handling Windows request: {}", buf.trim_end());

        let response = match serde_json::from_str::<Request<'_>>(&buf) {
            Ok(Request::Ping) => ok_response(Pong)?,
            Ok(Request::SystemInfo) => ok_response(system_info())?,
            Ok(Request::ListDevices) => ok_response(list_devices()?)?,
            Ok(Request::DeviceInfo { id, .. }) => ok_response(device_info(id)?)?,
            Ok(Request::DeviceStats { id }) => ok_response(device_stats(id)?)?,
            Ok(Request::SetPowerCap { id, cap }) => ok_response(set_power_cap(id, cap)?)?,
            Ok(_) => serde_json::to_vec(&Response::<()>::from(anyhow!(
                "This request is not implemented by the Windows backend yet"
            )))?,
            Err(err) => serde_json::to_vec(&Response::<()>::from(
                anyhow::Error::new(err).context("Failed to deserialize request"),
            ))?,
        };

        stream.write_all(&response).await?;
        stream.write_all(b"\n").await?;
        buf.clear();
    }

    Ok(())
}

fn init_nvml() -> anyhow::Result<Nvml> {
    // Loading the vendor DLL is the only unsafe part of NVML initialization.
    unsafe { Nvml::init() }.context("Could not initialize NVIDIA NVML on Windows")
}

fn list_devices() -> anyhow::Result<Vec<DeviceListEntry>> {
    let nvml = match init_nvml() {
        Ok(nvml) => nvml,
        Err(err) => {
            warn!("NVIDIA NVML is unavailable; no NVIDIA GPUs will be listed: {err:#}");
            return Ok(vec![]);
        }
    };
    let count = nvml.device_count().context("Could not query NVIDIA GPU count")?;
    let mut devices = Vec::with_capacity(count as usize);

    for index in 0..count {
        let device = nvml
            .device_by_index(index)
            .with_context(|| format!("Could not open NVIDIA GPU {index}"))?;
        devices.push(DeviceListEntry {
            id: format!("{NVIDIA_ID_PREFIX}{index}"),
            name: device.name().ok(),
            device_type: DeviceType::Dedicated,
        });
    }

    Ok(devices)
}

fn device_from_id<'a>(nvml: &'a Nvml, id: &str) -> anyhow::Result<Device<'a>> {
    let index = id
        .strip_prefix(NVIDIA_ID_PREFIX)
        .context("Unsupported Windows GPU id")?
        .parse::<u32>()
        .context("Invalid NVIDIA GPU index")?;

    nvml.device_by_index(index)
        .with_context(|| format!("Could not open NVIDIA GPU {index}"))
}

fn device_info(id: &str) -> anyhow::Result<DeviceInfo> {
    let nvml = init_nvml()?;
    let device = device_from_id(&nvml, id)?;

    let mut drm_info = DrmInfo::default();
    drm_info.device_name = device.name().ok();
    drm_info.family_name = device.architecture().map(|value| value.to_string()).ok();
    drm_info.chip_class = drm_info.family_name.clone();
    drm_info.cuda_cores = device.num_cores().ok();
    drm_info.vram_clock_ratio = 1.0;
    drm_info.memory_info = device
        .bar1_memory_info()
        .map(|bar| lact_schema::DrmMemoryInfo {
            cpu_accessible_used: bar.used,
            cpu_accessible_total: bar.total,
            resizeable_bar: device
                .memory_info()
                .ok()
                .map(|memory| bar.total >= memory.total),
        })
        .ok();

    Ok(DeviceInfo {
        pci_info: None,
        api_info: Default::default(),
        driver: format!(
            "nvidia {}",
            nvml.sys_driver_version().unwrap_or_else(|_| "unknown".to_owned())
        ),
        vbios_version: device.vbios_version().ok(),
        link_info: LinkInfo {
            current_width: device.current_pcie_link_width().map(|v| v.to_string()).ok(),
            current_speed: device
                .pcie_link_speed()
                .map(|v| format!("{} GT/s", v / 1000))
                .ok(),
            max_width: device.max_pcie_link_width().map(|v| v.to_string()).ok(),
            max_speed: device
                .max_pcie_link_speed()
                .ok()
                .and_then(|v| v.as_integer())
                .map(|v| format!("{} GT/s", v / 1000)),
        },
        drm_info: Some(drm_info),
        flags: vec![],
    })
}

#[allow(clippy::cast_possible_truncation)]
fn device_stats(id: &str) -> anyhow::Result<DeviceStats> {
    let nvml = init_nvml()?;
    let device = device_from_id(&nvml, id)?;
    let mut stats = DeviceStats::default();

    if let Ok(temp) = device.temperature(TemperatureSensor::Gpu) {
        let crit = device
            .temperature_threshold(TemperatureThreshold::Shutdown)
            .map(|value| value as f32)
            .ok();
        stats.temps.insert(
            "GPU".to_owned(),
            TemperatureEntry {
                value: Temperature {
                    current: Some(temp as f32),
                    crit,
                    crit_hyst: None,
                },
                primary: true,
                display_only: false,
            },
        );
    }

    if let Ok(memory) = device.memory_info() {
        stats.vram = VramStats {
            total: Some(memory.total),
            used: Some(memory.used),
            gtt_total_usable: None,
            gtt_used: None,
        };
    }

    let constraints = device.power_management_limit_constraints().ok();
    stats.power = PowerStats {
        average: None,
        current: device.power_usage().map(|mw| f64::from(mw) / 1000.0).ok(),
        cap_current: device
            .power_management_limit()
            .map(|mw| f64::from(mw) / 1000.0)
            .ok(),
        cap_max: constraints
            .as_ref()
            .map(|limits| f64::from(limits.max_limit) / 1000.0),
        cap_min: constraints
            .as_ref()
            .map(|limits| f64::from(limits.min_limit) / 1000.0),
        cap_default: device
            .power_management_limit_default()
            .map(|mw| f64::from(mw) / 1000.0)
            .ok(),
        sensors: HashMap::new(),
    };

    stats.busy_percent = device
        .utilization_rates()
        .ok()
        .and_then(|util| u8::try_from(util.gpu).ok());

    stats.clockspeed = ClockspeedStats {
        gpu_clockspeed: device.clock_info(Clock::Graphics).map(Into::into).ok(),
        target_gpu_clockspeed: None,
        vram_clockspeed: device.clock_info(Clock::Memory).map(Into::into).ok(),
        sensors: [
            ("SM".to_owned(), device.clock_info(Clock::SM).map(Into::into)),
            (
                "Video".to_owned(),
                device.clock_info(Clock::Video).map(Into::into),
            ),
        ]
        .into_iter()
        .filter_map(|(name, value)| Some((name, value.ok()?)))
        .collect(),
    };

    let fan_count = device.num_fans().unwrap_or(0);
    if fan_count > 0 {
        stats.fan = FanStats {
            pwm_current: device
                .fan_speed(0)
                .ok()
                .map(|percent| (f64::from(percent) * 2.55).round() as u8),
            speed_current: device.fan_speed_rpm(0).ok(),
            ..Default::default()
        };
    }

    Ok(stats)
}

#[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
fn set_power_cap(id: &str, cap: Option<f64>) -> anyhow::Result<u64> {
    let nvml = init_nvml()?;
    let mut device = device_from_id(&nvml, id)?;

    let target = if let Some(cap) = cap {
        (cap * 1000.0) as u32
    } else {
        device
            .power_management_limit_default()
            .context("Could not get default NVIDIA power limit")?
    };

    let constraints = device
        .power_management_limit_constraints()
        .context("Could not get NVIDIA power limit constraints")?;
    if target < constraints.min_limit || target > constraints.max_limit {
        return Err(anyhow!(
            "Requested power limit {:.1} W is outside the allowed range {:.1}-{:.1} W",
            f64::from(target) / 1000.0,
            f64::from(constraints.min_limit) / 1000.0,
            f64::from(constraints.max_limit) / 1000.0
        ));
    }

    device
        .set_power_management_limit(target)
        .context("Could not set NVIDIA power limit")?;

    Ok(0)
}

fn system_info() -> SystemInfo {
    SystemInfo {
        version: VersionInfo::current(),
        distro: Some("Windows".to_owned()),
        kernel_version: env::var("OS").unwrap_or_else(|_| "Windows".to_owned()),
        amdgpu_overdrive_enabled: None,
        amdgpu_params_configurator: None,
    }
}

fn ok_response<T: Serialize + Debug>(data: T) -> anyhow::Result<Vec<u8>> {
    Ok(serde_json::to_vec(&Response::Ok(data))?)
}
