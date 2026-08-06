use crate::dwmcore::{self, DirtyRect, DirtyRectReadError};
use crate::state;
use dwm_lut_payload::MonitorIdentity;
use dwm_lut_profile::HookProfile;

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
    if overlay_swap_chain == 0 {
        return Err(PresentInputError::NullOverlaySwapChain);
    }

    let hardware_protected =
        dwmcore::read_hardware_protected(overlay_swap_chain, profile.hardware_protected_offset)
            .ok_or(PresentInputError::UnreadableMemory)?;
    let monitor_identity =
        dwmcore::read_monitor_identity(overlay_swap_chain, profile.monitor_identity_offsets);
    let dirty_rects =
        unsafe { dwmcore::read_dirty_rects(rect_vec) }.map_err(|error| match error {
            DirtyRectReadError::InvalidVector => PresentInputError::InvalidDirtyRectVector,
            DirtyRectReadError::UnreadableMemory => PresentInputError::UnreadableMemory,
        })?;
    Ok(PresentInputs {
        monitor_identity,
        dirty_rects,
        hardware_protected,
    })
}

#[cfg(test)]
mod tests {
    use super::super::test_support::{FakePresentObjects, test_monitor_identity, test_profile};
    use super::{PresentInputError, collect_present_inputs_with_profile};
    use crate::dwmcore::DirtyRect;

    #[test]
    fn present_input_collection_reads_confirmed_inputs_without_swap_chain_accessor() {
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
    fn null_dirty_rect_vector_is_invalid_present_input() {
        let fake = FakePresentObjects::new(Vec::new(), false);
        let error = unsafe {
            collect_present_inputs_with_profile(
                &test_profile(),
                fake.overlay_swap_chain_address(),
                0,
            )
        }
        .expect_err("null rectVec pointer should be rejected");

        assert_eq!(error, PresentInputError::InvalidDirtyRectVector);
    }
}
