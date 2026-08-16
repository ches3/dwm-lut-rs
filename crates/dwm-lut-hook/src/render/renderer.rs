use std::collections::btree_map::Entry;
use std::collections::{BTreeMap, HashSet};
use std::ffi::c_void;
use std::mem::size_of;
use std::sync::LazyLock;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Duration;

use parking_lot::{Condvar, Mutex, MutexGuard};
use windows::Win32::Graphics::Direct3D::ID3DDestructionNotifier;
use windows::Win32::Graphics::Direct3D11::{
    D3D11_TEXTURE2D_DESC, ID3D11Device, ID3D11DeviceContext, ID3D11Texture2D,
};
use windows::core::{GUID, Interface};

use super::frame_plan::{FrameOperation, prepare_frame, select_frame_operation};
use super::resources::{DeviceResources, FrameEffect, FrameResources};
use super::{
    FrameAcquireError, FrameOutcome, FrameProcessingError, FrameSize, FrameStatus, RestoreToken,
    SwapChainRoute,
};
use crate::dwmcore::DirtyRect;
use crate::state::LutAssignment;
use dwm_lut_payload::MonitorIdentity;
use dwm_lut_profile::D3D11ResourceAccess;

static RENDERER: LazyLock<Mutex<Renderer>> = LazyLock::new(|| Mutex::new(Renderer::new()));
static RESTORE_PROGRESS: Condvar = Condvar::new();
static DESTROYED_BACK_BUFFERS: LazyLock<Mutex<Vec<usize>>> =
    LazyLock::new(|| Mutex::new(Vec::new()));
static BACK_BUFFER_PRIVATE_DATA_GUID: LazyLock<Option<GUID>> = LazyLock::new(|| GUID::new().ok());
static NEXT_BACK_BUFFER_ID: AtomicUsize = AtomicUsize::new(1);

pub(crate) struct RendererGuard {
    renderer: MutexGuard<'static, Renderer>,
}

struct Renderer {
    devices: BTreeMap<usize, DeviceResources>,
    restore_session: Option<RestoreSession>,
    next_restore_session_id: usize,
}

struct RestoreSession {
    id: usize,
    routes: HashSet<SwapChainRoute>,
}

#[derive(Clone, Copy)]
struct FrameRequest {
    overlay_swap_chain: usize,
    d3d11_resource_access: D3D11ResourceAccess,
    monitor_identity: MonitorIdentity,
}

struct AcquiredFrame {
    swap_chain: usize,
    device_id: usize,
    device: ID3D11Device,
    context: ID3D11DeviceContext,
    back_buffer: ID3D11Texture2D,
    back_buffer_id: usize,
    size: FrameSize,
    dxgi_format: u32,
}

impl Renderer {
    fn new() -> Self {
        Self {
            devices: BTreeMap::new(),
            restore_session: None,
            next_restore_session_id: 1,
        }
    }

    unsafe fn process_frame(
        &mut self,
        request: FrameRequest,
        dirty_rects: &[DirtyRect],
        assignments: &[LutAssignment],
    ) -> Result<FrameOutcome, FrameAcquireError> {
        self.remove_destroyed_back_buffers();
        let frame = unsafe { acquire_frame(request) }?;
        let frame_input = match prepare_frame(frame.dxgi_format, frame.size, dirty_rects) {
            Ok(frame_input) => frame_input,
            Err(reason) => {
                self.update_monitor_identity(frame.route(), request.monitor_identity);
                return Ok(frame.outcome(FrameStatus::Skipped(reason), None, None, None));
            }
        };
        let operation = select_frame_operation(
            assignments,
            request.monitor_identity,
            frame_input.back_buffer_format,
        );
        let lut_index = match operation {
            FrameOperation::Apply { lut, .. } => Some(lut.lut_index),
            FrameOperation::Restore => None,
        };

        let resources = match operation {
            FrameOperation::Apply { .. } => {
                self.resources_for_device(frame.device_id, &frame.device)
            }
            FrameOperation::Restore => self.devices.get_mut(&frame.device_id),
        };
        let result = match resources {
            Some(resources) => resources.process_frame(
                frame.resources(request.monitor_identity),
                frame_input,
                operation,
            ),
            None => match operation {
                FrameOperation::Apply { .. } => Err(FrameProcessingError::DeviceResources),
                FrameOperation::Restore => Ok(FrameEffect::Unchanged),
            },
        };
        Ok(frame_outcome(
            &frame,
            request.monitor_identity,
            lut_index,
            None,
            result,
        ))
    }

