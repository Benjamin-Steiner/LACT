//! Platform-facing compatibility types used by the wire schema.
//!
//! Linux keeps using the native amdgpu-sysfs representations so the existing
//! daemon remains wire-compatible. Windows uses small serde-compatible schema
//! types until the native ADLX backend supplies equivalent information.

#[cfg(unix)]
pub use amdgpu_sysfs::{
    gpu_handle::{
        PerformanceLevel, PowerLevelId, PowerLevelKind,
        fan_control::FanInfo,
        overdrive::ClocksTableGen as AmdClocksTableGen,
        power_profile_mode::PowerProfileModesTable,
    },
    hw_mon::Temperature,
};

#[cfg(windows)]
mod windows {
    use serde::{Deserialize, Serialize};

    pub type AmdClocksTableGen = serde_json::Value;
    pub type PowerProfileModesTable = serde_json::Value;
    pub type PowerLevelId = u8;

    #[derive(Serialize, Deserialize, Debug, Clone, Copy, PartialEq, Eq, Hash)]
    #[serde(rename_all = "snake_case")]
    pub enum PowerLevelKind {
        Core,
        Memory,
        Pcie,
    }

    #[derive(Serialize, Deserialize, Debug, Clone, Copy, PartialEq, Eq)]
    #[serde(rename_all = "snake_case")]
    pub enum PerformanceLevel {
        Auto,
        Low,
        High,
        Manual,
        ProfileStandard,
        ProfileMinSclk,
        ProfileMinMclk,
        ProfilePeak,
    }

    #[derive(Serialize, Deserialize, Debug, Clone, Copy, Default, PartialEq, Eq)]
    pub struct FanInfo {
        pub current: u32,
        pub allowed_range: Option<(u32, u32)>,
    }

    #[derive(Serialize, Deserialize, Debug, Clone, Copy, Default, PartialEq)]
    pub struct Temperature {
        pub current: Option<f32>,
        pub crit: Option<f32>,
        pub crit_hyst: Option<f32>,
    }
}

#[cfg(windows)]
pub use windows::*;
