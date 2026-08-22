#![allow(
    clippy::large_stack_arrays,
    clippy::missing_errors_doc,
    clippy::unreadable_literal,
    unsafe_op_in_unsafe_fn
)]

use anyhow::{Context, bail, ensure};
use lact_schema::{NvidiaVfPoint, NvidiaVoltageBoost};
use libloading::Library;
use std::{
    ffi::{CStr, c_char, c_void},
    mem::{self, transmute},
    ptr,
};

const LIBRARY_NAME: &str = "nvapi64.dll";
const QUERY_INTERFACE_FN: &[u8] = b"nvapi_QueryInterface\0";
const NVAPI_MAX_PHYSICAL_GPUS: usize = 64;
const NVAPI_SHORT_STRING_MAX: usize = 64;

const QUERY_NVAPI_INITIALIZE: u32 = 0x0150e828;
const QUERY_NVAPI_UNLOAD: u32 = 0xd22bdd7e;
const QUERY_NVAPI_ENUM_PHYSICAL_GPUS: u32 = 0xe5ac921f;
const QUERY_NVAPI_GET_BUS_ID: u32 = 0x1be0b8e5;
const QUERY_NVAPI_GET_ERROR_MESSAGE: u32 = 0x6c2d048c;
const QUERY_NVAPI_VOLTAGE_BOOST_GET: u32 = 0x9df23ca1;
const QUERY_NVAPI_VOLTAGE_BOOST_SET: u32 = 0xb9306d9b;
const QUERY_NVAPI_GPU_CLOCK_CLIENT_CLK_VF_POINTS_GET_STATUS: u32 = 0x21537ad4;
const QUERY_NVAPI_GPU_CLOCK_CLIENT_CLK_VF_POINTS_GET_INFO: u32 = 0x507b4b59;
const QUERY_NVAPI_GPU_CLOCK_CLIENT_CLK_VF_POINTS_SET_CONTROL: u32 = 0x0733e009;
const QUERY_NVAPI_GPU_CLOCK_CLIENT_CLK_VF_POINTS_GET_CONTROL: u32 = 0x23f1b133;

const CLOCK_CLIENT_CLK_VF_POINT_TYPE_PROG: NvU32 = 0;
const VOLTAGE_BOOST_MIN: i32 = 0;
const VOLTAGE_BOOST_MAX: i32 = 100;

type NvApiStatus = i32;
type NvPhysicalGpuHandle = *mut c_void;
type NvS32 = i32;
type NvU8 = u8;
type NvU32 = u32;

pub struct NvApi {
    lib: Library,
}

impl NvApi {
    pub fn new() -> anyhow::Result<Self> {
        let lib = unsafe { Library::new(LIBRARY_NAME) }
            .context("Could not load NVIDIA nvapi64.dll")?;
        let handle = Self { lib };

        unsafe {
            let initialize = handle.query_interface(QUERY_NVAPI_INITIALIZE)?;
            let initialize: unsafe extern "C" fn() -> NvApiStatus = transmute(initialize);
            handle.handle_status(initialize())?;
            // Force enumeration once during initialization. This also catches a
            // driver installation where NVAPI exists but cannot access a GPU.
            let _ = handle.enum_physical_gpus()?;
        }

        Ok(handle)
    }

    pub fn find_matching_gpu(&self, bus_id: u32) -> anyhow::Result<Option<NvPhysicalGpuHandle>> {
        unsafe {
            for handle in self.enum_physical_gpus()? {
                let f = self.query_interface(QUERY_NVAPI_GET_BUS_ID)?;
                let f: unsafe extern "C" fn(NvPhysicalGpuHandle, &mut u32) -> NvApiStatus =
                    transmute(f);
                let mut id = 0;
                self.handle_status(f(handle, &mut id))?;
                if id == bus_id {
                    return Ok(Some(handle));
                }
            }
        }
        Ok(None)
    }

