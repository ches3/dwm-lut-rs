use std::sync::{Arc, Mutex, MutexGuard, OnceLock, TryLockError};

use dwm_lut_payload::{ColorMode, HookPayload, MonitorIdentity, MonitorTarget, PayloadLut};

use crate::flip_gate::FlipGateEffects;
use crate::minhook::{MinHookRuntime, RegisteredHook};
use dwm_lut_profile::HookProfile;

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct LutMetadata {
    pub size: u32,
    pub domain_min: [f32; 3],
    pub domain_max: [f32; 3],
}

#[derive(Debug, Clone, PartialEq)]
pub struct ShaderTexture3D {
    pub width: u32,
    pub height: u32,
    pub depth: u32,
    pub texels: Vec<[f32; 4]>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct LutAssignment {
    pub target: MonitorTarget,
    pub metadata: LutMetadata,
    pub texture: ShaderTexture3D,
}

pub fn assignments_from_payload(payload: &HookPayload) -> Vec<LutAssignment> {
    let mut assignments = Vec::with_capacity(payload.assignments.len());
    for assignment in &payload.assignments {
        assignments.push(LutAssignment {
            target: assignment.target,
            metadata: LutMetadata {
                size: assignment.lut.size,
                domain_min: assignment.lut.domain_min,
                domain_max: assignment.lut.domain_max,
            },
            texture: cube_to_texture(&assignment.lut),
        });
    }
    assignments
}

pub fn cube_to_texture(cube: &PayloadLut) -> ShaderTexture3D {
    let texels = cube
        .values
        .iter()
        .map(|value| [value[0], value[1], value[2], 1.0])
        .collect();

    ShaderTexture3D {
        width: cube.size,
        height: cube.size,
        depth: cube.size,
        texels,
    }
}

pub(crate) fn find_assignment(
    assignments: &[LutAssignment],
    identity: MonitorIdentity,
    color_mode: ColorMode,
) -> Option<(usize, &LutAssignment)> {
    assignments.iter().enumerate().find(|(_, assignment)| {
        assignment.target.identity == identity && assignment.target.color_mode == color_mode
    })
}

#[derive(Debug, Clone, PartialEq)]
pub struct HookRuntime {
    pub minhook: MinHookRuntime,
    pub hooks: Vec<RegisteredHook>,
    pub flip_gate_effects: FlipGateEffects,
}

#[derive(Debug, Clone, PartialEq)]
pub struct HookState {
    pub profile_name: String,
    pub profile: HookProfile,
    pub assignments: Arc<Vec<LutAssignment>>,
    pub runtime: HookRuntime,
}

static STATE: OnceLock<Mutex<Option<HookState>>> = OnceLock::new();

static RETAINED_STATE: OnceLock<Mutex<Option<HookState>>> = OnceLock::new();

static PRESENT_RUNTIME_LOCK: OnceLock<Mutex<()>> = OnceLock::new();

#[cfg(test)]
pub(crate) static HOOK_GLOBAL_TEST_LOCK: Mutex<()> = Mutex::new(());

pub(crate) fn install_state(state: HookState) -> Result<(), Box<HookState>> {
    let slot = STATE.get_or_init(|| Mutex::new(None));
    let Ok(mut slot) = slot.lock() else {
        return Err(Box::new(state));
    };
    if slot.is_some() {
        return Err(Box::new(state));
    }
    *slot = Some(state);
    Ok(())
}

pub(crate) fn has_retained_state() -> bool {
    RETAINED_STATE
        .get()
        .and_then(|state| state.lock().ok().map(|guard| guard.is_some()))
        .unwrap_or(false)
}

pub fn hook_profile() -> Option<HookProfile> {
    with_state(|state| state.profile)
}

pub(crate) fn active_profile_name() -> Option<String> {
    with_state(|state| state.profile_name.clone())
}

pub(crate) fn assignments() -> Option<Arc<Vec<LutAssignment>>> {
    with_state(|state| state.assignments.clone())
}

fn present_runtime_lock() -> &'static Mutex<()> {
    PRESENT_RUNTIME_LOCK.get_or_init(|| Mutex::new(()))
}

pub(crate) fn lock_present_runtime() -> MutexGuard<'static, ()> {
    present_runtime_lock()
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
}

pub(crate) fn try_lock_present_runtime() -> Option<MutexGuard<'static, ()>> {
    match present_runtime_lock().try_lock() {
        Ok(guard) => Some(guard),
        Err(TryLockError::Poisoned(poisoned)) => Some(poisoned.into_inner()),
        Err(TryLockError::WouldBlock) => None,
    }
}

pub(crate) fn clear_state_after_shutdown() {
    set_flip_gate(false);
    if let Some(state) = STATE.get()
        && let Ok(mut guard) = state.lock()
    {
        let _ = guard.take();
    }
    if let Some(state) = RETAINED_STATE.get()
        && let Ok(mut guard) = state.lock()
        && let Some(mut state) = guard.take()
    {
        state.runtime.flip_gate_effects.set_enabled(false);
    }
}

