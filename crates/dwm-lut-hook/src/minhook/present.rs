#[cfg(debug_assertions)]
use std::collections::BTreeMap;
#[cfg(not(test))]
use std::ffi::c_void;
#[cfg(not(test))]
use std::mem::MaybeUninit;
use std::mem::{align_of, size_of};
use std::ptr;
#[cfg(debug_assertions)]
use std::sync::{Mutex, OnceLock};

use crate::profile::{HookProfile, MonitorIdentityPathHypothesis};
use crate::route_trace;
use crate::{ClipBox, DirtyRect, state};
use dwm_lut_payload::{AdapterLuid, MonitorIdentity};
#[cfg(not(test))]
use windows::Win32::System::Diagnostics::Debug::ReadProcessMemory;
#[cfg(test)]
use windows::Win32::System::Memory::PAGE_EXECUTE;
#[cfg(not(test))]
use windows::Win32::System::Memory::VirtualQuery;
use windows::Win32::System::Memory::{
    MEM_COMMIT, MEMORY_BASIC_INFORMATION, PAGE_EXECUTE_READ, PAGE_EXECUTE_READWRITE,
    PAGE_EXECUTE_WRITECOPY, PAGE_GUARD, PAGE_READONLY, PAGE_READWRITE, PAGE_WRITECOPY,
};
#[cfg(not(test))]
use windows::Win32::System::Threading::GetCurrentProcess;

use super::detours;

type PresentOriginal = unsafe extern "system" fn(usize, usize, u32, usize, i32, usize, u8) -> i64;

const MAX_DIRTY_RECTS: usize = 4096;

