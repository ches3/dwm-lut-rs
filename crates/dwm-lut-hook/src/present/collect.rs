#[cfg(not(test))]
use std::ffi::c_void;
#[cfg(not(test))]
use std::mem::MaybeUninit;
use std::mem::{align_of, size_of};

use super::DirtyRect;
use crate::profile::{HookProfile, MonitorIdentityOffsets};
use crate::state;
use dwm_lut_payload::{AdapterLuid, MonitorIdentity};
#[cfg(not(test))]
use windows::Win32::System::Diagnostics::Debug::ReadProcessMemory;
#[cfg(not(test))]
use windows::Win32::System::Memory::VirtualQuery;
use windows::Win32::System::Memory::{
    MEM_COMMIT, MEMORY_BASIC_INFORMATION, PAGE_EXECUTE_READ, PAGE_EXECUTE_READWRITE,
    PAGE_EXECUTE_WRITECOPY, PAGE_GUARD, PAGE_READONLY, PAGE_READWRITE, PAGE_WRITECOPY,
};
#[cfg(not(test))]
use windows::Win32::System::Threading::GetCurrentProcess;

const MAX_DIRTY_RECTS: usize = 4096;
const PAGE_PROTECTION_MASK: u32 = 0xff;

#[repr(C)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct RectVec {
    pub(crate) start: *const DirtyRect,
    pub(crate) end: *const DirtyRect,
    pub(crate) capacity_end: *const DirtyRect,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct PresentInputs {
    pub(crate) monitor_identity: Option<MonitorIdentity>,
    pub(crate) dirty_rects: Vec<DirtyRect>,
    pub(crate) hardware_protected: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum PresentInputError {
    MissingProfile,
    NullOverlaySwapChain,
    InvalidDirtyRectVector,
    UnreadableMemory,
}

pub(crate) unsafe fn collect_present_inputs(
    overlay_swap_chain: usize,
    rect_vec: usize,
) -> Result<PresentInputs, PresentInputError> {
    let profile = state::hook_profile().ok_or(PresentInputError::MissingProfile)?;
    unsafe { collect_present_inputs_with_profile(&profile, overlay_swap_chain, rect_vec) }
}

pub(crate) unsafe fn collect_present_inputs_with_profile(
    profile: &HookProfile,
    overlay_swap_chain: usize,
    rect_vec: usize,
) -> Result<PresentInputs, PresentInputError> {
    unsafe {
        collect_present_inputs_with_profile_and_reader(
            &process_memory_reader(),
            profile,
            overlay_swap_chain,
            rect_vec,
        )
    }
}

unsafe fn collect_present_inputs_with_profile_and_reader(
    reader: &impl MemoryReader,
    profile: &HookProfile,
    overlay_swap_chain: usize,
    rect_vec: usize,
) -> Result<PresentInputs, PresentInputError> {
    if overlay_swap_chain == 0 {
        return Err(PresentInputError::NullOverlaySwapChain);
    }

    let hardware_protected = unsafe {
        reader.read::<u8>(checked_address(
            overlay_swap_chain,
            profile.hardware_protected_offset,
        )?)? != 0
    };
    let monitor_identity =
        unsafe { read_monitor_identity_with(reader, overlay_swap_chain, profile.monitor_identity) };
    let dirty_rects = unsafe { read_dirty_rects_with(reader, rect_vec)? };
    Ok(PresentInputs {
        monitor_identity,
        dirty_rects,
        hardware_protected,
    })
}

unsafe fn read_monitor_identity_with(
    reader: &impl MemoryReader,
    overlay_swap_chain: usize,
    offsets: MonitorIdentityOffsets,
) -> Option<MonitorIdentity> {
    let low_part = unsafe {
        reader
            .read::<u32>(checked_address(overlay_swap_chain, offsets.adapter_luid_low_offset).ok()?)
            .ok()?
    };
    let high_part = unsafe {
        reader
            .read::<i32>(
                checked_address(overlay_swap_chain, offsets.adapter_luid_high_offset).ok()?,
            )
            .ok()?
    };
    let target_id = unsafe {
        reader
            .read::<u32>(checked_address(overlay_swap_chain, offsets.target_id_offset).ok()?)
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

#[cfg(test)]
pub(crate) unsafe fn read_dirty_rects(
    rect_vec: usize,
) -> Result<Vec<DirtyRect>, PresentInputError> {
    unsafe { read_dirty_rects_with(&process_memory_reader(), rect_vec) }
}

unsafe fn read_dirty_rects_with(
    reader: &impl MemoryReader,
    rect_vec: usize,
) -> Result<Vec<DirtyRect>, PresentInputError> {
    if rect_vec == 0 {
        return Err(PresentInputError::InvalidDirtyRectVector);
    }

    let rect_vec = unsafe { reader.read::<RectVec>(rect_vec)? };
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

    unsafe { reader.read_dirty_rect_slice(start, count) }
}

pub(crate) fn checked_address(base: usize, offset: usize) -> Result<usize, PresentInputError> {
    base.checked_add(offset)
        .ok_or(PresentInputError::UnreadableMemory)
}

trait MemoryReader {
    fn is_readable(&self, address: usize, size: usize) -> bool;
    unsafe fn read<T: Copy>(&self, address: usize) -> Result<T, PresentInputError>;
    unsafe fn read_dirty_rect_slice(
        &self,
        address: usize,
        count: usize,
    ) -> Result<Vec<DirtyRect>, PresentInputError>;
}

#[cfg(not(test))]
#[derive(Clone, Copy, Debug, Default)]
struct Win32MemoryReader;

#[cfg(test)]
#[derive(Clone, Copy, Debug, Default)]
struct DirectMemoryReader;

fn process_memory_reader() -> impl MemoryReader {
    #[cfg(test)]
    {
        DirectMemoryReader
    }
    #[cfg(not(test))]
    {
        Win32MemoryReader
    }
}

#[cfg(test)]
impl MemoryReader for DirectMemoryReader {
    fn is_readable(&self, address: usize, size: usize) -> bool {
        address != 0 && size != 0 && address.checked_add(size - 1).is_some()
    }

    unsafe fn read<T: Copy>(&self, address: usize) -> Result<T, PresentInputError> {
        if !self.is_readable(address, size_of::<T>()) {
            return Err(PresentInputError::UnreadableMemory);
        }
        Ok(unsafe { (address as *const T).read_unaligned() })
    }

    unsafe fn read_dirty_rect_slice(
        &self,
        address: usize,
        count: usize,
    ) -> Result<Vec<DirtyRect>, PresentInputError> {
        if count == 0 {
            return Ok(Vec::new());
        }
        let Some(byte_len) = count.checked_mul(size_of::<DirtyRect>()) else {
            return Err(PresentInputError::UnreadableMemory);
        };
        if !self.is_readable(address, byte_len) {
            return Err(PresentInputError::UnreadableMemory);
        }

        let mut values = Vec::with_capacity(count);
        for index in 0..count {
            let Some(offset) = index.checked_mul(size_of::<DirtyRect>()) else {
                return Err(PresentInputError::UnreadableMemory);
            };
            let item_address = checked_address(address, offset)?;
            values.push(unsafe { (item_address as *const DirtyRect).read_unaligned() });
        }
        Ok(values)
    }
}

#[cfg(not(test))]
impl MemoryReader for Win32MemoryReader {
    fn is_readable(&self, address: usize, size: usize) -> bool {
        if address == 0 || size == 0 {
            return false;
        }
        let Some(end) = address.checked_add(size - 1) else {
            return false;
        };
        is_readable_range_in_process(address, end)
    }

    unsafe fn read<T: Copy>(&self, address: usize) -> Result<T, PresentInputError> {
        if !self.is_readable(address, size_of::<T>()) {
            return Err(PresentInputError::UnreadableMemory);
        }

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

    unsafe fn read_dirty_rect_slice(
        &self,
        address: usize,
        count: usize,
    ) -> Result<Vec<DirtyRect>, PresentInputError> {
        if count == 0 {
            return Ok(Vec::new());
        }
        let Some(byte_len) = count.checked_mul(size_of::<DirtyRect>()) else {
            return Err(PresentInputError::UnreadableMemory);
        };
        if address == 0 || address.checked_add(byte_len).is_none() {
            return Err(PresentInputError::UnreadableMemory);
        }

        let mut values = Vec::<DirtyRect>::with_capacity(count);
        let mut bytes_read = 0usize;
        let result = unsafe {
            ReadProcessMemory(
                GetCurrentProcess(),
                address as *const c_void,
                values.as_mut_ptr().cast(),
                byte_len,
                Some(&mut bytes_read),
            )
        };
        if result.is_err() || bytes_read != byte_len {
            return Err(PresentInputError::UnreadableMemory);
        }
        unsafe {
            values.set_len(count);
        }
        Ok(values)
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

pub(crate) fn is_readable_memory_region(info: &MEMORY_BASIC_INFORMATION) -> bool {
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

#[cfg(test)]
mod tests {
    use std::mem::size_of;

    use windows::Win32::System::Memory::{
        MEM_COMMIT, PAGE_EXECUTE, PAGE_GUARD, PAGE_READONLY, PAGE_READWRITE,
    };

    use super::super::test_support::{
        FakePresentObjects, initialize_test_state, test_monitor_identity, test_profile,
    };
    use super::DirtyRect;
    use crate::state::HOOK_GLOBAL_TEST_LOCK;

    use super::{
        PresentInputError, RectVec, checked_address, collect_present_inputs_with_profile,
        is_readable_memory_region, read_dirty_rects,
    };

    #[test]
    fn present_input_collection_reads_confirmed_inputs_without_swap_chain_accessor() {
        let _guard = HOOK_GLOBAL_TEST_LOCK
            .lock()
            .expect("test mutex should lock");
        initialize_test_state();
        let fake = FakePresentObjects::new(
            vec![DirtyRect {
                left: 10,
                top: 20,
                right: 30,
                bottom: 40,
            }],
            false,
        );

        let inputs = unsafe {
            collect_present_inputs_with_profile(
                &test_profile(),
                fake.overlay_swap_chain_address(),
                fake.rect_vec_address(),
            )
        }
        .expect("present inputs should be collected");

        assert_eq!(inputs.monitor_identity, Some(test_monitor_identity()));
        assert_eq!(inputs.dirty_rects, fake.dirty_rects);
        assert!(!inputs.hardware_protected);
    }

    #[test]
    fn present_input_collection_reads_confirmed_inputs_when_hardware_protected() {
        let _guard = HOOK_GLOBAL_TEST_LOCK
            .lock()
            .expect("test mutex should lock");
        initialize_test_state();
        let fake = FakePresentObjects::new(
            vec![DirtyRect {
                left: 10,
                top: 20,
                right: 30,
                bottom: 40,
            }],
            true,
        );

        let inputs = unsafe {
            collect_present_inputs_with_profile(
                &test_profile(),
                fake.overlay_swap_chain_address(),
                fake.rect_vec_address(),
            )
        }
        .expect("hardware protected state should be collected");

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
        let rect_vec = RectVec {
            start: rects.as_ptr(),
            end: unsafe { rects.as_ptr().add(1) },
            capacity_end: rects.as_ptr(),
        };

        let error = unsafe { read_dirty_rects((&rect_vec as *const RectVec) as usize) }
            .expect_err("end past capacity should be rejected");

        assert_eq!(error, PresentInputError::InvalidDirtyRectVector);
    }

    #[test]
    fn present_input_collection_rejects_misaligned_dirty_rect_vector() {
        let start = std::ptr::dangling::<DirtyRect>() as usize + 1;
        let rect_vec = RectVec {
            start: start as *const DirtyRect,
            end: (start + size_of::<DirtyRect>()) as *const DirtyRect,
            capacity_end: (start + size_of::<DirtyRect>()) as *const DirtyRect,
        };

        let error = unsafe { read_dirty_rects((&rect_vec as *const RectVec) as usize) }
            .expect_err("misaligned start should be rejected");

        assert_eq!(error, PresentInputError::InvalidDirtyRectVector);
    }

    #[test]
    fn pointer_addition_overflow_is_not_treated_as_readable_memory() {
        let error =
            checked_address(usize::MAX, 1).expect_err("overflowed address should be rejected");

        assert_eq!(error, PresentInputError::UnreadableMemory);
    }

    #[test]
    fn null_dirty_rect_vector_is_invalid_present_input() {
        let error =
            unsafe { read_dirty_rects(0) }.expect_err("null rectVec pointer should be rejected");

        assert_eq!(error, PresentInputError::InvalidDirtyRectVector);
    }

    #[test]
    fn memory_region_readability_requires_read_protection() {
        let readable = windows::Win32::System::Memory::MEMORY_BASIC_INFORMATION {
            RegionSize: 4096,
            State: MEM_COMMIT,
            Protect: PAGE_READONLY,
            ..Default::default()
        };
        let execute_only = windows::Win32::System::Memory::MEMORY_BASIC_INFORMATION {
            Protect: PAGE_EXECUTE,
            ..readable
        };
        let guarded = windows::Win32::System::Memory::MEMORY_BASIC_INFORMATION {
            Protect: PAGE_READWRITE | PAGE_GUARD,
            ..readable
        };

        assert!(is_readable_memory_region(&readable));
        assert!(!is_readable_memory_region(&execute_only));
        assert!(!is_readable_memory_region(&guarded));
    }
}
