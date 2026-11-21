//! The graphics card the local voice will run on and its memory, asked of
//! Vulkan the way qwentts.cpp's ggml chooses it, so the number of requests
//! at once can fit the card before the server tries and fails.
//!
//! ggml (qwentts.cpp 2edb35f, ggml ef4555b) lists the discrete and
//! integrated GPUs in Vulkan's order, and `ggml_backend_init_best` takes
//! the first discrete one, else the first integrated one, else the first
//! that isn't a CPU. Its memory is the device-local heaps (every heap for
//! an integrated GPU, which shares the computer's memory):
//! `ggml_backend_vk_get_device_memory`.
//!
//! The Vulkan loader is opened with `libloading` (already a dependency)
//! rather than a Vulkan crate: a new dependency would change Cargo.lock and
//! with it every user's analysis fingerprint.

// The macOS server runs on Metal: nothing here asks Vulkan.
#![cfg_attr(target_os = "macos", allow(dead_code, unused_imports))]

use std::ffi::{CStr, c_char, c_void};
use std::sync::OnceLock;

/// The card the server will use.
#[derive(Clone, Debug)]
pub struct Gpu {
    pub name: String,
    /// Bytes of graphics memory (device-local heaps).
    pub memory: u64,
    /// Shares the computer's memory (an integrated GPU).
    pub integrated: bool,
}

static GPU: OnceLock<Option<Gpu>> = OnceLock::new();

/// The card, asking Vulkan the first time (tens of milliseconds, more on a
/// first start of the driver). `None`: no Vulkan, or no GPU.
pub fn gpu() -> Option<&'static Gpu> {
    GPU.get_or_init(probe).as_ref()
}

/// The card if already known, without waiting: the first call starts
/// asking in the background, for the panel.
pub fn known() -> Option<Option<&'static Gpu>> {
    if let Some(g) = GPU.get() {
        return Some(g.as_ref());
    }
    static STARTED: std::sync::Once = std::sync::Once::new();
    STARTED.call_once(|| {
        let _ = std::thread::Builder::new()
            .name("tts-gpu-probe".into())
            .spawn(|| {
                gpu();
            });
    });
    None
}

const DEVICE_TYPE_INTEGRATED: u32 = 1;
const DEVICE_TYPE_DISCRETE: u32 = 2;
const DEVICE_TYPE_CPU: u32 = 4;
const HEAP_DEVICE_LOCAL: u32 = 1;

#[repr(C)]
struct ApplicationInfo {
    s_type: u32,
    p_next: *const c_void,
    application_name: *const c_char,
    application_version: u32,
    engine_name: *const c_char,
    engine_version: u32,
    api_version: u32,
}

#[repr(C)]
struct InstanceCreateInfo {
    s_type: u32,
    p_next: *const c_void,
    flags: u32,
    application_info: *const ApplicationInfo,
    enabled_layer_count: u32,
    enabled_layer_names: *const *const c_char,
    enabled_extension_count: u32,
    enabled_extension_names: *const *const c_char,
}

#[repr(C)]
#[derive(Clone, Copy)]
struct MemoryType {
    property_flags: u32,
    heap_index: u32,
}

#[repr(C)]
#[derive(Clone, Copy)]
struct MemoryHeap {
    size: u64,
    flags: u32,
}

/// `VkPhysicalDeviceMemoryProperties` (520 bytes).
#[repr(C)]
struct MemoryProperties {
    memory_type_count: u32,
    memory_types: [MemoryType; 32],
    memory_heap_count: u32,
    memory_heaps: [MemoryHeap; 16],
}

/// `VkPhysicalDeviceProperties` is 824 bytes on 64-bit targets; only its
/// head is read: the device type at byte 16, the name at byte 20.
#[repr(C, align(8))]
struct DeviceProperties([u8; 1024]);

type Instance = *mut c_void;
type PhysicalDevice = *mut c_void;
type GetInstanceProcAddr = unsafe extern "system" fn(Instance, *const c_char) -> *const c_void;
type CreateInstance =
    unsafe extern "system" fn(*const InstanceCreateInfo, *const c_void, *mut Instance) -> i32;
type DestroyInstance = unsafe extern "system" fn(Instance, *const c_void);
type EnumeratePhysicalDevices =
    unsafe extern "system" fn(Instance, *mut u32, *mut PhysicalDevice) -> i32;
type GetPhysicalDeviceProperties = unsafe extern "system" fn(PhysicalDevice, *mut DeviceProperties);
type GetPhysicalDeviceMemoryProperties =
    unsafe extern "system" fn(PhysicalDevice, *mut MemoryProperties);

#[cfg(windows)]
const LOADER: &str = "vulkan-1.dll";
#[cfg(all(unix, not(target_os = "macos")))]
const LOADER: &str = "libvulkan.so.1";

#[cfg(target_os = "macos")]
fn probe() -> Option<Gpu> {
    // The macOS server runs on Metal, sharing the computer's memory.
    None
}

