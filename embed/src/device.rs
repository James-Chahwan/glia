//! The embedding device (FB.14): where a bge-small session runs, how the
//! device is chosen, the batch it is fed, and the adapter it reports.
//!
//! SELECTION, in order ([`requested`] reads [`DEVICE_ENV`]):
//! - `GLIA_EMBED_DEVICE=cpu`: the CPU, in-process; fallback `override`.
//! - `GLIA_EMBED_DEVICE=webgpu` / `=cuda`: that device, in the worker; a
//!   device this build (or its worker) lacks, or a session that fails to
//!   start, is a hard error naming it. CUDA is only ever explicit.
//! - unset (or empty): WebGPU in the worker when built, else the CPU with
//!   fallback `not_built`; a WebGPU session that fails to commit (no adapter,
//!   no Vulkan ICD) falls back to the CPU, `no_adapter:<first error line>`.
//!
//! After the start, a GPU failure always falls back to the CPU for the rest
//! of the run, never an error: a worker that dies is `gpu_crash:<signal |
//! status>`, a CUDA out-of-memory that survives one halved batch is `oom`
//! (see [`crate::worker`]). ORT without `error_on_failure` would run a
//! failed provider's graph silently on the CPU; FastEmbedder never omits it.
//!
//! [`Device::WebGpu`] is fed [`batch`] = 8 texts per session run: the
//! 2026-10-25 spike (GTX 1660 Super, Dawn over Vulkan) measured 30.4 texts/s
//! at batch 8, throughput halving above it and the PROCESS ABORTING
//! (`free(): invalid pointer`) at batch >= 64. CUDA takes 32, the CPU
//! [`crate::CHUNK`].

use std::fmt;

/// The environment variable [`requested`] reads: `cpu`, `webgpu` or `cuda`;
/// unset or empty is automatic selection.
pub const DEVICE_ENV: &str = "GLIA_EMBED_DEVICE";

/// The device a session runs on, recorded on every vector row it writes
/// (`device` in `.glia/vectors.jsonl`; an absent field reads as `cpu`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
#[non_exhaustive]
pub enum Device {
    /// ONNX Runtime's CPU provider, in the calling process.
    Cpu,
    /// ORT's WebGPU provider (Dawn over Vulkan / Metal / D3D12), in the worker.
    WebGpu,
    /// ORT's CUDA provider (feature `gpu-cuda`, explicit only), in the worker.
    Cuda,
}

/// Every device, in [`Device::as_str`] order.
pub const DEVICES: [Device; 3] = [Device::Cpu, Device::WebGpu, Device::Cuda];

impl Device {
    /// The spelling [`DEVICE_ENV`], the marker and vector rows use.
    pub fn as_str(self) -> &'static str {
        match self {
            Device::Cpu => "cpu",
            Device::WebGpu => "webgpu",
            Device::Cuda => "cuda",
        }
    }

    /// The device `s` spells ([`Device::as_str`]), ignoring ASCII case.
    pub fn parse(s: &str) -> Option<Device> {
        DEVICES
            .into_iter()
            .find(|d| d.as_str().eq_ignore_ascii_case(s.trim()))
    }

    /// Whether THIS binary can run a session on the device. A worker binary
    /// reports its own list in its hello frame ([`crate::worker`]).
    pub fn built(self) -> bool {
        match self {
            Device::Cpu => crate::ORT_BUILT,
            Device::WebGpu => WEBGPU_BUILT,
            Device::Cuda => CUDA_BUILT,
        }
    }
}