    pub fn get_vf_curve(
        &self,
        handle: NvPhysicalGpuHandle,
    ) -> anyhow::Result<Vec<NvidiaVfPoint>> {
        unsafe {
            let info = self.clock_client_clk_vf_points_get_info(handle)?;
            let status = self.clock_client_clk_vf_points_get_status(handle, info.vf_points_mask)?;
            let control = self
                .clock_client_clk_vf_get_control(handle, info.vf_points_mask)
                .ok();
            let point_count = point_count_from_mask(info.vf_points_mask);
            let mut curve = Vec::with_capacity(point_count);

            for index in 0..point_count {
                let point_info = info.vf_points[index];
                if !vf_curve_point_is_editable(point_info) {
                    continue;
                }

                let point = status.vf_points[index];
                let (base_freq_khz, base_voltage_uv) = if status.b_vf_tuple_base_supported != 0 {
                    (
                        point.vf_tuple_base.freq_khz,
                        point.vf_tuple_base.voltage_uv,
                    )
                } else {
                    // On cards/drivers that do not expose the base tuple, derive
                    // the base frequency from the current point and NVAPI control
                    // offset. This keeps the Windows daemon stateless while still
                    // letting us edit the curve repeatedly without accumulating
                    // offsets.
                    let offset_khz = control
                        .as_ref()
                        .map_or(0, |control| control.vf_points[index].data.prog.freq_offset_khz);
                    let base = i64::from(point.freq_khz) - i64::from(offset_khz);
                    (base.max(0) as u32, point.voltage_uv)
                };

                curve.push(NvidiaVfPoint {
                    index: u8::try_from(index).expect("NVAPI exposes at most 255 V/F points"),
                    freq: point.freq_khz / 1000,
                    voltage: point.voltage_uv / 1000,
                    base_freq: base_freq_khz / 1000,
                    base_voltage: base_voltage_uv / 1000,
                });
            }

            Ok(curve)
        }
    }

    pub fn set_vf_point_clock(
        &self,
        handle: NvPhysicalGpuHandle,
        index: u8,
        target_mhz: i32,
        min_offset_mhz: i32,
        max_offset_mhz: i32,
    ) -> anyhow::Result<()> {
        unsafe {
            let info = self.clock_client_clk_vf_points_get_info(handle)?;
            let status = self.clock_client_clk_vf_points_get_status(handle, info.vf_points_mask)?;
            let mut control =
                self.clock_client_clk_vf_get_control(handle, info.vf_points_mask)?;
            let index = usize::from(index);
            let point_count = point_count_from_mask(info.vf_points_mask);
            ensure!(index < point_count, "NVAPI V/F point {index} is out of range");
            ensure!(
                vf_curve_point_is_editable(info.vf_points[index]),
                "NVAPI V/F point {index} is not writable"
            );

            let point = status.vf_points[index];
            let base_freq_khz = if status.b_vf_tuple_base_supported != 0 {
                i64::from(point.vf_tuple_base.freq_khz)
            } else {
                i64::from(point.freq_khz)
                    - i64::from(control.vf_points[index].data.prog.freq_offset_khz)
            };
            let target_khz = i64::from(target_mhz) * 1000;
            let offset_khz = target_khz - base_freq_khz;
            let offset_mhz = offset_khz / 1000;
            ensure!(
                (i64::from(min_offset_mhz)..=i64::from(max_offset_mhz)).contains(&offset_mhz),
                "Requested V/F offset {offset_mhz} MHz for point {index} is outside the driver range {min_offset_mhz}..={max_offset_mhz} MHz"
            );
            let offset_khz = i32::try_from(offset_khz)
                .context("Requested V/F clock offset does not fit in NVAPI range")?;

            control.vf_points[index].data.prog.freq_offset_khz = offset_khz;
            self.clock_client_clk_vf_set_control(handle, control)?;
        }
        Ok(())
    }

