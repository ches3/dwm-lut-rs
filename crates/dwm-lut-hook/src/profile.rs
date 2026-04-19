#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BuildProfile {
    Windows11_25H2,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HookTarget {
    Present,
    IsCandidateDirectFlipCompatible,
    OverlaysEnabled,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AobToken {
    Exact(u8),
    Wildcard,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SignatureLocator {
    DeferredAob {
        module_name: &'static str,
        capture_key: &'static str,
        tokens: &'static [AobToken],
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SignatureStage {
    Phase4Required,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HookSignature {
    pub target: HookTarget,
    pub locator: SignatureLocator,
    pub stage: SignatureStage,
    pub note: &'static str,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HookProfile {
    pub build: BuildProfile,
    pub module_name: &'static str,
    pub signatures: Vec<HookSignature>,
}

impl HookProfile {
    pub fn for_build(build: BuildProfile) -> Self {
        match build {
            BuildProfile::Windows11_25H2 => windows_11_25h2(),
        }
    }
}

const PRESENT_AOB: &[AobToken] = &[];
const DIRECT_FLIP_AOB: &[AobToken] = &[];
const OVERLAYS_ENABLED_AOB: &[AobToken] = &[];

fn windows_11_25h2() -> HookProfile {
    HookProfile {
        build: BuildProfile::Windows11_25H2,
        module_name: "dwmcore.dll",
        signatures: vec![
            HookSignature {
                target: HookTarget::Present,
                locator: SignatureLocator::DeferredAob {
                    module_name: "dwmcore.dll",
                    capture_key: "present_25h2",
                    tokens: PRESENT_AOB,
                },
                stage: SignatureStage::Phase4Required,
                note: "Present hook resolution is captured as a dedicated 25H2 AOB slot.",
            },
            HookSignature {
                target: HookTarget::IsCandidateDirectFlipCompatible,
                locator: SignatureLocator::DeferredAob {
                    module_name: "dwmcore.dll",
                    capture_key: "direct_flip_compat_25h2",
                    tokens: DIRECT_FLIP_AOB,
                },
                stage: SignatureStage::Phase4Required,
                note: "DirectFlip suppression depends on a 25H2-specific AOB entry.",
            },
            HookSignature {
                target: HookTarget::OverlaysEnabled,
                locator: SignatureLocator::DeferredAob {
                    module_name: "dwmcore.dll",
                    capture_key: "overlays_enabled_25h2",
                    tokens: OVERLAYS_ENABLED_AOB,
                },
                stage: SignatureStage::Phase4Required,
                note: "Overlay suppression is tracked as a separate profile entry.",
            },
        ],
    }
}