    unsafe fn restore_back_buffer(
        &mut self,
        request: FrameRequest,
        dirty_rects: &[DirtyRect],
    ) -> Result<FrameOutcome, FrameAcquireError> {
        self.remove_destroyed_back_buffers();
        let frame = unsafe { acquire_frame(request) }?;
        let frame_input = match prepare_frame(frame.dxgi_format, frame.size, dirty_rects) {
            Ok(frame_input) => frame_input,
            Err(reason) => {
                self.update_monitor_identity(frame.route(), request.monitor_identity);
                return Ok(frame.outcome(FrameStatus::Skipped(reason), None, None, None));
            }
        };
        let route = frame.route();
        let result = match self.devices.get_mut(&frame.device_id) {
            Some(resources) => {
                resources.restore_frame(frame.resources(request.monitor_identity), frame_input)
            }
            None => Ok(FrameEffect::Unchanged),
        };
        self.track_restore_route(route);
        let restore_session_id = self.restore_session.as_ref().map(|session| session.id);
        Ok(frame_outcome(
            &frame,
            request.monitor_identity,
            None,
            restore_session_id,
            result,
        ))
    }

    fn assignments_changed(&mut self) {
        for resources in self.devices.values_mut() {
            resources.invalidate_luts();
        }
    }

    fn begin_restore(&mut self, active_monitors: Option<&HashSet<MonitorIdentity>>) -> usize {
        self.remove_destroyed_back_buffers();
        let routes = self
            .devices
            .iter()
            .flat_map(|(device_id, resources)| {
                resources
                    .restore_targets(active_monitors)
                    .map(|swap_chain| SwapChainRoute {
                        device_id: *device_id,
                        swap_chain,
                    })
            })
            .collect();
        let session_id = self.next_restore_session_id;
        self.next_restore_session_id = self.next_restore_session_id.wrapping_add(1).max(1);
        self.restore_session = Some(RestoreSession {
            id: session_id,
            routes,
        });
        self.tracked_restore_back_buffer_count()
    }

    fn pending_restore_back_buffer_count(&mut self) -> usize {
        self.remove_destroyed_back_buffers();
        self.tracked_restore_back_buffer_count()
    }

    fn track_restore_route(&mut self, route: SwapChainRoute) {
        let Some(session) = self.restore_session.as_ref() else {
            return;
        };
        if session.routes.contains(&route) {
            return;
        }
        let has_applied_luts = self.lut_applied_back_buffer_count(route) != 0;
        if has_applied_luts {
            self.restore_session
                .as_mut()
                .expect("validated restore session must remain active")
                .routes
                .insert(route);
        }
    }

    fn commit_restore(&mut self, token: RestoreToken) {
        self.remove_destroyed_back_buffers();
        if let Some(session_id) = token.session_id {
            let Some(session) = self.restore_session.as_ref() else {
                return;
            };
            if session.id != session_id || !session.routes.contains(&token.route) {
                return;
            }
        }

        let restored = self
            .devices
            .get_mut(&token.route.device_id)
            .is_some_and(|resources| {
                resources.record_restored(
                    token.route.swap_chain,
                    token.back_buffer_id,
                    token.state_generation,
                )
            });
        if !restored {
            return;
        }
        self.prune_resources();

        if self
            .restore_session
            .as_ref()
            .is_some_and(|session| session.routes.contains(&token.route))
        {
            let remaining = self.tracked_restore_back_buffer_count();
            crate::log::shutdown_back_buffer_restored(
                token.monitor_identity,
                token.back_buffer_id,
                remaining,
            );
            RESTORE_PROGRESS.notify_all();
        }
    }