pub(crate) fn retain_state_after_shutdown() {
    set_flip_gate(false);
    let active = STATE.get_or_init(|| Mutex::new(None));
    let retained = RETAINED_STATE.get_or_init(|| Mutex::new(None));
    if let (Ok(mut active), Ok(mut retained)) = (active.lock(), retained.lock())
        && let Some(state) = active.take()
    {
        *retained = Some(state);
    }
}

pub(crate) fn reactivate_retained_state(
    profile_name: String,
    assignments: Vec<LutAssignment>,
) -> Option<(MinHookRuntime, Vec<RegisteredHook>)> {
    let active = STATE.get_or_init(|| Mutex::new(None));
    let retained = RETAINED_STATE.get_or_init(|| Mutex::new(None));
    let (Ok(mut active), Ok(mut retained)) = (active.lock(), retained.lock()) else {
        return None;
    };
    if active.is_some() {
        return None;
    }
    let mut state = retained.take()?;
    update_lut_assignments(&mut state, profile_name, assignments);
    let plan = (state.runtime.minhook, state.runtime.hooks.clone());
    *active = Some(state);
    Some(plan)
}

pub(crate) fn set_flip_gate(enabled: bool) {
    crate::flip_gate::set_enabled(enabled);
    let _ = with_state_mut(|state| {
        state.runtime.flip_gate_effects.set_enabled(enabled);
    });
}

pub(crate) fn minhook_cleanup_plan() -> Option<(MinHookRuntime, Vec<RegisteredHook>)> {
    with_state(|state| (state.runtime.minhook, state.runtime.hooks.clone()))
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ReplaceLutAssignmentsError {
    NotInitialized,
}

pub fn replace_lut_assignments(
    profile_name: String,
    assignments: Vec<LutAssignment>,
) -> Result<(), ReplaceLutAssignmentsError> {
    with_state_mut(|state| {
        update_lut_assignments(state, profile_name, assignments);
    })
    .ok_or(ReplaceLutAssignmentsError::NotInitialized)?;
    Ok(())
}

fn update_lut_assignments(
    state: &mut HookState,
    profile_name: String,
    assignments: Vec<LutAssignment>,
) {
    state.profile_name = profile_name;
    state.assignments = Arc::new(assignments);
}

fn with_state<R>(f: impl FnOnce(&HookState) -> R) -> Option<R> {
    let state = STATE.get()?;
    let guard = state.lock().ok()?;
    guard.as_ref().map(f)
}

fn with_state_mut<R>(f: impl FnOnce(&mut HookState) -> R) -> Option<R> {
    let state = STATE.get()?;
    let mut guard = state.lock().ok()?;
    guard.as_mut().map(f)
}

#[cfg(test)]
pub(crate) fn reset_state_for_tests() {
    clear_state_after_shutdown();
    crate::lifecycle::reset_for_tests();
    crate::d3d11::reset_fake_render_result();
    crate::minhook::reset_test_minhook_behavior(None, None, None, None);
    crate::minhook::reset_test_original_slots();
}

#[cfg(test)]
mod tests {
    use dwm_lut_payload::{
        AdapterLuid, ColorMode, HookPayload, MonitorIdentity, MonitorTarget, PayloadAssignment,
        PayloadLut,
    };

    use super::{assignments_from_payload, find_assignment};

    fn identity_cube() -> PayloadLut {
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

    fn payload(
        assignments: impl IntoIterator<Item = (MonitorIdentity, ColorMode, PayloadLut)>,
    ) -> HookPayload {
        HookPayload {
            profile_name: "test".to_string(),
            assignments: assignments
                .into_iter()
                .map(|(identity, color_mode, lut)| PayloadAssignment {
                    target: MonitorTarget {
                        identity,
                        color_mode,
                    },
                    lut,
                })
                .collect(),
            flip_gate_enabled: true,
        }
    }

    #[test]
    fn find_assignment_selects_by_identity_and_color_mode() {
        let identity_a = MonitorIdentity {
            adapter_luid: AdapterLuid {
                high_part: 0,
                low_part: 0x14e02,
            },
            target_id: 11,
        };
        let identity_b = MonitorIdentity {
            adapter_luid: AdapterLuid {
                high_part: 0,
                low_part: 0x14e02,
            },
            target_id: 4357,
        };
        let assignments = assignments_from_payload(&payload([
            (identity_a, ColorMode::Sdr, identity_cube()),
            (identity_b, ColorMode::Sdr, identity_cube()),
            (identity_b, ColorMode::Hdr, identity_cube()),
        ]));

        assert_eq!(
            find_assignment(&assignments, identity_b, ColorMode::Sdr).map(|(index, _)| index),
            Some(1)
        );
        assert_eq!(
            find_assignment(&assignments, identity_b, ColorMode::Hdr).map(|(index, _)| index),
            Some(2)
        );
        assert!(find_assignment(&assignments, identity_a, ColorMode::Hdr).is_none());
    }
}