    pub fn reset_vf_curve(&self, handle: NvPhysicalGpuHandle) -> anyhow::Result<()> {
        unsafe {
            let info = self.clock_client_clk_vf_points_get_info(handle)?;
            let mut control =
                self.clock_client_clk_vf_get_control(handle, info.vf_points_mask)?;
            let point_count = point_count_from_mask(info.vf_points_mask);

            for index in 0..point_count {
                if vf_curve_point_is_editable(info.vf_points[index]) {
                    control.vf_points[index].data.prog.freq_offset_khz = 0;
                }
            }
            self.clock_client_clk_vf_set_control(handle, control)?;
        }
        Ok(())
    }

    pub fn get_voltage_boost(
        &self,
        handle: NvPhysicalGpuHandle,
    ) -> anyhow::Result<NvidiaVoltageBoost> {
        let current = unsafe { self.get_voltage_boost_raw(handle)? };
        Ok(NvidiaVoltageBoost {
            current: i32::from(current),
            min: VOLTAGE_BOOST_MIN,
            max: VOLTAGE_BOOST_MAX,
        })
    }

    pub fn set_voltage_boost(
        &self,
        handle: NvPhysicalGpuHandle,
        percent: i32,
    ) -> anyhow::Result<()> {
        ensure!(
            (VOLTAGE_BOOST_MIN..=VOLTAGE_BOOST_MAX).contains(&percent),
            "Voltage boost {percent}% is outside the allowed range {VOLTAGE_BOOST_MIN}..={VOLTAGE_BOOST_MAX}%"
        );
        let percent = u8::try_from(percent).expect("validated voltage boost fits into u8");
        unsafe {
            let mut data = NvApiVoltageBoost {
                percent,
                ..Default::default()
            };
            self.physical_gpu_query(handle, &mut data, QUERY_NVAPI_VOLTAGE_BOOST_SET)?;
            let applied = self.get_voltage_boost_raw(handle)?;
            ensure!(
                applied == percent,
                "NVIDIA driver did not apply voltage boost: requested {percent}%, driver reports {applied}%"
            );
        }
        Ok(())
    }

    unsafe fn get_voltage_boost_raw(
        &self,
        handle: NvPhysicalGpuHandle,
    ) -> anyhow::Result<u8> {
        let mut data = NvApiVoltageBoost::default();
        self.physical_gpu_query(handle, &mut data, QUERY_NVAPI_VOLTAGE_BOOST_GET)?;
        Ok(data.percent)
    }

    unsafe fn clock_client_clk_vf_points_get_info(
        &self,
        handle: NvPhysicalGpuHandle,
    ) -> anyhow::Result<ClockClientClkVfPointsInfoV1> {
        let mut data = ClockClientClkVfPointsInfoV1::default();
        self.physical_gpu_query(
            handle,
            &mut data,
            QUERY_NVAPI_GPU_CLOCK_CLIENT_CLK_VF_POINTS_GET_INFO,
        )?;
        Ok(data)
    }

    unsafe fn clock_client_clk_vf_points_get_status(
        &self,
        handle: NvPhysicalGpuHandle,
        vf_points_mask: [NvU32; 8],
    ) -> anyhow::Result<ClockClientClkVfPointsStatusV3> {
        let mut data = ClockClientClkVfPointsStatusV3 {
            vf_points_mask,
            ..Default::default()
        };
        self.physical_gpu_query(
            handle,
            &mut data,
            QUERY_NVAPI_GPU_CLOCK_CLIENT_CLK_VF_POINTS_GET_STATUS,
        )?;
        Ok(data)
    }

    unsafe fn clock_client_clk_vf_get_control(
        &self,
        handle: NvPhysicalGpuHandle,
        vf_points_mask: [NvU32; 8],
    ) -> anyhow::Result<ClockClientClkVfPointsControlV1> {
        let mut data = ClockClientClkVfPointsControlV1 {
            vf_points_mask,
            ..Default::default()
        };
        self.physical_gpu_query(
            handle,
            &mut data,
            QUERY_NVAPI_GPU_CLOCK_CLIENT_CLK_VF_POINTS_GET_CONTROL,
        )?;
        Ok(data)
    }