    fn tracked_restore_back_buffer_count(&self) -> usize {
        self.restore_session.as_ref().map_or(0, |session| {
            session
                .routes
                .iter()
                .map(|route| self.lut_applied_back_buffer_count(*route))
                .sum()
        })
    }

    fn lut_applied_back_buffer_count(&self, route: SwapChainRoute) -> usize {
        self.devices.get(&route.device_id).map_or(0, |resources| {
            resources.lut_applied_back_buffer_count(route.swap_chain)
        })
    }

    fn remove_destroyed_back_buffers(&mut self) {
        let destroyed = std::mem::take(&mut *DESTROYED_BACK_BUFFERS.lock());
        if destroyed.is_empty() {
            return;
        }
        for back_buffer_id in destroyed {
            for resources in self.devices.values_mut() {
                resources.remove_back_buffer(back_buffer_id);
            }
        }
        self.prune_resources();
    }

    fn clear_resources(&mut self) {
        self.restore_session = None;
        self.devices.clear();
    }

    fn resources_for_device(
        &mut self,
        device_id: usize,
        device: &ID3D11Device,
    ) -> Option<&mut DeviceResources> {
        match self.devices.entry(device_id) {
            Entry::Occupied(entry) => Some(entry.into_mut()),
            Entry::Vacant(entry) => {
                let resources = DeviceResources::create(device)?;
                Some(entry.insert(resources))
            }
        }
    }

    fn update_monitor_identity(
        &mut self,
        route: SwapChainRoute,
        monitor_identity: MonitorIdentity,
    ) {
        if let Some(resources) = self.devices.get_mut(&route.device_id) {
            resources.update_monitor_identity(route.swap_chain, monitor_identity);
        }
    }

    fn prune_resources(&mut self) {
        self.devices.retain(|_, resources| {
            resources.prune_unused_swap_chains();
            !resources.is_empty()
        });
    }
}

impl AcquiredFrame {
    fn route(&self) -> SwapChainRoute {
        SwapChainRoute {
            device_id: self.device_id,
            swap_chain: self.swap_chain,
        }
    }

    fn resources(&self, monitor_identity: MonitorIdentity) -> FrameResources<'_> {
        FrameResources {
            swap_chain: self.swap_chain,
            back_buffer_id: self.back_buffer_id,
            monitor_identity,
            device: &self.device,
            context: &self.context,
            back_buffer: &self.back_buffer,
            size: self.size,
        }
    }

    fn outcome(
        &self,
        status: FrameStatus,
        present_dirty_rect: Option<DirtyRect>,
        lut_index: Option<usize>,
        restore_token: Option<RestoreToken>,
    ) -> FrameOutcome {
        FrameOutcome {
            present_dirty_rect,
            status,
            dxgi_format: self.dxgi_format,
            frame_size: self.size,
            lut_index,
            restore_token,
            #[cfg(debug_assertions)]
            back_buffer_id: self.back_buffer_id,
        }
    }
}

unsafe fn acquire_frame(request: FrameRequest) -> Result<AcquiredFrame, FrameAcquireError> {
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
    let back_buffer_id = get_or_register_back_buffer_id(&back_buffer)
        .ok_or(FrameAcquireError::BackBufferIdentity)?;

    Ok(AcquiredFrame {
        swap_chain: request.overlay_swap_chain,
        device_id: device.as_raw() as usize,
        device,
        context,
        back_buffer,
        back_buffer_id,
        size: FrameSize {
            width: desc.Width,
            height: desc.Height,
        },
        dxgi_format: desc.Format.0 as u32,
    })
}