impl fmt::Display for Device {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

/// Whether this binary carries ORT's WebGPU provider: feature `gpu-webgpu` on
/// a target ORT ships WebGPU binaries for (x86_64 Linux and Windows, aarch64
/// macOS).
pub const WEBGPU_BUILT: bool = cfg!(all(
    feature = "gpu-webgpu",
    any(
        all(target_os = "linux", target_arch = "x86_64"),
        all(target_os = "windows", target_arch = "x86_64"),
        all(target_os = "macos", target_arch = "aarch64")
    )
));

/// Whether this binary carries ORT's CUDA provider: feature `gpu-cuda` on
/// x86_64 Linux or Windows.
pub const CUDA_BUILT: bool = cfg!(all(
    feature = "gpu-cuda",
    any(
        all(target_os = "linux", target_arch = "x86_64"),
        all(target_os = "windows", target_arch = "x86_64")
    )
));

/// The devices this binary can run, in [`DEVICES`] order.
pub fn built_devices() -> Vec<Device> {
    DEVICES.into_iter().filter(|d| d.built()).collect()
}

/// The device [`DEVICE_ENV`] asks for: None when unset or empty (automatic),
/// Err for any other spelling.
pub fn requested() -> Result<Option<Device>, String> {
    let v = std::env::var(DEVICE_ENV).unwrap_or_default();
    parse_request(&v)
}

/// [`requested`] over a given value.
pub fn parse_request(v: &str) -> Result<Option<Device>, String> {
    if v.trim().is_empty() {
        return Ok(None);
    }
    Device::parse(v).map(Some).ok_or_else(|| {
        let known: Vec<&str> = DEVICES.iter().map(|d| d.as_str()).collect();
        format!(
            "{DEVICE_ENV}={v}: unknown device, expected one of {} (or unset for automatic)",
            known.join(", ")
        )
    })
}

/// Texts per session run on `d` (module doc: WebGPU's pin of 8 is the
/// spike's abort at >= 64).
pub fn batch(d: Device) -> usize {
    match d {
        Device::Cpu => crate::CHUNK,
        Device::WebGpu => 8,
        Device::Cuda => 32,
    }
}

/// The adapter a session runs on, as the marker prints it. ORT reports none,
/// so it is probed: [`gpu_adapter`] for a GPU, [`cpu_adapter`] for the CPU.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub struct Adapter {
    /// The adapter's name (`NVIDIA GeForce GTX 1660 SUPER`), `unknown` when
    /// no probe answered.
    pub name: String,
    /// Its driver version (`580.95.05`), `none` for the CPU, `unknown` when
    /// no probe answered.
    pub driver: String,
}

impl Adapter {
    /// An adapter of this name and driver.
    pub fn new(name: impl Into<String>, driver: impl Into<String>) -> Self {
        Adapter {
            name: name.into(),
            driver: driver.into(),
        }
    }
}

/// The CPU as an adapter: `/proc/cpuinfo`'s first `model name` (Linux), else
/// `cpu`; driver `none`.
pub fn cpu_adapter() -> Adapter {
    let name = std::fs::read_to_string("/proc/cpuinfo")
        .ok()
        .and_then(|s| {
            s.lines()
                .find(|l| l.starts_with("model name"))
                .and_then(|l| l.split_once(':'))
                .map(|(_, v)| v.trim().to_string())
        })
        .filter(|n| !n.is_empty())
        .unwrap_or_else(|| "cpu".to_string());
    Adapter::new(name, "none")
}

/// The GPU a WebGPU / CUDA session most likely runs on: Vulkan's first
/// discrete (then integrated, virtual, other; never a CPU-type) physical
/// device, with the Vulkan 1.2 driver-info string as its driver; else the
/// NVIDIA kernel module's version from `/proc/driver/nvidia/version` under the
/// name `unknown`; else `unknown` / `unknown`. Vulkan is loaded at run time
/// (`ash`, `loaded`): no binary links libvulkan, and a host without a loader
/// or an ICD (`VK_DRIVER_FILES=/nonexistent.json`) simply answers nothing.
/// Runs in the worker, so a misbehaving ICD can only take the worker down.
pub fn gpu_adapter() -> Adapter {
    if let Some(a) = vulkan_adapter() {
        return a;
    }
    let driver = std::fs::read_to_string("/proc/driver/nvidia/version")
        .ok()
        .and_then(|s| nvidia_driver(&s))
        .unwrap_or_else(|| "unknown".to_string());
    Adapter::new("unknown", driver)
}