#[cfg(not(target_os = "macos"))]
fn probe() -> Option<Gpu> {
    // SAFETY: the loader is the system's Vulkan loader; every function is
    // called with the signature the Vulkan 1.0 specification gives it, and
    // the instance is destroyed before the library is dropped.
    unsafe {
        let lib = libloading::Library::new(LOADER).ok()?;
        let get_proc: libloading::Symbol<GetInstanceProcAddr> =
            lib.get(b"vkGetInstanceProcAddr\0").ok()?;
        let get_proc = *get_proc;
        let load = |instance: Instance, name: &CStr| {
            let f = get_proc(instance, name.as_ptr());
            (!f.is_null()).then_some(f)
        };
        let create: CreateInstance =
            std::mem::transmute(load(std::ptr::null_mut(), c"vkCreateInstance")?);
        let app = ApplicationInfo {
            s_type: 0, // VK_STRUCTURE_TYPE_APPLICATION_INFO
            p_next: std::ptr::null(),
            application_name: c"hanstat".as_ptr(),
            application_version: 0,
            engine_name: std::ptr::null(),
            engine_version: 0,
            api_version: 1 << 22, // 1.0
        };
        let info = InstanceCreateInfo {
            s_type: 1, // VK_STRUCTURE_TYPE_INSTANCE_CREATE_INFO
            p_next: std::ptr::null(),
            flags: 0,
            application_info: &app,
            enabled_layer_count: 0,
            enabled_layer_names: std::ptr::null(),
            enabled_extension_count: 0,
            enabled_extension_names: std::ptr::null(),
        };
        let mut instance: Instance = std::ptr::null_mut();
        if create(&info, std::ptr::null(), &mut instance) != 0 || instance.is_null() {
            return None;
        }
        let gpu = (|| {
            let enumerate: EnumeratePhysicalDevices =
                std::mem::transmute(load(instance, c"vkEnumeratePhysicalDevices")?);
            let properties: GetPhysicalDeviceProperties =
                std::mem::transmute(load(instance, c"vkGetPhysicalDeviceProperties")?);
            let memory: GetPhysicalDeviceMemoryProperties =
                std::mem::transmute(load(instance, c"vkGetPhysicalDeviceMemoryProperties")?);
            let mut n = 0u32;
            if enumerate(instance, &mut n, std::ptr::null_mut()) != 0 || n == 0 {
                return None;
            }
            let mut devices = vec![std::ptr::null_mut(); n as usize];
            // VK_INCOMPLETE (5) still fills what fits.
            if enumerate(instance, &mut n, devices.as_mut_ptr()) < 0 {
                return None;
            }
            devices.truncate(n as usize);
            let listed: Vec<(PhysicalDevice, u32, String)> = devices
                .into_iter()
                .map(|d| {
                    let mut p = DeviceProperties([0; 1024]);
                    properties(d, &mut p);
                    let kind = u32::from_ne_bytes(p.0[16..20].try_into().unwrap());
                    let name = CStr::from_bytes_until_nul(&p.0[20..276])
                        .map(|s| s.to_string_lossy().into_owned())
                        .unwrap_or_default();
                    (d, kind, name)
                })
                .collect();
            let chosen = listed
                .iter()
                .find(|d| d.1 == DEVICE_TYPE_DISCRETE)
                .or_else(|| listed.iter().find(|d| d.1 == DEVICE_TYPE_INTEGRATED))
                .or_else(|| listed.iter().find(|d| d.1 != DEVICE_TYPE_CPU))?;
            let integrated = chosen.1 == DEVICE_TYPE_INTEGRATED;
            let mut m: MemoryProperties = std::mem::zeroed();
            memory(chosen.0, &mut m);
            let heaps = &m.memory_heaps[..(m.memory_heap_count as usize).min(16)];
            let total = heaps
                .iter()
                .filter(|h| integrated || h.flags & HEAP_DEVICE_LOCAL != 0)
                .map(|h| h.size)
                .sum();
            Some(Gpu {
                name: chosen.2.clone(),
                memory: total,
                integrated,
            })
        })();
        if let Some(destroy) = load(instance, c"vkDestroyInstance") {
            let destroy: DestroyInstance = std::mem::transmute(destroy);
            destroy(instance, std::ptr::null());
        }
        gpu
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn struct_sizes_match_vulkan() {
        assert_eq!(std::mem::size_of::<super::MemoryProperties>(), 520);
        assert_eq!(
            std::mem::offset_of!(super::MemoryProperties, memory_heaps),
            264
        );
        assert!(std::mem::size_of::<super::DeviceProperties>() >= 824);
    }

    /// Prints what this computer's card reports (`--ignored --nocapture`).
    #[test]
    #[ignore = "asks this computer's Vulkan driver"]
    fn probe_this_gpu() {
        let g = super::gpu().expect("a Vulkan GPU");
        println!("{g:?} ({:.2} GiB)", g.memory as f64 / (1u64 << 30) as f64);
        assert!(g.memory > 0 && !g.name.is_empty());
    }
}