    unsafe fn clock_client_clk_vf_set_control(
        &self,
        handle: NvPhysicalGpuHandle,
        mut control: ClockClientClkVfPointsControlV1,
    ) -> anyhow::Result<()> {
        self.physical_gpu_query(
            handle,
            &mut control,
            QUERY_NVAPI_GPU_CLOCK_CLIENT_CLK_VF_POINTS_SET_CONTROL,
        )
    }

    unsafe fn enum_physical_gpus(&self) -> anyhow::Result<Vec<NvPhysicalGpuHandle>> {
        let f = self.query_interface(QUERY_NVAPI_ENUM_PHYSICAL_GPUS)?;
        let f: unsafe extern "C" fn(
            &mut [NvPhysicalGpuHandle; NVAPI_MAX_PHYSICAL_GPUS],
            &mut u32,
        ) -> NvApiStatus = transmute(f);
        let mut count = 0;
        let mut handles = [ptr::null_mut(); NVAPI_MAX_PHYSICAL_GPUS];
        self.handle_status(f(&mut handles, &mut count))?;
        Ok(handles.into_iter().take(count as usize).collect())
    }

    unsafe fn query_interface(&self, id: u32) -> anyhow::Result<*const ()> {
        let query_interface = self
            .lib
            .get::<unsafe extern "C" fn(u32) -> *const ()>(QUERY_INTERFACE_FN)
            .context("Could not resolve nvapi_QueryInterface")?;
        let function = query_interface(id);
        if function.is_null() {
            bail!("NVAPI query interface returned null for id 0x{id:08x}");
        }
        Ok(function)
    }

    unsafe fn handle_status(&self, status: NvApiStatus) -> anyhow::Result<()> {
        if status == 0 {
            return Ok(());
        }

        let f = self.query_interface(QUERY_NVAPI_GET_ERROR_MESSAGE)?;
        let f: unsafe extern "C" fn(
            NvApiStatus,
            &mut [c_char; NVAPI_SHORT_STRING_MAX],
        ) -> NvApiStatus = transmute(f);
        let mut text = [0; NVAPI_SHORT_STRING_MAX];
        let error_status = f(status, &mut text);
        if error_status != 0 {
            bail!(
                "NVAPI returned status {status} and error text lookup returned {error_status}"
            );
        }
        let text = CStr::from_ptr(text.as_ptr()).to_string_lossy();
        bail!("NVAPI error {status}: {text}")
    }

    unsafe fn physical_gpu_query<T>(
        &self,
        handle: NvPhysicalGpuHandle,
        data: &mut T,
        query_id: u32,
    ) -> anyhow::Result<()> {
        let f = self.query_interface(query_id)?;
        let f: unsafe extern "C" fn(NvPhysicalGpuHandle, *mut T) -> NvApiStatus = transmute(f);
        self.handle_status(f(handle, data))
    }
}

impl Drop for NvApi {
    fn drop(&mut self) {
        unsafe {
            if let Ok(unload) = self.query_interface(QUERY_NVAPI_UNLOAD) {
                let unload: unsafe extern "C" fn() -> NvApiStatus = transmute(unload);
                let _ = unload();
            }
        }
    }
}

fn vf_curve_point_is_editable(point: ClockClientClkVfPointInfoV1) -> bool {
    point.b_voltage_based == 1 && point.type_ == CLOCK_CLIENT_CLK_VF_POINT_TYPE_PROG
}

fn point_count_from_mask(mask: [u32; 8]) -> usize {
    mask.iter().map(|value| value.count_ones() as usize).sum()
}

#[allow(clippy::cast_possible_truncation)]
const fn make_version<T>(version: usize) -> u32 {
    (mem::size_of::<T>() | (version << 16)) as u32
}

#[repr(C)]
#[derive(Debug, Copy, Clone)]
struct NvApiVoltageBoost {
    version: NvU32,
    percent: NvU8,
    reserved: [NvU8; 32],
}

impl Default for NvApiVoltageBoost {
    fn default() -> Self {
        Self {
            version: make_version::<Self>(1),
            percent: 0,
            reserved: [0; 32],
        }
    }
}