/// The version in `/proc/driver/nvidia/version`'s first line (`NVRM version:
/// NVIDIA UNIX x86_64 Kernel Module  580.95.05  Thu ..`): its first token made
/// of digits and dots with at least one dot.
pub fn nvidia_driver(proc_version: &str) -> Option<String> {
    proc_version
        .lines()
        .next()?
        .split_whitespace()
        .find(|t| {
            t.contains('.')
                && t.chars().all(|c| c.is_ascii_digit() || c == '.')
                && t.starts_with(|c: char| c.is_ascii_digit())
        })
        .map(str::to_string)
}

#[cfg(any(feature = "gpu-webgpu", feature = "gpu-cuda"))]
#[cfg(not(all(target_os = "macos", target_arch = "x86_64")))]
fn vulkan_adapter() -> Option<Adapter> {
    use ash::vk;
    // SAFETY: Entry::load dlopens the system Vulkan loader; every handle made
    // below is destroyed before return and none escapes this function.
    unsafe {
        let entry = ash::Entry::load().ok()?;
        let app = vk::ApplicationInfo::default().api_version(vk::make_api_version(0, 1, 2, 0));
        let info = vk::InstanceCreateInfo::default().application_info(&app);
        let instance = entry.create_instance(&info, None).ok()?;
        let devices = instance.enumerate_physical_devices().unwrap_or_default();
        let rank = |t: vk::PhysicalDeviceType| match t {
            vk::PhysicalDeviceType::DISCRETE_GPU => Some(0),
            vk::PhysicalDeviceType::INTEGRATED_GPU => Some(1),
            vk::PhysicalDeviceType::VIRTUAL_GPU => Some(2),
            vk::PhysicalDeviceType::CPU => None,
            _ => Some(3),
        };
        let mut best: Option<(u8, usize, Adapter)> = None;
        for (i, &pd) in devices.iter().enumerate() {
            let props = instance.get_physical_device_properties(pd);
            let Some(r) = rank(props.device_type) else {
                continue;
            };
            let name = props
                .device_name_as_c_str()
                .map(|c| c.to_string_lossy().into_owned())
                .unwrap_or_else(|_| "unknown".to_string());
            let mut driver = String::new();
            if props.api_version >= vk::make_api_version(0, 1, 2, 0) {
                let mut dp = vk::PhysicalDeviceDriverProperties::default();
                let mut p2 = vk::PhysicalDeviceProperties2::default().push_next(&mut dp);
                instance.get_physical_device_properties2(pd, &mut p2);
                if let Ok(s) = dp.driver_info_as_c_str() {
                    driver = s.to_string_lossy().trim().to_string();
                }
            }
            if driver.is_empty() {
                driver = vulkan_version(props.vendor_id, props.driver_version);
            }
            let cand = (r, i, Adapter::new(name, driver));
            if best.as_ref().is_none_or(|b| (cand.0, cand.1) < (b.0, b.1)) {
                best = Some(cand);
            }
        }
        instance.destroy_instance(None);
        best.map(|(_, _, a)| a)
    }
}

#[cfg(not(all(
    any(feature = "gpu-webgpu", feature = "gpu-cuda"),
    not(all(target_os = "macos", target_arch = "x86_64"))
)))]
fn vulkan_adapter() -> Option<Adapter> {
    None
}

/// A Vulkan `driverVersion` in its vendor's encoding: NVIDIA (0x10de) packs
/// 10.8.8.6 bits, every other vendor the standard 10.10.12 of
/// `VK_MAKE_API_VERSION`.
pub fn vulkan_version(vendor_id: u32, v: u32) -> String {
    if vendor_id == 0x10de {
        format!(
            "{}.{}.{}",
            (v >> 22) & 0x3ff,
            (v >> 14) & 0xff,
            (v >> 6) & 0xff
        )
    } else {
        format!("{}.{}.{}", v >> 22, (v >> 12) & 0x3ff, v & 0xfff)
    }
}