fn frame_outcome(
    frame: &AcquiredFrame,
    monitor_identity: MonitorIdentity,
    lut_index: Option<usize>,
    restore_session_id: Option<usize>,
    result: Result<FrameEffect, FrameProcessingError>,
) -> FrameOutcome {
    let (present_dirty_rect, status, restored_generation) = match result {
        Ok(FrameEffect::Unchanged) => (None, FrameStatus::Unchanged, None),
        Ok(FrameEffect::LutApplied { present_dirty_rect }) => {
            (present_dirty_rect, FrameStatus::LutApplied, None)
        }
        Ok(FrameEffect::Restored { generation }) => (
            Some(frame.size.full_rect()),
            FrameStatus::Restored,
            Some(generation),
        ),
        Err(reason) => (None, FrameStatus::Failed(reason), None),
    };
    let restore_token = restored_generation.map(|state_generation| RestoreToken {
        session_id: restore_session_id,
        monitor_identity,
        route: frame.route(),
        back_buffer_id: frame.back_buffer_id,
        state_generation,
    });
    frame.outcome(status, present_dirty_rect, lut_index, restore_token)
}

fn get_or_register_back_buffer_id(back_buffer: &ID3D11Texture2D) -> Option<usize> {
    let private_data_guid = BACK_BUFFER_PRIVATE_DATA_GUID.as_ref()?;
    let mut id = 0usize;
    let mut size = size_of::<usize>() as u32;
    let get_result = unsafe {
        back_buffer.GetPrivateData(
            private_data_guid,
            &mut size,
            Some((&mut id as *mut usize).cast()),
        )
    };
    if get_result.is_ok() && size == size_of::<usize>() as u32 && id != 0 {
        return Some(id);
    }

    let id = NEXT_BACK_BUFFER_ID.fetch_add(1, Ordering::Relaxed);
    if id == 0 {
        return None;
    }
    let notifier: ID3DDestructionNotifier = back_buffer.cast().ok()?;
    let callback_id = unsafe {
        notifier
            .RegisterDestructionCallback(Some(back_buffer_destroyed), id as *const c_void)
            .ok()?
    };
    let set_result = unsafe {
        back_buffer.SetPrivateData(
            private_data_guid,
            size_of::<usize>() as u32,
            Some((&id as *const usize).cast()),
        )
    };
    if set_result.is_err() {
        let _ = unsafe { notifier.UnregisterDestructionCallback(callback_id) };
        return None;
    }
    Some(id)
}

unsafe extern "system" fn back_buffer_destroyed(data: *mut c_void) {
    let id = data as usize;
    if id == 0 {
        return;
    }
    DESTROYED_BACK_BUFFERS.lock().push(id);
    RESTORE_PROGRESS.notify_all();
}

impl RendererGuard {
    pub(crate) unsafe fn process_frame(
        &mut self,
        overlay_swap_chain: usize,
        d3d11_resource: D3D11ResourceAccess,
        monitor_identity: MonitorIdentity,
        dirty_rects: &[DirtyRect],
        assignments: &[LutAssignment],
    ) -> Result<FrameOutcome, FrameAcquireError> {
        unsafe {
            self.renderer.process_frame(
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

    pub(crate) unsafe fn restore_back_buffer(
        &mut self,
        overlay_swap_chain: usize,
        d3d11_resource: D3D11ResourceAccess,
        monitor_identity: MonitorIdentity,
        dirty_rects: &[DirtyRect],
    ) -> Result<FrameOutcome, FrameAcquireError> {
        unsafe {
            self.renderer.restore_back_buffer(
                FrameRequest {
                    overlay_swap_chain,
                    d3d11_resource_access: d3d11_resource,
                    monitor_identity,
                },
                dirty_rects,
            )
        }
    }

    pub(crate) fn assignments_changed(&mut self) {
        self.renderer.assignments_changed();
    }

    pub(crate) fn begin_restore(
        &mut self,
        active_monitors: Option<&HashSet<MonitorIdentity>>,
    ) -> usize {
        self.renderer.begin_restore(active_monitors)
    }

    pub(crate) fn pending_restore_back_buffer_count(&mut self) -> usize {
        self.renderer.pending_restore_back_buffer_count()
    }

    pub(crate) fn commit_restore(&mut self, token: RestoreToken) {
        self.renderer.commit_restore(token);
    }

    pub(crate) fn wait_for_restore_progress(&mut self, timeout: Duration) {
        RESTORE_PROGRESS.wait_for(&mut self.renderer, timeout);
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
