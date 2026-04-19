pub type MhStatus = i32;

pub type MhInitializeApi = unsafe extern "system" fn() -> MhStatus;
pub type MhCreateHookApi = unsafe extern "system" fn(
    target: *mut core::ffi::c_void,
    detour: *mut core::ffi::c_void,
    original: *mut *mut core::ffi::c_void,
) -> MhStatus;
pub type MhEnableHookApi = unsafe extern "system" fn(target: *mut core::ffi::c_void) -> MhStatus;
pub type MhDisableHookApi = unsafe extern "system" fn(target: *mut core::ffi::c_void) -> MhStatus;
pub type MhRemoveHookApi = unsafe extern "system" fn(target: *mut core::ffi::c_void) -> MhStatus;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MinHookBindings {
    pub initialize: &'static str,
    pub create_hook: &'static str,
    pub enable_hook: &'static str,
    pub disable_hook: &'static str,
    pub remove_hook: &'static str,
}

impl MinHookBindings {
    pub const fn new() -> Self {
        Self {
            initialize: "MH_Initialize",
            create_hook: "MH_CreateHook",
            enable_hook: "MH_EnableHook",
            disable_hook: "MH_DisableHook",
            remove_hook: "MH_RemoveHook",
        }
    }
}

impl Default for MinHookBindings {
    fn default() -> Self {
        Self::new()
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum MinHookState {
    BoundaryDefined,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MinHookRuntime {
    pub bindings: MinHookBindings,
    pub state: MinHookState,
}

impl MinHookRuntime {
    pub fn boundary_defined() -> Self {
        Self {
            bindings: MinHookBindings::new(),
            state: MinHookState::BoundaryDefined,
        }
    }
}
