//! The renderer of the window on Linux under X11 (ADR 0705).
//!
//! gpui tries a Vulkan adapter before an OpenGL adapter of the same kind, and it has no
//! setting that excludes an adapter. The software Vulkan driver of Mesa before version 25
//! presents a black window under X11. The Vulkan loader reads its list of drivers from the
//! environment when the process loads it, so the binary starts itself again with an empty
//! list when that driver is the one that gpui would use; gpui then draws with OpenGL.

use std::os::unix::process::CommandExt;

use gpui_wgpu::wgpu;

/// Set in the process that this check started, so that it runs once.
const CHECKED: &str = "JUNTO_RENDERER_CHECKED";
/// The first major version of Mesa whose software Vulkan driver was seen to draw (ADR 0705).
const FIRST_MESA_THAT_DRAWS: u32 = 25;

/// What the check reads of a Vulkan adapter.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct VulkanAdapter {
    /// The adapter computes on the processor.
    pub software: bool,
    pub driver: String,
    pub driver_info: String,
}

fn mesa_major(driver_info: &str) -> Option<u32> {
    let version = driver_info.split_once("Mesa ")?.1;
    let digits = version
        .find(|character: char| !character.is_ascii_digit())
        .map_or(version, |end| &version[..end]);
    digits.parse().ok()
}

/// Whether gpui would draw with the software Vulkan driver of a Mesa that presents a black
/// window: no Vulkan adapter is a graphics processor, and one is that driver.
pub fn software_vulkan_presents_nothing(adapters: &[VulkanAdapter]) -> bool {
    adapters.iter().all(|adapter| adapter.software)
        && adapters.iter().any(|adapter| {
            adapter.driver == "llvmpipe"
                && mesa_major(&adapter.driver_info)
                    .is_some_and(|major| major < FIRST_MESA_THAT_DRAWS)
        })
}

fn vulkan_adapters() -> Vec<VulkanAdapter> {
    let instance = wgpu::Instance::new(wgpu::InstanceDescriptor {
        backends: wgpu::Backends::VULKAN,
        ..wgpu::InstanceDescriptor::new_without_display_handle()
    });
    let Ok(runtime) = tokio::runtime::Builder::new_current_thread().build() else {
        return Vec::new();
    };
    runtime
        .block_on(instance.enumerate_adapters(wgpu::Backends::VULKAN))
        .iter()
        .map(|adapter| {
            let info = adapter.get_info();
            VulkanAdapter {
                software: info.device_type == wgpu::DeviceType::Cpu,
                driver: info.driver,
                driver_info: info.driver_info,
            }
        })
        .collect()
}

/// Runs before the application object exists and before any thread starts. Returns when
/// the renderer that gpui will choose can stay; otherwise the process is replaced.
pub fn choose_before_the_first_window() {
    if std::env::var_os(CHECKED).is_some() || gpui::guess_compositor() != "X11" {
        return;
    }
    let adapters = vulkan_adapters();
    if !software_vulkan_presents_nothing(&adapters) {
        return;
    }
    tracing::warn!(
        ?adapters,
        "the software Vulkan driver of this Mesa presents a black window under X11; \
         starting again with the OpenGL renderer"
    );
    let Ok(program) = std::env::current_exe() else {
        return;
    };
    // An empty driver list: the loader finds no Vulkan adapter, and gpui uses OpenGL.
    let failure = std::process::Command::new(program)
        .args(std::env::args_os().skip(1))
        .env(CHECKED, "1")
        .env("VK_DRIVER_FILES", "/dev/null")
        .env("VK_ICD_FILENAMES", "/dev/null")
        .exec();
    tracing::warn!(%failure, "the application could not start again; the window can stay black");
}

#[cfg(test)]
mod tests {
    use super::*;

    fn adapter(software: bool, driver: &str, driver_info: &str) -> VulkanAdapter {
        VulkanAdapter {
            software,
            driver: driver.to_owned(),
            driver_info: driver_info.to_owned(),
        }
    }

    #[test]
    fn only_the_software_driver_of_an_old_mesa_without_a_graphics_processor_is_replaced() {
        let old = adapter(true, "llvmpipe", "Mesa 22.3.6 (LLVM 15.0.6)");
        let new = adapter(true, "llvmpipe", "Mesa 25.0.7-2~bpo12+1 (LLVM 15.0.6)");
        let hardware = adapter(false, "radv", "Mesa 22.3.6");
        assert!(software_vulkan_presents_nothing(std::slice::from_ref(&old)));
        assert!(!software_vulkan_presents_nothing(&[new]));
        assert!(!software_vulkan_presents_nothing(std::slice::from_ref(
            &hardware
        )));
        // gpui prefers the graphics processor: the software driver beside it is not used.
        assert!(!software_vulkan_presents_nothing(&[hardware, old]));
        assert!(!software_vulkan_presents_nothing(&[]));
        assert!(!software_vulkan_presents_nothing(&[adapter(
            true,
            "llvmpipe",
            "an unknown version"
        )]));
        assert!(!software_vulkan_presents_nothing(&[adapter(
            true,
            "SwiftShader driver",
            "Mesa 22.3.6"
        )]));
    }
}