/// The marker line, once per embedder when its device is settled and once
/// more when it falls back: `[embed] device=<adapter name> backend=<cpu |
/// webgpu | cuda> driver=<v> fallback=<reason | none> batch=<n>`.
pub fn marker(adapter: &Adapter, backend: Device, fallback: Option<&str>, batch: usize) -> String {
    format!(
        "[embed] device={} backend={} driver={} fallback={} batch={batch}",
        adapter.name,
        backend.as_str(),
        adapter.driver,
        fallback.unwrap_or("none")
    )
}

/// `reason` cut to its first non-empty line (a fallback reason stays one
/// marker field).
pub fn first_line(reason: &str) -> String {
    reason
        .lines()
        .map(str::trim)
        .find(|l| !l.is_empty())
        .unwrap_or("")
        .to_string()
}

/// A fallback reason made short: [`first_line`], from ORT's own message when
/// it wraps an assertion (`.. status == Success was false. Failed to get a
/// WebGPU adapter: No supported adapters` -> the part after `was false. `),
/// at most 160 characters.
pub fn short_reason(reason: &str) -> String {
    let line = first_line(reason);
    let tail = match line.rfind(" was false. ") {
        Some(at) => &line[at + " was false. ".len()..],
        None => line.as_str(),
    };
    tail.chars()
        .take(160)
        .collect::<String>()
        .trim()
        .to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn devices_round_trip() {
        for d in DEVICES {
            assert_eq!(Device::parse(d.as_str()), Some(d));
        }
        assert_eq!(Device::parse("WebGPU"), Some(Device::WebGpu));
        assert_eq!(Device::parse("metal"), None);
    }

    #[test]
    fn requests_parse() {
        assert_eq!(parse_request(""), Ok(None));
        assert_eq!(parse_request("  "), Ok(None));
        assert_eq!(parse_request("cpu"), Ok(Some(Device::Cpu)));
        let e = parse_request("tpu").expect_err("tpu");
        assert!(e.contains("GLIA_EMBED_DEVICE=tpu"), "{e}");
    }

    #[test]
    fn nvidia_versions() {
        let s = "NVRM version: NVIDIA UNIX Open Kernel Module for x86_64  580.95.05  \
                 Release Build  (dvs-builder@U16-I1-N08-12-3)  Sat Sep 13 00:00:00 UTC 2025\n\
                 GCC version:  gcc version 15.2.1";
        assert_eq!(nvidia_driver(s).as_deref(), Some("580.95.05"));
        assert_eq!(nvidia_driver("nothing here"), None);
        // NVIDIA's packing of 580.95.5: 580 << 22 | 95 << 14 | 5 << 6.
        assert_eq!(
            vulkan_version(0x10de, (580 << 22) | (95 << 14) | (5 << 6)),
            "580.95.5"
        );
        assert_eq!(vulkan_version(0x1002, (2 << 22) | (3 << 12) | 7), "2.3.7");
    }

    #[test]
    fn marker_shape() {
        let a = Adapter::new("NVIDIA GeForce GTX 1660 SUPER", "580.95.05");
        assert_eq!(
            marker(&a, Device::WebGpu, None, 8),
            "[embed] device=NVIDIA GeForce GTX 1660 SUPER backend=webgpu driver=580.95.05 \
             fallback=none batch=8"
        );
        assert_eq!(first_line("\n  a b\nc"), "a b");
        assert_eq!(
            short_reason(
                "cannot load m on webgpu: Failed to build ONNX session: /x/webgpu_context.cc:67 \
                 auto f() const status == wgpu::RequestAdapterStatus::Success was false. Failed \
                 to get a WebGPU adapter: No supported adapters\nmore"
            ),
            "Failed to get a WebGPU adapter: No supported adapters"
        );
        assert_eq!(short_reason(&"x".repeat(400)).len(), 160);
    }
}