#[repr(C)]
#[derive(Debug, Copy, Clone)]
struct ClockClientClkVfPointsStatusV3 {
    version: NvU32,
    vf_points_mask: [NvU32; 8],
    b_vf_tuple_base_supported: NvU8,
    reserved: [NvU8; 64],
    vf_points: [ClockClientClkVfPointStatusV3; 255],
}

impl Default for ClockClientClkVfPointsStatusV3 {
    fn default() -> Self {
        Self {
            version: make_version::<Self>(3),
            vf_points_mask: [0; 8],
            b_vf_tuple_base_supported: 0,
            reserved: [0; 64],
            vf_points: [ClockClientClkVfPointStatusV3::default(); 255],
        }
    }
}

#[repr(C)]
#[derive(Debug, Copy, Clone)]
struct ClockClientClkVfPointStatusV3 {
    type_: NvU32,
    freq_khz: NvU32,
    voltage_uv: NvU32,
    vf_tuple_base: ClockClientClkVfPointTupleV1,
    vf_tuple_offset: ClockClientClkVfPointTupleV1,
    reserved: [NvU8; 256],
}

impl Default for ClockClientClkVfPointStatusV3 {
    fn default() -> Self {
        Self {
            type_: 0,
            freq_khz: 0,
            voltage_uv: 0,
            vf_tuple_base: ClockClientClkVfPointTupleV1::default(),
            vf_tuple_offset: ClockClientClkVfPointTupleV1::default(),
            reserved: [0; 256],
        }
    }
}

#[repr(C)]
#[derive(Debug, Copy, Clone, Default)]
struct ClockClientClkVfPointTupleV1 {
    freq_khz: NvU32,
    voltage_uv: NvU32,
    reserved: [NvU8; 32],
}

#[repr(C)]
#[derive(Debug, Copy, Clone)]
struct ClockClientClkVfPointsInfoV1 {
    version: NvU32,
    vf_points_mask: [NvU32; 8],
    reserved: [NvU8; 32],
    vf_points: [ClockClientClkVfPointInfoV1; 255],
}

impl Default for ClockClientClkVfPointsInfoV1 {
    fn default() -> Self {
        Self {
            version: make_version::<Self>(1),
            vf_points_mask: [0; 8],
            reserved: [0; 32],
            vf_points: [ClockClientClkVfPointInfoV1::default(); 255],
        }
    }
}

#[repr(C)]
#[derive(Debug, Copy, Clone, Default)]
struct ClockClientClkVfPointInfoV1 {
    type_: NvU32,
    b_voltage_based: NvU8,
    reserved: [NvU8; 16],
}

#[repr(C)]
#[derive(Copy, Clone)]
struct ClockClientClkVfPointsControlV1 {
    version: NvU32,
    vf_points_mask: [NvU32; 8],
    reserved: [NvU8; 32],
    vf_points: [ClockClientClkVfPointControlV1; 255],
}

impl Default for ClockClientClkVfPointsControlV1 {
    fn default() -> Self {
        Self {
            version: make_version::<Self>(1),
            vf_points_mask: [0; 8],
            reserved: [0; 32],
            vf_points: [ClockClientClkVfPointControlV1::default(); 255],
        }
    }
}

#[repr(C)]
#[derive(Copy, Clone, Default)]
struct ClockClientClkVfPointControlV1 {
    type_: NvU32,
    reserved: [NvU8; 16],
    data: ClockClientClkVfPointControlDataV1,
}

#[repr(C)]
#[derive(Copy, Clone)]
union ClockClientClkVfPointControlDataV1 {
    prog: ClockClientClkVfPointControlProgV1,
    reserved: [NvU8; 16],
}

impl Default for ClockClientClkVfPointControlDataV1 {
    fn default() -> Self {
        Self { reserved: [0; 16] }
    }
}

#[repr(C)]
#[derive(Debug, Copy, Clone, Default)]
struct ClockClientClkVfPointControlProgV1 {
    freq_offset_khz: NvS32,
}