#[repr(C)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct RectVec {
    start: *const DirtyRect,
    end: *const DirtyRect,
    capacity_end: *const DirtyRect,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct PresentInputsWithoutFormat {
    monitor_identity: Option<MonitorIdentity>,
    clip_box: ClipBox,
    dirty_rects: Vec<DirtyRect>,
    hardware_protected: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum PresentInputError {
    MissingProfile,
    NullOverlaySwapChain,
    NullContextState,
    InvalidDirtyRectVector,
    UnreadableMemory,
}

#[cfg(debug_assertions)]
const PRESENT_DIAGNOSTIC_SAMPLE_INTERVAL: u64 = 600;

#[cfg(debug_assertions)]
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
struct PresentDetourLogKey {
    overlay_swap_chain: usize,
}

#[cfg(debug_assertions)]
#[derive(Clone, Copy, Debug)]
struct PresentOriginalCallDetail {
    hardware_protected: bool,
    monitor_identity: Option<MonitorIdentity>,
    dirty_rect_count: usize,
    first_dirty_rect: Option<DirtyRect>,
    render_result: crate::d3d11_renderer::RenderPresentLutResult,
    present_dirty_rect_source: &'static str,
}

#[cfg(debug_assertions)]
struct DiagnosticLogLimiter<K> {
    counts: BTreeMap<K, u64>,
}

#[cfg(debug_assertions)]
impl<K> Default for DiagnosticLogLimiter<K> {
    fn default() -> Self {
        Self {
            counts: BTreeMap::new(),
        }
    }
}

#[cfg(debug_assertions)]
impl<K: Ord> DiagnosticLogLimiter<K> {
    fn should_log(&mut self, key: K) -> bool {
        self.should_log_interval(key, PRESENT_DIAGNOSTIC_SAMPLE_INTERVAL)
    }

    fn should_log_interval(&mut self, key: K, interval: u64) -> bool {
        let count = self.counts.entry(key).or_insert(0);
        *count = count.saturating_add(1);
        *count == 1 || *count <= 8 || (*count).is_multiple_of(interval)
    }
}

#[cfg(debug_assertions)]
fn should_log_diagnostic<K: Ord>(
    limiter: &OnceLock<Mutex<DiagnosticLogLimiter<K>>>,
    key: K,
) -> bool {
    limiter
        .get_or_init(|| Mutex::new(DiagnosticLogLimiter::default()))
        .lock()
        .map(|mut limiter| limiter.should_log(key))
        .unwrap_or(true)
}

#[cfg(debug_assertions)]
static PRESENT_DETOUR_LOG_LIMITER: OnceLock<Mutex<DiagnosticLogLimiter<PresentDetourLogKey>>> =
    OnceLock::new();

#[cfg(debug_assertions)]
static HW_PRESENT_DETOUR_LOG_LIMITER: OnceLock<Mutex<DiagnosticLogLimiter<PresentDetourLogKey>>> =
    OnceLock::new();

#[cfg(debug_assertions)]
const HW_PRESENT_DETOUR_LOG_INTERVAL: u64 = 32;

#[cfg(debug_assertions)]
fn should_log_hw_present_detour_enter(overlay_swap_chain: usize) -> bool {
    HW_PRESENT_DETOUR_LOG_LIMITER
        .get_or_init(|| Mutex::new(DiagnosticLogLimiter::default()))
        .lock()
        .map(|mut limiter| {
            limiter.should_log_interval(
                PresentDetourLogKey { overlay_swap_chain },
                HW_PRESENT_DETOUR_LOG_INTERVAL,
            )
        })
        .unwrap_or(true)
}

fn should_log_present_detour_enter(overlay_swap_chain: usize, hardware_protected: bool) -> bool {
    #[cfg(debug_assertions)]
    {
        if hardware_protected {
            return should_log_hw_present_detour_enter(overlay_swap_chain);
        }
        should_log_diagnostic(
            &PRESENT_DETOUR_LOG_LIMITER,
            PresentDetourLogKey { overlay_swap_chain },
        )
    }
    #[cfg(not(debug_assertions))]
    {
        let _ = (overlay_swap_chain, hardware_protected);
        false
    }
}

const PAGE_PROTECTION_MASK: u32 = 0xff;

unsafe fn collect_present_inputs(
    this: usize,
    overlay_swap_chain: usize,
    rect_vec: usize,
) -> Result<PresentInputsWithoutFormat, PresentInputError> {
    let profile = state::hook_profile().ok_or(PresentInputError::MissingProfile)?;
    unsafe { collect_present_inputs_with_profile(&profile, this, overlay_swap_chain, rect_vec) }
}

unsafe fn collect_present_inputs_with_profile(
    profile: &HookProfile,
    this: usize,
    overlay_swap_chain: usize,
    rect_vec: usize,
) -> Result<PresentInputsWithoutFormat, PresentInputError> {
    let hypotheses = profile.hypotheses;

    if overlay_swap_chain == 0 {
        return Err(PresentInputError::NullOverlaySwapChain);
    }

    let hardware_protected = unsafe {
        read_memory::<u8>(checked_address(
            overlay_swap_chain,
            hypotheses.hardware_protected.offset,
        )?)? != 0
    };
    let clip_box = unsafe {
        read_clip_box(
            this,
            hypotheses.clip_box.context_state_pointer_offset,
            hypotheses.clip_box.offset,
        )?
    };
    let monitor_identity =
        unsafe { read_monitor_identity(overlay_swap_chain, hypotheses.monitor_identity) };
    let dirty_rects = unsafe { read_dirty_rects(rect_vec)? };
    Ok(PresentInputsWithoutFormat {
        monitor_identity,
        clip_box,
        dirty_rects,
        hardware_protected,
    })
}

unsafe fn read_monitor_identity(
    overlay_swap_chain: usize,
    hypothesis: MonitorIdentityPathHypothesis,
) -> Option<MonitorIdentity> {
    let low_part = unsafe {
        read_memory::<u32>(
            checked_address(overlay_swap_chain, hypothesis.adapter_luid_low_offset).ok()?,
        )
        .ok()?
    };
    let high_part = unsafe {
        read_memory::<i32>(
            checked_address(overlay_swap_chain, hypothesis.adapter_luid_high_offset).ok()?,
        )
        .ok()?
    };
    let target_id = unsafe {
        read_memory::<u32>(checked_address(overlay_swap_chain, hypothesis.target_id_offset).ok()?)
            .ok()?
    };

    Some(MonitorIdentity {
        adapter_luid: AdapterLuid {
            high_part,
            low_part,
        },
        target_id,
    })
}

unsafe fn read_clip_box(
    context_address: usize,
    context_state_pointer_offset: usize,
    clip_box_offset: usize,
) -> Result<ClipBox, PresentInputError> {
    let state_pointer_address = checked_address(context_address, context_state_pointer_offset)?;
    let state_object = unsafe { read_memory::<usize>(state_pointer_address)? };
    if state_object == 0 {
        return Err(PresentInputError::NullContextState);
    }

    let origin =
        unsafe { read_memory::<[i32; 2]>(checked_address(state_object, clip_box_offset)?)? };
    Ok(ClipBox {
        left: origin[0],
        top: origin[1],
        right: origin[0],
        bottom: origin[1],
    })
}

unsafe fn read_dirty_rects(rect_vec: usize) -> Result<Vec<DirtyRect>, PresentInputError> {
    if rect_vec == 0 {
        return Err(PresentInputError::InvalidDirtyRectVector);
    }

    let rect_vec = unsafe { read_memory::<RectVec>(rect_vec)? };
    let start = rect_vec.start as usize;
    let end = rect_vec.end as usize;
    let capacity_end = rect_vec.capacity_end as usize;
    if start == 0 && end == 0 && capacity_end == 0 {
        return Ok(Vec::new());
    }
    if start == 0
        || end < start
        || capacity_end < end
        || !start.is_multiple_of(align_of::<DirtyRect>())
    {
        return Err(PresentInputError::InvalidDirtyRectVector);
    }

    let byte_len = end - start;
    if !byte_len.is_multiple_of(size_of::<DirtyRect>()) {
        return Err(PresentInputError::InvalidDirtyRectVector);
    }
    if !(capacity_end - start).is_multiple_of(size_of::<DirtyRect>()) {
        return Err(PresentInputError::InvalidDirtyRectVector);
    }

    let count = byte_len / size_of::<DirtyRect>();
    if count > MAX_DIRTY_RECTS {
        return Err(PresentInputError::InvalidDirtyRectVector);
    }
    if count > 0 && !is_readable_range(start, byte_len) {
        return Err(PresentInputError::UnreadableMemory);
    }

    let mut dirty_rects = Vec::with_capacity(count);
    for index in 0..count {
        let offset = index * size_of::<DirtyRect>();
        dirty_rects.push(unsafe { read_memory::<DirtyRect>(checked_address(start, offset)?)? });
    }

    Ok(dirty_rects)
}

fn checked_address(base: usize, offset: usize) -> Result<usize, PresentInputError> {
    base.checked_add(offset)
        .ok_or(PresentInputError::UnreadableMemory)
}

unsafe fn read_memory<T: Copy>(address: usize) -> Result<T, PresentInputError> {
    if !is_readable_range(address, size_of::<T>()) {
        return Err(PresentInputError::UnreadableMemory);
    }

    #[cfg(test)]
    {
        Ok(unsafe { (address as *const T).read_unaligned() })
    }

    #[cfg(not(test))]
    {
        let mut value = MaybeUninit::<T>::uninit();
        let mut bytes_read = 0usize;
        let result = unsafe {
            ReadProcessMemory(
                GetCurrentProcess(),
                address as *const c_void,
                value.as_mut_ptr().cast(),
                size_of::<T>(),
                Some(&mut bytes_read),
            )
        };
        if result.is_err() || bytes_read != size_of::<T>() {
            return Err(PresentInputError::UnreadableMemory);
        }
        Ok(unsafe { value.assume_init() })
    }
}

fn is_readable_range(address: usize, size: usize) -> bool {
    if address == 0 || size == 0 {
        return false;
    }
    let Some(end) = address.checked_add(size - 1) else {
        return false;
    };

    #[cfg(test)]
    {
        let _ = end;
        true
    }

    #[cfg(not(test))]
    {
        is_readable_range_in_process(address, end)
    }
}

#[cfg(not(test))]
fn is_readable_range_in_process(mut address: usize, end: usize) -> bool {
    while address <= end {
        let Some(info) = query_memory(address) else {
            return false;
        };
        if !is_readable_memory_region(&info) {
            return false;
        }

        let region_start = info.BaseAddress as usize;
        let Some(region_end) = region_start.checked_add(info.RegionSize.saturating_sub(1)) else {
            return false;
        };
        if region_end >= end {
            return true;
        }
        address = match region_end.checked_add(1) {
            Some(next) if next > address => next,
            _ => return false,
        };
    }

    true
}

#[cfg(not(test))]
fn query_memory(address: usize) -> Option<MEMORY_BASIC_INFORMATION> {
    let mut info = MEMORY_BASIC_INFORMATION::default();
    let written = unsafe {
        VirtualQuery(
            Some(address as *const c_void),
            &mut info,
            size_of::<MEMORY_BASIC_INFORMATION>(),
        )
    };
    (written != 0).then_some(info)
}

fn is_readable_memory_region(info: &MEMORY_BASIC_INFORMATION) -> bool {
    if info.State != MEM_COMMIT || (info.Protect.0 & PAGE_GUARD.0) != 0 {
        return false;
    }

    matches!(
        info.Protect.0 & PAGE_PROTECTION_MASK,
        value if value == PAGE_READONLY.0
            || value == PAGE_READWRITE.0
            || value == PAGE_WRITECOPY.0
            || value == PAGE_EXECUTE_READ.0
            || value == PAGE_EXECUTE_READWRITE.0
            || value == PAGE_EXECUTE_WRITECOPY.0
    )
}

fn deactivate_present_context(context_address: usize) {
    let _ = state::evaluate_present_hook(
        context_address,
        None,
        ClipBox {
            left: 0,
            top: 0,
            right: 0,
            bottom: 0,
        },
        0,
        &[],
        false,
    );
}

pub(super) unsafe extern "system" fn present_detour(
    this: usize,
    overlay_swap_chain: usize,
    a3: u32,
    rect_vec: usize,
    a5: i32,
    a6: usize,
    a7: u8,
) -> i64 {
    let original = detours::present_original();
    if original.is_null() {
        return 0;
    }
    if !state::is_runtime_active() {
        let original: PresentOriginal = unsafe { std::mem::transmute(original) };
        return unsafe { original(this, overlay_swap_chain, a3, rect_vec, a5, a6, a7) };
    }

    let mut original_rect_vec = rect_vec;
    let mut present_rect_storage = [DirtyRect {
        left: 0,
        top: 0,
        right: 0,
        bottom: 0,
    }];
    let mut present_rect_vec_storage = RectVec {
        start: ptr::null(),
        end: ptr::null(),
        capacity_end: ptr::null(),
    };
    #[cfg(debug_assertions)]
    let mut protected_resource_result_detail = None;
    #[cfg(debug_assertions)]
    let mut last_present_context = None;
    match unsafe { collect_present_inputs(this, overlay_swap_chain, rect_vec) } {
        Ok(inputs) => {
            if should_log_present_detour_enter(overlay_swap_chain, inputs.hardware_protected) {
                debug_log!(
                    "event=present_detour_enter this=0x{:x} overlay_swap_chain=0x{:x} rect_vec=0x{:x}",
                    this,
                    overlay_swap_chain,
                    rect_vec
                );
            }
            route_trace::record_present_enter(overlay_swap_chain, inputs.hardware_protected);
            #[cfg(debug_assertions)]
            {
                last_present_context =
                    Some((inputs.hardware_protected, inputs.monitor_identity, None));
            }
            if let Some(_present_guard) = state::try_lock_present_runtime() {
                if let Some(render_result) = render_present_lut_if_active(
                    overlay_swap_chain,
                    inputs.monitor_identity,
                    inputs.hardware_protected,
                    inputs.clip_box,
                    &inputs.dirty_rects,
                ) {
                    #[cfg(debug_assertions)]
                    {
                        last_present_context = Some((
                            inputs.hardware_protected,
                            inputs.monitor_identity,
                            Some(render_result.lut_applied),
                        ));
                        if inputs.hardware_protected {
                            protected_resource_result_detail = Some(PresentOriginalCallDetail {
                                hardware_protected: inputs.hardware_protected,
                                monitor_identity: inputs.monitor_identity,
                                dirty_rect_count: inputs.dirty_rects.len(),
                                first_dirty_rect: inputs.dirty_rects.first().copied(),
                                render_result,
                                present_dirty_rect_source: "original",
                            });
                        }
                    }
                    route_trace::record_present_lut_result(
                        inputs.hardware_protected,
                        render_result.lut_applied,
                    );
                    let present_dirty_rect_source =
                        if let Some(rect) = render_result.present_dirty_rect {
                            original_rect_vec = full_present_rect_vec(
                                rect,
                                &mut present_rect_storage,
                                &mut present_rect_vec_storage,
                            );
                            "expanded"
                        } else {
                            "original"
                        };
                    #[cfg(debug_assertions)]
                    {
                        if inputs.hardware_protected {
                            protected_resource_result_detail = Some(PresentOriginalCallDetail {
                                hardware_protected: inputs.hardware_protected,
                                monitor_identity: inputs.monitor_identity,
                                dirty_rect_count: inputs.dirty_rects.len(),
                                first_dirty_rect: inputs.dirty_rects.first().copied(),
                                render_result,
                                present_dirty_rect_source,
                            });
                        }
                    }
                    #[cfg(not(debug_assertions))]
                    let _ = present_dirty_rect_source;
                    if let Some(dxgi_format) = render_result.dxgi_format {
                        let _ = state::evaluate_rendered_present_hook(
                            this,
                            inputs.monitor_identity,
                            inputs.clip_box,
                            dxgi_format,
                            &inputs.dirty_rects,
                            render_result,
                        );
                    }
                }
            } else {
                route_trace::record_present_lock_miss(overlay_swap_chain);
            }
        }
        Err(error) => {
            #[cfg(debug_assertions)]
            {
                debug_log!(
                    "event=present_input_collect_error this=0x{:x} overlay_swap_chain=0x{:x} rect_vec=0x{:x} error={:?}",
                    this,
                    overlay_swap_chain,
                    rect_vec,
                    error
                );
            }
            #[cfg(not(debug_assertions))]
            let _ = error;
            deactivate_present_context(this);
        }
    }

    #[cfg(debug_assertions)]
    if let Some((hardware_protected, monitor_identity, lut_applied)) = last_present_context {
        let last_present_sequence = route_trace::record_last_present_context(
            overlay_swap_chain,
            monitor_identity,
            hardware_protected,
            lut_applied,
            None,
        );
        let original: PresentOriginal = unsafe { std::mem::transmute(original) };
        let original_result =
            unsafe { original(this, overlay_swap_chain, a3, original_rect_vec, a5, a6, a7) };
        route_trace::record_last_present_original_result(last_present_sequence, original_result);
        if let Some(detail) = protected_resource_result_detail {
            route_trace::record_protected_present_resource_result_summary(
                overlay_swap_chain,
                detail.monitor_identity,
                detail.hardware_protected,
                original_result,
                detail.render_result,
                detail.dirty_rect_count,
                detail.first_dirty_rect,
                detail.present_dirty_rect_source == "expanded",
                detail.render_result.present_dirty_rect,
            );
        }
        return original_result;
    }

    let original: PresentOriginal = unsafe { std::mem::transmute(original) };
    unsafe { original(this, overlay_swap_chain, a3, original_rect_vec, a5, a6, a7) }
}

fn render_present_lut_if_active(
    overlay_swap_chain: usize,
    monitor_identity: Option<MonitorIdentity>,
    hardware_protected: bool,
    clip_box: ClipBox,
    dirty_rects: &[DirtyRect],
) -> Option<crate::d3d11_renderer::RenderPresentLutResult> {
    if !state::is_runtime_active() {
        return None;
    }
    Some(state::render_present_lut(
        overlay_swap_chain,
        monitor_identity,
        hardware_protected,
        clip_box,
        dirty_rects,
    ))
}

fn full_present_rect_vec(
    rect: DirtyRect,
    rect_storage: &mut [DirtyRect; 1],
    rect_vec_storage: &mut RectVec,
) -> usize {
    rect_storage[0] = rect;
    let start = rect_storage.as_ptr();
    *rect_vec_storage = RectVec {
        start,
        end: unsafe { start.add(1) },
        capacity_end: unsafe { start.add(1) },
    };
    (rect_vec_storage as *const RectVec) as usize
}

#[cfg(test)]
mod tests {
    use std::ffi::c_void;
    use std::mem::size_of;
    use std::sync::Mutex;
    use std::sync::atomic::Ordering;

    use dwm_lut_payload::{
        AdapterLuid, ColorMode, HookPayload, MonitorIdentity, MonitorTarget, PayloadAssignment,
        PayloadLut,
    };

    use crate::profile::HookTarget;
    use crate::profile::VERSIONED_PROFILES;
    use crate::resolver::{LoadedModule, ResolvedTarget, SignatureResolutionReport};
    use crate::state::{self, PRESENT_RUNTIME_TEST_LOCK as CONTROLLED_TEST_LOCK};
    use crate::{
        BackBufferFormat, ClipBox, DXGI_FORMAT_B8G8R8A8_UNORM, DXGI_FORMAT_R16G16B16A16_FLOAT,
        DirtyRect, HookProfile,
    };

    fn test_profile() -> HookProfile {
        (VERSIONED_PROFILES[0].build)()
    }

    static LAST_ORIGINAL_PRESENT_RECTS: Mutex<Option<Vec<DirtyRect>>> = Mutex::new(None);

    fn last_original_present_rects() -> Option<Vec<DirtyRect>> {
        LAST_ORIGINAL_PRESENT_RECTS
            .lock()
            .ok()
            .and_then(|rects| rects.clone())
    }

    fn reset_last_original_present_rects() {
        if let Ok(mut rects) = LAST_ORIGINAL_PRESENT_RECTS.lock() {
            *rects = None;
        }
    }

    unsafe extern "system" fn returns_present_status(
        _a0: usize,
        _a1: usize,
        _a2: u32,
        a3: usize,
        _a4: i32,
        _a5: usize,
        _a6: u8,
    ) -> i64 {
        if let Ok(mut rects) = LAST_ORIGINAL_PRESENT_RECTS.lock() {
            *rects = unsafe { super::read_dirty_rects(a3) }.ok();
        }
        0x55
    }

    fn test_monitor_identity() -> MonitorIdentity {
        test_monitor_identity_for_target(4357)
    }

    fn test_monitor_identity_for_target(target_id: u32) -> MonitorIdentity {
        MonitorIdentity {
            adapter_luid: AdapterLuid {
                high_part: 0,
                low_part: 0x14e02,
            },
            target_id,
        }
    }

    fn synthetic_resolution(profile: &HookProfile) -> SignatureResolutionReport {
        let base_address = 0x1800_0000usize;
        SignatureResolutionReport {
            module: LoadedModule {
                module_name: crate::profile::HOOK_MODULE_NAME,
                base_address,
                size: 0x20_0000,
            },
            targets: profile
                .signatures
                .iter()
                .enumerate()
                .map(|(index, signature)| ResolvedTarget {
                    target: signature.target,
                    address: if signature.target == HookTarget::OverlayTestMode {
                        0
                    } else {
                        base_address + 0x1000 + index * 0x100
                    },
                })
                .collect(),
            skipped_signatures: Vec::new(),
        }
    }

    fn identity_lut() -> PayloadLut {
        PayloadLut {
            size: 2,
            domain_min: [0.0, 0.0, 0.0],
            domain_max: [1.0, 1.0, 1.0],
            values: vec![
                [0.0, 0.0, 0.0],
                [1.0, 0.0, 0.0],
                [0.0, 1.0, 0.0],
                [1.0, 1.0, 0.0],
                [0.0, 0.0, 1.0],
                [1.0, 0.0, 1.0],
                [0.0, 1.0, 1.0],
                [1.0, 1.0, 1.0],
            ],
        }
    }

    fn test_payload(color_modes: &[ColorMode]) -> HookPayload {
        HookPayload {
            assignments: color_modes
                .iter()
                .map(|color_mode| PayloadAssignment {
                    target: MonitorTarget {
                        identity: test_monitor_identity(),
                        color_mode: *color_mode,
                    },
                    lut: identity_lut(),
                })
                .collect(),
        }
    }

    fn initialize_test_state() {
        state::reset_state_for_tests();
        initialize_test_state_from_payload(test_payload(&[ColorMode::Sdr]));
    }

    fn initialize_test_state_from_payload(payload: HookPayload) {
        let profile = test_profile();
        let resolution = synthetic_resolution(&profile);
        crate::bootstrap::initialize_with_resolution(profile, payload, resolution)
            .expect("initialization should succeed with synthetic resolution");
    }

    fn activate_context(context_address: usize) {
        let dirty_rects = [DirtyRect {
            left: 0,
            top: 0,
            right: 64,
            bottom: 64,
        }];
        state::evaluate_present_hook(
            context_address,
            Some(test_monitor_identity()),
            ClipBox {
                left: 0,
                top: 0,
                right: 1920,
                bottom: 1080,
            },
            DXGI_FORMAT_B8G8R8A8_UNORM,
            &dirty_rects,
            true,
        )
        .expect("present evaluation should run");
    }

    struct FakePresentObjects {
        context: Box<usize>,
        _context_state: Vec<usize>,
        overlay_swap_chain: Vec<usize>,
        dirty_rects: Vec<DirtyRect>,
        rect_vec: super::RectVec,
    }

    impl FakePresentObjects {
        fn new(clip_box: ClipBox, dirty_rects: Vec<DirtyRect>, hardware_protected: bool) -> Self {
            let profile = test_profile();
            let context_state_len = (profile.hypotheses.clip_box.offset + size_of::<ClipBox>())
                .div_ceil(size_of::<usize>());
            let mut context_state = vec![0usize; context_state_len];
            unsafe {
                ((context_state.as_mut_ptr() as *mut u8).add(profile.hypotheses.clip_box.offset)
                    as *mut ClipBox)
                    .write(clip_box);
            }

            let context = Box::new(context_state.as_ptr() as usize);

            let identity = profile.hypotheses.monitor_identity;
            let overlay_swap_chain_len = (profile
                .hypotheses
                .hardware_protected
                .offset
                .max(identity.target_id_offset + size_of::<u32>())
                + 1)
            .div_ceil(size_of::<usize>());
            let mut overlay_swap_chain = vec![0usize; overlay_swap_chain_len];
            unsafe {
                (overlay_swap_chain.as_mut_ptr() as *mut u8)
                    .add(profile.hypotheses.hardware_protected.offset)
                    .write(u8::from(hardware_protected));
                ((overlay_swap_chain.as_mut_ptr() as *mut u8).add(identity.adapter_luid_low_offset)
                    as *mut u32)
                    .write(test_monitor_identity().adapter_luid.low_part);
                ((overlay_swap_chain.as_mut_ptr() as *mut u8)
                    .add(identity.adapter_luid_high_offset) as *mut i32)
                    .write(test_monitor_identity().adapter_luid.high_part);
                ((overlay_swap_chain.as_mut_ptr() as *mut u8).add(identity.target_id_offset)
                    as *mut u32)
                    .write(test_monitor_identity().target_id);
            }

            let rect_vec = if dirty_rects.is_empty() {
                super::RectVec {
                    start: std::ptr::null(),
                    end: std::ptr::null(),
                    capacity_end: std::ptr::null(),
                }
            } else {
                let start = dirty_rects.as_ptr();
                super::RectVec {
                    start,
                    end: unsafe { start.add(dirty_rects.len()) },
                    capacity_end: unsafe { start.add(dirty_rects.capacity()) },
                }
            };

            Self {
                context,
                _context_state: context_state,
                overlay_swap_chain,
                dirty_rects,
                rect_vec,
            }
        }

        fn context_address(&self) -> usize {
            (&*self.context as *const usize) as usize
        }

        fn overlay_swap_chain_address(&self) -> usize {
            self.overlay_swap_chain.as_ptr() as usize
        }

        fn rect_vec_address(&self) -> usize {
            (&self.rect_vec as *const super::RectVec) as usize
        }
    }

    #[test]
    fn present_input_collection_reads_confirmed_inputs_without_swap_chain_accessor() {
        let _guard = CONTROLLED_TEST_LOCK.lock().expect("test mutex should lock");
        initialize_test_state();
        let fake = FakePresentObjects::new(
            ClipBox {
                left: 120,
                top: 80,
                right: 1920,
                bottom: 1080,
            },
            vec![DirtyRect {
                left: 10,
                top: 20,
                right: 30,
                bottom: 40,
            }],
            false,
        );

        let inputs = unsafe {
            super::collect_present_inputs_with_profile(
                &test_profile(),
                fake.context_address(),
                fake.overlay_swap_chain_address(),
                fake.rect_vec_address(),
            )
        }
        .expect("present inputs should be collected");

        assert_eq!(inputs.clip_box.left, 120);
        assert_eq!(inputs.clip_box.top, 80);
        assert_eq!(inputs.monitor_identity, Some(test_monitor_identity()));
        assert_eq!(inputs.dirty_rects, fake.dirty_rects);
        assert!(!inputs.hardware_protected);
    }

    #[test]
    fn present_input_collection_reads_confirmed_inputs_when_hardware_protected() {
        let _guard = CONTROLLED_TEST_LOCK.lock().expect("test mutex should lock");
        initialize_test_state();
        let fake = FakePresentObjects::new(
            ClipBox {
                left: 120,
                top: 80,
                right: 1920,
                bottom: 1080,
            },
            vec![DirtyRect {
                left: 10,
                top: 20,
                right: 30,
                bottom: 40,
            }],
            true,
        );

        let inputs = unsafe {
            super::collect_present_inputs_with_profile(
                &test_profile(),
                fake.context_address(),
                fake.overlay_swap_chain_address(),
                fake.rect_vec_address(),
            )
        }
        .expect("hardware protected state should be collected");

        assert_eq!(inputs.clip_box.left, 120);
        assert_eq!(inputs.clip_box.top, 80);
        assert_eq!(inputs.monitor_identity, Some(test_monitor_identity()));
        assert_eq!(inputs.dirty_rects, fake.dirty_rects);
        assert!(inputs.hardware_protected);
    }

    #[test]
    fn present_input_collection_rejects_dirty_rect_vector_past_capacity() {
        let rects = [DirtyRect {
            left: 0,
            top: 0,
            right: 1,
            bottom: 1,
        }];
        let rect_vec = super::RectVec {
            start: rects.as_ptr(),
            end: unsafe { rects.as_ptr().add(1) },
            capacity_end: rects.as_ptr(),
        };

        let error =
            unsafe { super::read_dirty_rects((&rect_vec as *const super::RectVec) as usize) }
                .expect_err("end past capacity should be rejected");

        assert_eq!(error, super::PresentInputError::InvalidDirtyRectVector);
    }

    #[test]
    fn present_input_collection_rejects_misaligned_dirty_rect_vector() {
        let start = std::ptr::dangling::<DirtyRect>() as usize + 1;
        let rect_vec = super::RectVec {
            start: start as *const DirtyRect,
            end: (start + size_of::<DirtyRect>()) as *const DirtyRect,
            capacity_end: (start + size_of::<DirtyRect>()) as *const DirtyRect,
        };

        let error =
            unsafe { super::read_dirty_rects((&rect_vec as *const super::RectVec) as usize) }
                .expect_err("misaligned start should be rejected");

        assert_eq!(error, super::PresentInputError::InvalidDirtyRectVector);
    }

    #[test]
    fn pointer_addition_overflow_is_not_treated_as_readable_memory() {
        let error = super::checked_address(usize::MAX, 1)
            .expect_err("overflowed address should be rejected");

        assert_eq!(error, super::PresentInputError::UnreadableMemory);
    }

    #[test]
    fn null_dirty_rect_vector_is_invalid_present_input() {
        let error = unsafe { super::read_dirty_rects(0) }
            .expect_err("null rectVec pointer should be rejected");

        assert_eq!(error, super::PresentInputError::InvalidDirtyRectVector);
    }

    #[test]
    fn memory_region_readability_requires_read_protection() {
        let readable = windows::Win32::System::Memory::MEMORY_BASIC_INFORMATION {
            RegionSize: 4096,
            State: super::MEM_COMMIT,
            Protect: super::PAGE_READONLY,
            ..Default::default()
        };
        let execute_only = windows::Win32::System::Memory::MEMORY_BASIC_INFORMATION {
            Protect: super::PAGE_EXECUTE,
            ..readable
        };
        let guarded = windows::Win32::System::Memory::MEMORY_BASIC_INFORMATION {
            Protect: super::PAGE_READWRITE | super::PAGE_GUARD,
            ..readable
        };

        assert!(super::is_readable_memory_region(&readable));
        assert!(!super::is_readable_memory_region(&execute_only));
        assert!(!super::is_readable_memory_region(&guarded));
    }

    #[test]
    fn monitor_identity_offsets_match_overlay_swap_chain_fixture() {
        let profile = test_profile();
        let hypothesis = profile.hypotheses.monitor_identity;
        assert_eq!(hypothesis.adapter_luid_low_offset, 0x34);
        assert_eq!(hypothesis.adapter_luid_high_offset, 0x38);
        assert_eq!(hypothesis.target_id_offset, 0x3c);

        let fake = FakePresentObjects::new(
            ClipBox {
                left: 0,
                top: 0,
                right: 1920,
                bottom: 1080,
            },
            Vec::new(),
            false,
        );

        let identity =
            unsafe { super::read_monitor_identity(fake.overlay_swap_chain_address(), hypothesis) };

        assert_eq!(identity, Some(test_monitor_identity()));
    }

    #[test]
    fn present_detour_keeps_context_active_when_render_succeeds() {
        let _guard = CONTROLLED_TEST_LOCK.lock().expect("test mutex should lock");
        reset_last_original_present_rects();
        initialize_test_state();
        let fake = FakePresentObjects::new(
            ClipBox {
                left: 0,
                top: 0,
                right: 1920,
                bottom: 1080,
            },
            vec![DirtyRect {
                left: 0,
                top: 0,
                right: 64,
                bottom: 64,
            }],
            false,
        );
        crate::d3d11_renderer::set_test_render_present_lut_result(true);
        super::super::detours::original_pointer_for_target(HookTarget::Present)
            .store(returns_present_status as *mut c_void, Ordering::Release);

        assert_eq!(
            unsafe {
                super::present_detour(
                    fake.context_address(),
                    fake.overlay_swap_chain_address(),
                    0,
                    fake.rect_vec_address(),
                    0,
                    0,
                    0,
                )
            },
            0x55
        );

        let context = state::lut_bypass_runtime()
            .and_then(|runtime| runtime.context(fake.context_address()).cloned())
            .expect("successful LUT render should keep the context active");
        assert_eq!(context.lut_index, Some(0));
        assert_eq!(context.dirty_rect_count, 1);
        let render_call = crate::d3d11_renderer::test_render_present_lut_call()
            .expect("renderer should be called with collected present inputs");
        assert_eq!(
            crate::d3d11_renderer::test_render_context_active(),
            Some(false)
        );
        assert_eq!(
            render_call.overlay_swap_chain,
            fake.overlay_swap_chain_address()
        );
        assert_eq!(render_call.swap_chain_path.container_vtable_index, 24);
        assert_eq!(render_call.swap_chain_path.resource_vtable_index, 19);
        assert_eq!(render_call.monitor_identity, Some(test_monitor_identity()));
        assert_eq!(render_call.clip_box.left, 0);
        assert_eq!(render_call.clip_box.top, 0);
        assert_eq!(render_call.dirty_rects, fake.dirty_rects);
    }

    #[test]
    fn present_detour_records_renderer_dxgi_format_for_bypass_state() {
        let _guard = CONTROLLED_TEST_LOCK.lock().expect("test mutex should lock");
        reset_last_original_present_rects();
        state::reset_state_for_tests();
        state::reset_state_for_tests();
        initialize_test_state_from_payload(test_payload(&[ColorMode::Sdr, ColorMode::Hdr]));
        let fake = FakePresentObjects::new(
            ClipBox {
                left: 120,
                top: 80,
                right: 120,
                bottom: 80,
            },
            vec![DirtyRect {
                left: 0,
                top: 0,
                right: 64,
                bottom: 64,
            }],
            false,
        );
        crate::d3d11_renderer::set_test_render_present_lut_result(true);
        crate::d3d11_renderer::set_test_render_present_dxgi_format(Some(
            DXGI_FORMAT_R16G16B16A16_FLOAT,
        ));
        super::super::detours::original_pointer_for_target(HookTarget::Present)
            .store(returns_present_status as *mut c_void, Ordering::Release);

        assert_eq!(
            unsafe {
                super::present_detour(
                    fake.context_address(),
                    fake.overlay_swap_chain_address(),
                    0,
                    fake.rect_vec_address(),
                    0,
                    0,
                    0,
                )
            },
            0x55
        );

        let context = state::lut_bypass_runtime()
            .and_then(|runtime| runtime.context(fake.context_address()).cloned())
            .expect("HDR render plan should keep the context active");
        assert_eq!(
            context.back_buffer_format,
            Some(BackBufferFormat::Rgba16Float)
        );
        assert_eq!(context.lut_index, Some(1));

        crate::d3d11_renderer::reset_test_render_present_lut_result();
    }

    #[test]
    fn present_detour_expands_original_present_dirty_rect_for_full_redraw() {
        let _guard = CONTROLLED_TEST_LOCK.lock().expect("test mutex should lock");
        reset_last_original_present_rects();
        initialize_test_state();
        let fake = FakePresentObjects::new(
            ClipBox {
                left: 0,
                top: 0,
                right: 1920,
                bottom: 1080,
            },
            vec![DirtyRect {
                left: 10,
                top: 20,
                right: 64,
                bottom: 96,
            }],
            false,
        );
        let full_rect = DirtyRect {
            left: 0,
            top: 0,
            right: 1920,
            bottom: 1080,
        };
        crate::d3d11_renderer::set_test_render_present_lut_result_with_present_rect(
            true,
            Some(full_rect),
        );
        super::super::detours::original_pointer_for_target(HookTarget::Present)
            .store(returns_present_status as *mut c_void, Ordering::Release);

        assert_eq!(
            unsafe {
                super::present_detour(
                    fake.context_address(),
                    fake.overlay_swap_chain_address(),
                    0,
                    fake.rect_vec_address(),
                    0,
                    0,
                    0,
                )
            },
            0x55
        );

        assert_eq!(last_original_present_rects(), Some(vec![full_rect]));
        let render_call = crate::d3d11_renderer::test_render_present_lut_call()
            .expect("renderer should still receive original present inputs");
        assert_eq!(render_call.dirty_rects, fake.dirty_rects);
    }

    #[test]
    fn present_detour_keeps_context_active_when_render_misses_a_frame() {
        let _guard = CONTROLLED_TEST_LOCK.lock().expect("test mutex should lock");
        reset_last_original_present_rects();
        initialize_test_state();
        let fake = FakePresentObjects::new(
            ClipBox {
                left: 0,
                top: 0,
                right: 1920,
                bottom: 1080,
            },
            vec![DirtyRect {
                left: 0,
                top: 0,
                right: 64,
                bottom: 64,
            }],
            false,
        );
        activate_context(fake.context_address());
        super::super::detours::original_pointer_for_target(HookTarget::Present)
            .store(returns_present_status as *mut c_void, Ordering::Release);

        assert_eq!(
            unsafe {
                super::present_detour(
                    fake.context_address(),
                    fake.overlay_swap_chain_address(),
                    0,
                    fake.rect_vec_address(),
                    0,
                    0,
                    0,
                )
            },
            0x55
        );
        let context = state::lut_bypass_runtime()
            .and_then(|runtime| runtime.context(fake.context_address()).cloned())
            .expect("present plan should keep the context active across a missed render");
        assert_eq!(context.lut_index, Some(0));
        assert_eq!(context.dirty_rect_count, 1);
    }

    #[test]
    fn rendered_present_clears_context_when_observed_format_has_no_assignment() {
        let _guard = CONTROLLED_TEST_LOCK.lock().expect("test mutex should lock");
        initialize_test_state();
        let context_address = 0x1234;
        let clip_box = ClipBox {
            left: 0,
            top: 0,
            right: 1920,
            bottom: 1080,
        };
        let dirty_rects = [DirtyRect {
            left: 0,
            top: 0,
            right: 64,
            bottom: 64,
        }];

        activate_context(context_address);
        state::evaluate_rendered_present_hook(
            context_address,
            Some(test_monitor_identity()),
            clip_box,
            DXGI_FORMAT_R16G16B16A16_FLOAT,
            &dirty_rects,
            crate::d3d11_renderer::RenderPresentLutResult {
                dxgi_format: Some(DXGI_FORMAT_R16G16B16A16_FLOAT),
                ..Default::default()
            },
        )
        .expect("post-render present evaluation should run");

        assert!(
            state::lut_bypass_runtime()
                .and_then(|runtime| runtime.context(context_address).cloned())
                .is_none()
        );
    }

    #[test]
    fn present_detour_preserves_hdr_context_when_back_buffer_format_is_unobserved() {
        let _guard = CONTROLLED_TEST_LOCK.lock().expect("test mutex should lock");
        reset_last_original_present_rects();
        state::reset_state_for_tests();
        initialize_test_state_from_payload(test_payload(&[ColorMode::Hdr]));
        let fake = FakePresentObjects::new(
            ClipBox {
                left: 0,
                top: 0,
                right: 1920,
                bottom: 1080,
            },
            vec![DirtyRect {
                left: 0,
                top: 0,
                right: 64,
                bottom: 64,
            }],
            false,
        );
        state::evaluate_present_hook(
            fake.context_address(),
            Some(test_monitor_identity()),
            ClipBox {
                left: 0,
                top: 0,
                right: 1920,
                bottom: 1080,
            },
            DXGI_FORMAT_R16G16B16A16_FLOAT,
            &fake.dirty_rects,
            true,
        )
        .expect("HDR present evaluation should activate the context");
        super::super::detours::original_pointer_for_target(HookTarget::Present)
            .store(returns_present_status as *mut c_void, Ordering::Release);

        assert_eq!(
            unsafe {
                super::present_detour(
                    fake.context_address(),
                    fake.overlay_swap_chain_address(),
                    0,
                    fake.rect_vec_address(),
                    0,
                    0,
                    0,
                )
            },
            0x55
        );

        let context = state::lut_bypass_runtime()
            .and_then(|runtime| runtime.context(fake.context_address()).cloned())
            .expect("unobserved format must not clear an active HDR context");
        assert_eq!(
            context.back_buffer_format,
            Some(BackBufferFormat::Rgba16Float)
        );
        assert_eq!(context.lut_index, Some(0));
    }

    #[test]
    fn present_detour_renders_when_hardware_protected_inputs_are_readable() {
        let _guard = CONTROLLED_TEST_LOCK.lock().expect("test mutex should lock");
        initialize_test_state();
        let fake = FakePresentObjects::new(
            ClipBox {
                left: 0,
                top: 0,
                right: 1920,
                bottom: 1080,
            },
            Vec::new(),
            true,
        );
        activate_context(fake.context_address());
        super::super::detours::original_pointer_for_target(HookTarget::Present)
            .store(returns_present_status as *mut c_void, Ordering::Release);
        crate::d3d11_renderer::set_test_render_present_lut_result(true);

        assert_eq!(
            unsafe {
                super::present_detour(
                    fake.context_address(),
                    fake.overlay_swap_chain_address(),
                    0,
                    fake.rect_vec_address(),
                    0,
                    0,
                    0,
                )
            },
            0x55
        );
        let render_call = crate::d3d11_renderer::test_render_present_lut_call()
            .expect("hardware protected present should reach renderer");
        assert_eq!(
            render_call.overlay_swap_chain,
            fake.overlay_swap_chain_address()
        );
        assert_eq!(render_call.monitor_identity, Some(test_monitor_identity()));
        assert!(render_call.hardware_protected);
        assert_eq!(render_call.dirty_rects, fake.dirty_rects);
        crate::d3d11_renderer::reset_test_render_present_lut_result();
    }

    #[test]
    fn present_detour_clears_context_when_input_acquisition_fails() {
        let _guard = CONTROLLED_TEST_LOCK.lock().expect("test mutex should lock");
        initialize_test_state();
        activate_context(0x1234);
        super::super::detours::original_pointer_for_target(HookTarget::Present)
            .store(returns_present_status as *mut c_void, Ordering::Release);

        assert_eq!(
            unsafe { super::present_detour(0x1234, 0, 0, 0, 0, 0, 0) },
            0x55
        );
        assert!(
            state::lut_bypass_runtime()
                .and_then(|runtime| runtime.context(0x1234).cloned())
                .is_none()
        );
    }

    #[test]
    fn present_render_is_skipped_when_shutdown_starts_after_entry_check() {
        let _guard = CONTROLLED_TEST_LOCK.lock().expect("test mutex should lock");
        initialize_test_state();
        crate::d3d11_renderer::set_test_render_present_lut_result(true);

        assert_eq!(state::begin_shutdown(), state::ShutdownStart::Started);

        let render_result = super::render_present_lut_if_active(
            0x1234,
            Some(test_monitor_identity()),
            false,
            ClipBox {
                left: 0,
                top: 0,
                right: 1920,
                bottom: 1080,
            },
            &[],
        );

        assert!(render_result.is_none());
        assert!(crate::d3d11_renderer::test_render_present_lut_call().is_none());
        state::reset_state_for_tests();
    }
}
