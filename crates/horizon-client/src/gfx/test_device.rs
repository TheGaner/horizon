//! One GPU backend discovery for all renderer unit tests.
//!
//! Creating independent `wgpu::Instance`s from concurrent tests repeatedly
//! enters Vulkan loader / ICD discovery; it crashed on an NVIDIA Linux host in
//! `vkEnumerateInstanceExtensionProperties`. An adapter belongs to this
//! process-wide test fixture, while each test gets its own Device and Queue,
//! keeping allocations, encoder state and error scopes isolated.
use std::sync::OnceLock;

struct TestGpu {
    // Keep the instance alive for the adapter lifetime.
    _instance: wgpu::Instance,
    adapter: wgpu::Adapter,
}

/// CI jobs with an actual GPU can require GPU-backed tests instead of
/// silently treating a driver/device failure as a skipped test.
fn gpu_required() -> bool {
    std::env::var("HORIZON_REQUIRE_GPU_TESTS").is_ok_and(|value| value == "1")
}

fn adapter() -> Option<&'static wgpu::Adapter> {
    static GPU: OnceLock<Result<TestGpu, String>> = OnceLock::new();
    match GPU.get_or_init(|| {
        let instance = wgpu::Instance::new(wgpu::InstanceDescriptor::new_without_display_handle());
        let adapter = pollster::block_on(instance.request_adapter(&Default::default()))
            .map_err(|error| format!("GPU adapter unavailable: {error}"))?;
        eprintln!(
            "Horizon GPU tests: {} ({:?})",
            adapter.get_info().name,
            adapter.get_info().backend
        );
        Ok(TestGpu {
            _instance: instance,
            adapter,
        })
    }) {
        Ok(gpu) => Some(&gpu.adapter),
        Err(error) if gpu_required() => panic!("Horizon GPU tests required: {error}"),
        Err(error) => {
            eprintln!("Horizon GPU tests skipped: {error}");
            None
        }
    }
}

pub(super) fn device() -> Option<(wgpu::Device, wgpu::Queue)> {
    device_with(wgpu::Features::empty())
}

/// Negotiate the requested features on the shared adapter, but construct an
/// independent logical device and queue for each test.
pub(super) fn device_with(features: wgpu::Features) -> Option<(wgpu::Device, wgpu::Queue)> {
    let adapter = adapter()?;
    let required_features = adapter.features() & features;
    match pollster::block_on(adapter.request_device(&wgpu::DeviceDescriptor {
        required_features,
        ..Default::default()
    })) {
        Ok(device) => Some(device),
        Err(error) if gpu_required() => panic!("Horizon GPU device required: {error}"),
        Err(error) => {
            eprintln!("Horizon GPU test skipped: failed to create device: {error}");
            None
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The loader and ICD enumeration must happen at most once, even with
    /// concurrent users in different renderer modules. This also covers the
    /// no-GPU case without making a CI runner's hardware a prerequisite.
    #[test]
    fn concurrent_adapter_access_is_the_same_instance() {
        let threads: Vec<_> = (0..16)
            .map(|_| {
                std::thread::spawn(|| adapter().map(|adapter| std::ptr::from_ref(adapter) as usize))
            })
            .collect();
        let results: Vec<_> = threads.into_iter().map(|t| t.join().unwrap()).collect();
        assert!(results.windows(2).all(|pair| pair[0] == pair[1]));
    }
}
