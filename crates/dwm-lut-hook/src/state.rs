use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, OnceLock};

use arc_swap::ArcSwapOption;
use dwm_lut_payload::{ColorMode, HookPayload, MonitorIdentity, MonitorTarget, PayloadLut};
use parking_lot::{Mutex, MutexGuard};

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

#[derive(Debug, Clone, PartialEq)]
pub struct LutConfig {
    pub profile_name: String,
    pub assignments: Arc<Vec<LutAssignment>>,
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
}

static HOOK_RUNTIME: Mutex<Option<HookRuntime>> = Mutex::new(None);
static FLIP_GATE_ENABLED: AtomicBool = AtomicBool::new(false);
static LUT_CONFIG: ArcSwapOption<LutConfig> = ArcSwapOption::const_empty();
static PROFILE: OnceLock<HookProfile> = OnceLock::new();
static PRESENT_RUNTIME_LOCK: Mutex<()> = Mutex::new(());

#[cfg(test)]
pub(crate) static HOOK_GLOBAL_TEST_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

fn lock_hook_runtime() -> MutexGuard<'static, Option<HookRuntime>> {
    HOOK_RUNTIME.lock()
}

pub(crate) fn has_hook_runtime() -> bool {
    lock_hook_runtime().is_some()
}

pub(crate) fn store_hook_runtime(runtime: HookRuntime) -> Result<(), HookRuntime> {
    let mut stored = lock_hook_runtime();
    if stored.is_some() {
        return Err(runtime);
    }
    *stored = Some(runtime);
    Ok(())
}

pub(crate) fn clear_hook_runtime() {
    let _ = lock_hook_runtime().take();
}

pub(crate) fn clone_hook_runtime() -> Option<HookRuntime> {
    lock_hook_runtime().clone()
}

pub(crate) fn store_lut_config(config: LutConfig) {
    LUT_CONFIG.store(Some(Arc::new(config)));
}

pub(crate) fn lut_config() -> Option<Arc<LutConfig>> {
    LUT_CONFIG.load_full()
}

pub(crate) fn assignments() -> Option<Arc<Vec<LutAssignment>>> {
    lut_config().map(|config| Arc::clone(&config.assignments))
}

pub(crate) fn lut_profile_name() -> Option<String> {
    lut_config().map(|config| config.profile_name.clone())
}

pub(crate) fn clear_lut_config() {
    LUT_CONFIG.store(None::<Arc<LutConfig>>);
}

pub(crate) fn flip_gate_enabled() -> bool {
    FLIP_GATE_ENABLED.load(Ordering::Acquire)
}

pub(crate) fn store_flip_gate_enabled(enabled: bool) {
    FLIP_GATE_ENABLED.store(enabled, Ordering::Release);
}

pub(crate) fn store_hook_profile(profile: HookProfile) -> Result<(), HookProfile> {
    PROFILE.set(profile)
}

pub fn hook_profile() -> Option<HookProfile> {
    PROFILE.get().copied()
}

pub(crate) fn lock_present_runtime() -> MutexGuard<'static, ()> {
    PRESENT_RUNTIME_LOCK.lock()
}

pub(crate) fn try_lock_present_runtime() -> Option<MutexGuard<'static, ()>> {
    PRESENT_RUNTIME_LOCK.try_lock()
}

#[cfg(test)]
pub(crate) fn reset_state_for_tests() {
    let test_profile = (dwm_lut_profile::SUPPORTED_BUILDS
        .first()
        .expect("SUPPORTED_BUILDS is non-empty")
        .profiles
        .last()
        .expect("supported build must include profiles")
        .profile)();
    if PROFILE.set(test_profile).is_err() {
        assert_eq!(
            PROFILE.get().copied(),
            Some(test_profile),
            "all tests must use the same process-wide hook profile",
        );
    }

    clear_lut_config();
    store_flip_gate_enabled(false);
    clear_hook_runtime();
    crate::lifecycle::reset_for_tests();
    crate::d3d11::reset_fake_render_result();
    crate::minhook::reset_test_minhook_behavior(None, None, None, None);
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
