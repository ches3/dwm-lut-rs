use std::collections::BTreeMap;
use std::collections::btree_map::Entry;
use std::mem::size_of;
use std::sync::LazyLock;

use parking_lot::{Mutex, MutexGuard};
use windows::Win32::Graphics::Direct3D11::{D3D11_TEXTURE2D_DESC, ID3D11Texture2D};
use windows::core::{GUID, Interface};

use super::frame_plan::prepare_frame_input;
use super::resources::{DeviceResources, FrameResources};
use super::{FrameAcquireError, FrameSize, LutApplyOutcome, LutApplyStatus, LutRenderFailure};
use crate::dwmcore::DirtyRect;
use crate::state::LutAssignment;
use dwm_lut_payload::MonitorIdentity;
use dwm_lut_profile::D3D11ResourceAccess;

static RENDERER: LazyLock<Mutex<Renderer>> = LazyLock::new(|| Mutex::new(Renderer::new()));

const BACK_BUFFER_ID_PRIVATE_DATA_GUID: GUID =
    GUID::from_u128(0x6ca95369_322a_4ee3_8515_fec2020a7416);

pub(crate) struct RendererGuard {
    renderer: MutexGuard<'static, Renderer>,
}

struct Renderer {
    devices: BTreeMap<usize, DeviceResources>,
}

#[derive(Clone, Copy)]
struct FrameRequest {
    overlay_swap_chain: usize,
    d3d11_resource_access: D3D11ResourceAccess,
    monitor_identity: MonitorIdentity,
}

impl Renderer {
    fn new() -> Self {
        Self {
            devices: BTreeMap::new(),
        }
    }

    unsafe fn apply_lut(
        &mut self,
        request: FrameRequest,
        dirty_rects: &[DirtyRect],
        assignments: &[LutAssignment],
    ) -> Result<LutApplyOutcome, FrameAcquireError> {
        let back_buffer = unsafe {
            crate::dwmcore::resolve_d3d11_texture2d(
                request.overlay_swap_chain,
                request.d3d11_resource_access,
            )
        }
        .map_err(FrameAcquireError::BackBuffer)?;
        let device = (unsafe { back_buffer.GetDevice() }).map_err(|_| FrameAcquireError::Device)?;
        let context =
            (unsafe { device.GetImmediateContext() }).map_err(|_| FrameAcquireError::Context)?;
        let mut desc = D3D11_TEXTURE2D_DESC::default();
        unsafe {
            back_buffer.GetDesc(&mut desc);
        }

        let back_buffer_id = get_or_create_back_buffer_id(&back_buffer)
            .ok_or(FrameAcquireError::BackBufferIdentity)?;
        let size = FrameSize {
            width: desc.Width,
            height: desc.Height,
        };
        let input = match prepare_frame_input(
            assignments,
            request.monitor_identity,
            desc.Format.0 as u32,
            size,
            dirty_rects,
        ) {
            Ok(input) => input,
            Err(skip) => {
                return Ok(LutApplyOutcome {
                    present_dirty_rect: None,
                    status: LutApplyStatus::Skipped(skip.reason),
                    dxgi_format: desc.Format.0 as u32,
                    frame_size: size,
                    lut_index: skip.lut_index,
                    #[cfg(debug_assertions)]
                    back_buffer_id,
                });
            }
        };

        let device_id = device.as_raw() as usize;
        for (other_device_id, resources) in &mut self.devices {
            if *other_device_id != device_id {
                resources.remove_swap_chain(request.overlay_swap_chain);
            }
        }

        let lut = input.lut;
        let result = match self.resources_for_device(device_id, &device, assignments) {
            Some(resources) => resources.render_frame(
                request.overlay_swap_chain,
                back_buffer_id,
                FrameResources {
                    device: &device,
                    context: &context,
                    back_buffer: &back_buffer,
                    size,
                },
                assignments,
                input,
            ),
            None => Err(LutRenderFailure::DeviceResourcesCreateFailed),
        };
        let (present_dirty_rect, status) = match result {
            Ok(present_dirty_rect) => (present_dirty_rect, LutApplyStatus::Applied),
            Err(reason) => (None, LutApplyStatus::Failed(reason)),
        };

        Ok(LutApplyOutcome {
            present_dirty_rect,
            status,
            dxgi_format: lut.back_buffer_format.dxgi_format(),
            frame_size: size,
            lut_index: Some(lut.lut_index),
            #[cfg(debug_assertions)]
            back_buffer_id,
        })
    }

    fn clear_resources(&mut self) {
        self.devices.clear();
    }

    fn resources_for_device(
        &mut self,
        device_id: usize,
        device: &windows::Win32::Graphics::Direct3D11::ID3D11Device,
        assignments: &[LutAssignment],
    ) -> Option<&mut DeviceResources> {
        match self.devices.entry(device_id) {
            Entry::Occupied(entry) => Some(entry.into_mut()),
            Entry::Vacant(entry) => {
                let resources = DeviceResources::create(device, assignments)?;
                Some(entry.insert(resources))
            }
        }
    }
}

fn get_or_create_back_buffer_id(back_buffer: &ID3D11Texture2D) -> Option<u128> {
    let mut id = GUID::zeroed();
    let mut size = size_of::<GUID>() as u32;
    let get_result = unsafe {
        back_buffer.GetPrivateData(
            &BACK_BUFFER_ID_PRIVATE_DATA_GUID,
            &mut size,
            Some((&mut id as *mut GUID).cast()),
        )
    };
    if get_result.is_ok() && size == size_of::<GUID>() as u32 && id != GUID::zeroed() {
        return Some(id.to_u128());
    }

    let id = GUID::new().ok()?;
    unsafe {
        back_buffer.SetPrivateData(
            &BACK_BUFFER_ID_PRIVATE_DATA_GUID,
            size_of::<GUID>() as u32,
            Some((&id as *const GUID).cast()),
        )
    }
    .ok()?;
    Some(id.to_u128())
}

impl RendererGuard {
    pub(crate) unsafe fn apply_lut(
        &mut self,
        overlay_swap_chain: usize,
        d3d11_resource: D3D11ResourceAccess,
        monitor_identity: MonitorIdentity,
        dirty_rects: &[DirtyRect],
        assignments: &[LutAssignment],
    ) -> Result<LutApplyOutcome, FrameAcquireError> {
        unsafe {
            self.renderer.apply_lut(
                FrameRequest {
                    overlay_swap_chain,
                    d3d11_resource_access: d3d11_resource,
                    monitor_identity,
                },
                dirty_rects,
                assignments,
            )
        }
    }

    pub(crate) fn clear_resources(&mut self) {
        self.renderer.clear_resources();
    }
}

pub(crate) fn try_lock_renderer() -> Option<RendererGuard> {
    RENDERER
        .try_lock()
        .map(|renderer| RendererGuard { renderer })
}

pub(crate) fn lock_renderer() -> RendererGuard {
    RendererGuard {
        renderer: RENDERER.lock(),
    }
}
