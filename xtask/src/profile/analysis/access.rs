use dwm_lut_profile::{
    AccessFieldTarget, AccessMethodExtractSpec, DirectFieldWidth, FieldOffsetExtractSpec,
    FieldOffsetTarget, ProfileAccessTarget, VirtualBaseComponent, VirtualBaseTarget,
    VirtualMethodTarget,
};

use super::model::{
    ExtractionStatus, FieldOffsetExtractionRow, VirtualBaseExtractionRow, VirtualMethodAbiStatus,
    VirtualMethodExtractionRow,
};

pub struct RawAccessExtraction {
    pub virtual_methods: Vec<VirtualMethodExtractionRow>,
    pub field_offsets: Vec<FieldOffsetExtractionRow>,
    pub virtual_bases: Vec<VirtualBaseExtractionRow>,
}

pub const fn access_field_label(field: AccessFieldTarget) -> &'static str {
    match field {
        AccessFieldTarget::VirtualMethod(target) => target.label(),
        AccessFieldTarget::FieldOffset(target) => target.label(),
        AccessFieldTarget::VirtualBaseOffset { component, .. } => match component {
            VirtualBaseComponent::VbptrOffset => "vbptr_offset",
            VirtualBaseComponent::VbtableEntryOffset => "vbtable_entry_offset",
        },
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AccessReport {
    pub target: ProfileAccessTarget,
    pub result: AccessResult,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AccessResult {
    Selected(Vec<AccessRow>),
    AllFailed(Vec<AccessRow>),
}

impl AccessResult {
    pub fn rows(&self) -> &[AccessRow] {
        match self {
            Self::Selected(rows) | Self::AllFailed(rows) => rows,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AccessRow {
    pub field: AccessFieldTarget,
    pub status: AccessStatus,
    pub value: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AccessStatus {
    Ok,
    Missing,
    NoSymbol,
    NoVftable,
    NoDisp,
    Ambiguous,
    LoadSizeMismatch,
    AbiMismatch,
    AbiUnsupported,
    AbiUnavailable,
}

impl AccessStatus {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Ok => "ok",
            Self::Missing => "missing",
            Self::NoSymbol => "no_symbol",
            Self::NoVftable => "no_vftable",
            Self::NoDisp => "no_disp",
            Self::Ambiguous => "ambiguous",
            Self::LoadSizeMismatch => "load_size_mismatch",
            Self::AbiMismatch => "abi_mismatch",
            Self::AbiUnsupported => "abi_unsupported",
            Self::AbiUnavailable => "abi_unavailable",
        }
    }

    pub const fn is_ok(self) -> bool {
        matches!(self, Self::Ok)
    }
}

pub fn build_access_reports(raw: &RawAccessExtraction) -> Vec<AccessReport> {
    ProfileAccessTarget::ALL
        .iter()
        .copied()
        .map(|target| evaluate_access(target, raw))
        .collect()
}

fn evaluate_access(target: ProfileAccessTarget, raw: &RawAccessExtraction) -> AccessReport {
    let evaluated = target
        .extract_spec()
        .methods
        .iter()
        .map(|method| evaluate_method(method, raw))
        .collect::<Vec<_>>();

    let result = match evaluated.iter().position(EvaluatedMethod::is_ok) {
        Some(selected_index) => AccessResult::Selected(
            evaluated
                .into_iter()
                .nth(selected_index)
                .expect("selected access method exists")
                .rows,
        ),
        None => AccessResult::AllFailed(
            evaluated
                .into_iter()
                .flat_map(|method| method.rows)
                .collect(),
        ),
    };

    AccessReport { target, result }
}

struct EvaluatedMethod {
    rows: Vec<AccessRow>,
}

impl EvaluatedMethod {
    fn is_ok(&self) -> bool {
        !self.rows.is_empty() && self.rows.iter().all(|row| row.status.is_ok())
    }
}

fn evaluate_method(method: &AccessMethodExtractSpec, raw: &RawAccessExtraction) -> EvaluatedMethod {
    EvaluatedMethod {
        rows: method
            .fields
            .iter()
            .copied()
            .map(|field| evaluate_field(field, raw))
            .collect(),
    }
}

fn evaluate_field(field: AccessFieldTarget, raw: &RawAccessExtraction) -> AccessRow {
    match field {
        AccessFieldTarget::VirtualMethod(target) => evaluate_virtual_method(field, target, raw),
        AccessFieldTarget::FieldOffset(target) => evaluate_field_offset(field, target, raw),
        AccessFieldTarget::VirtualBaseOffset { target, component } => {
            evaluate_virtual_base(field, target, component, raw)
        }
    }
}

fn evaluate_virtual_method(
    field: AccessFieldTarget,
    target: VirtualMethodTarget,
    raw: &RawAccessExtraction,
) -> AccessRow {
    let Some(extracted) = raw.virtual_methods.iter().find(|row| row.target == target) else {
        return missing_row(field);
    };

    let value = extracted.value.map(|value| value.to_string());
    let status = if extracted.status != ExtractionStatus::Ok {
        extraction_status(extracted.status)
    } else {
        match extracted.abi_status {
            VirtualMethodAbiStatus::Ok => AccessStatus::Ok,
            VirtualMethodAbiStatus::Mismatch => AccessStatus::AbiMismatch,
            VirtualMethodAbiStatus::Unsupported => AccessStatus::AbiUnsupported,
            VirtualMethodAbiStatus::Unavailable => AccessStatus::AbiUnavailable,
        }
    };

    AccessRow {
        field,
        status,
        value,
    }
}

fn evaluate_field_offset(
    field: AccessFieldTarget,
    target: FieldOffsetTarget,
    raw: &RawAccessExtraction,
) -> AccessRow {
    let Some(extracted) = raw.field_offsets.iter().find(|row| row.target == target) else {
        return missing_row(field);
    };

    let status = if extracted.status != ExtractionStatus::Ok {
        extraction_status(extracted.status)
    } else {
        match target.extract_spec() {
            FieldOffsetExtractSpec::GetterDisp { width, .. }
                if extracted.load_size != Some(direct_field_width_bytes(width)) =>
            {
                AccessStatus::LoadSizeMismatch
            }
            FieldOffsetExtractSpec::GetterDisp { .. }
            | FieldOffsetExtractSpec::CtorFirstParamStore { .. } => AccessStatus::Ok,
        }
    };

    AccessRow {
        field,
        status,
        value: extracted.value.map(|value| format!("{value:#x}")),
    }
}

const fn direct_field_width_bytes(width: DirectFieldWidth) -> usize {
    match width {
        DirectFieldWidth::U8 => 1,
    }
}

fn evaluate_virtual_base(
    field: AccessFieldTarget,
    target: VirtualBaseTarget,
    component: VirtualBaseComponent,
    raw: &RawAccessExtraction,
) -> AccessRow {
    let Some(extracted) = raw.virtual_bases.iter().find(|row| row.target == target) else {
        return missing_row(field);
    };

    let value = extracted.value.map(|value| match component {
        VirtualBaseComponent::VbptrOffset => format!("{:#x}", value.vbptr_offset),
        VirtualBaseComponent::VbtableEntryOffset => {
            format!("{:#x}", value.vbtable_entry_offset)
        }
    });

    AccessRow {
        field,
        status: extraction_status(extracted.status),
        value,
    }
}

fn missing_row(field: AccessFieldTarget) -> AccessRow {
    AccessRow {
        field,
        status: AccessStatus::Missing,
        value: None,
    }
}

const fn extraction_status(status: ExtractionStatus) -> AccessStatus {
    match status {
        ExtractionStatus::Ok => AccessStatus::Ok,
        ExtractionStatus::NoSymbol => AccessStatus::NoSymbol,
        ExtractionStatus::NoVftable => AccessStatus::NoVftable,
        ExtractionStatus::NoDisp => AccessStatus::NoDisp,
        ExtractionStatus::Ambiguous => AccessStatus::Ambiguous,
    }
}

#[cfg(test)]
mod tests {
    use dwm_lut_profile::{
        AccessFieldTarget, DeviceResourceVirtualBaseAccess, FieldOffsetTarget, ProfileAccessTarget,
        VirtualBaseTarget, VirtualMethodTarget,
    };

    use super::{AccessResult, AccessStatus, RawAccessExtraction, build_access_reports};
    use crate::profile::analysis::model::{
        ExtractionStatus, FieldOffsetExtractionRow, VirtualBaseExtractionRow,
        VirtualMethodAbiStatus, VirtualMethodExtractionRow,
    };

    fn virtual_method(
        target: VirtualMethodTarget,
        status: ExtractionStatus,
        value: Option<usize>,
        abi_status: VirtualMethodAbiStatus,
    ) -> VirtualMethodExtractionRow {
        VirtualMethodExtractionRow {
            target,
            status,
            value,
            abi_status,
        }
    }

    fn field_offset(
        target: FieldOffsetTarget,
        status: ExtractionStatus,
        value: Option<usize>,
        load_size: Option<usize>,
    ) -> FieldOffsetExtractionRow {
        FieldOffsetExtractionRow {
            target,
            status,
            value,
            load_size,
        }
    }

    fn virtual_base(
        status: ExtractionStatus,
        value: Option<DeviceResourceVirtualBaseAccess>,
    ) -> VirtualBaseExtractionRow {
        VirtualBaseExtractionRow {
            target: VirtualBaseTarget::DeviceResourceFromOverlaySwapChain,
            status,
            value,
        }
    }

    fn access(
        reports: &[super::AccessReport],
        target: ProfileAccessTarget,
    ) -> &super::AccessReport {
        reports
            .iter()
            .find(|report| report.target == target)
            .expect("access report")
    }

    #[test]
    fn selects_first_successful_d3d11_method() {
        let raw = RawAccessExtraction {
            virtual_methods: vec![
                virtual_method(
                    VirtualMethodTarget::GetPhysicalBackBuffer,
                    ExtractionStatus::Ok,
                    Some(24),
                    VirtualMethodAbiStatus::Ok,
                ),
                virtual_method(
                    VirtualMethodTarget::GetD3D11Resource,
                    ExtractionStatus::Ok,
                    Some(19),
                    VirtualMethodAbiStatus::Ok,
                ),
                virtual_method(
                    VirtualMethodTarget::GetCurrentBackBuffer,
                    ExtractionStatus::Ok,
                    Some(0),
                    VirtualMethodAbiStatus::Ok,
                ),
                virtual_method(
                    VirtualMethodTarget::GetRenderTargetView,
                    ExtractionStatus::Ok,
                    Some(7),
                    VirtualMethodAbiStatus::Ok,
                ),
            ],
            field_offsets: vec![],
            virtual_bases: vec![],
        };

        let reports = build_access_reports(&raw);
        let report = access(&reports, ProfileAccessTarget::D3D11Resource);
        let AccessResult::Selected(rows) = &report.result else {
            panic!("expected selected method");
        };

        assert_eq!(rows.len(), 2);
        assert_eq!(
            rows[0].field,
            AccessFieldTarget::VirtualMethod(VirtualMethodTarget::GetPhysicalBackBuffer)
        );
        assert_eq!(
            rows[1].field,
            AccessFieldTarget::VirtualMethod(VirtualMethodTarget::GetD3D11Resource)
        );
    }

    #[test]
    fn falls_back_to_second_d3d11_method_when_first_fails() {
        let raw = RawAccessExtraction {
            virtual_methods: vec![
                virtual_method(
                    VirtualMethodTarget::GetPhysicalBackBuffer,
                    ExtractionStatus::NoVftable,
                    None,
                    VirtualMethodAbiStatus::Unavailable,
                ),
                virtual_method(
                    VirtualMethodTarget::GetD3D11Resource,
                    ExtractionStatus::Ok,
                    Some(19),
                    VirtualMethodAbiStatus::Ok,
                ),
                virtual_method(
                    VirtualMethodTarget::GetCurrentBackBuffer,
                    ExtractionStatus::Ok,
                    Some(0),
                    VirtualMethodAbiStatus::Ok,
                ),
                virtual_method(
                    VirtualMethodTarget::GetRenderTargetView,
                    ExtractionStatus::Ok,
                    Some(7),
                    VirtualMethodAbiStatus::Ok,
                ),
            ],
            field_offsets: vec![],
            virtual_bases: vec![],
        };

        let reports = build_access_reports(&raw);
        let report = access(&reports, ProfileAccessTarget::D3D11Resource);
        let AccessResult::Selected(rows) = &report.result else {
            panic!("expected selected method");
        };

        assert_eq!(rows.len(), 2);
        assert_eq!(
            rows[0].field,
            AccessFieldTarget::VirtualMethod(VirtualMethodTarget::GetCurrentBackBuffer)
        );
        assert_eq!(
            rows[1].field,
            AccessFieldTarget::VirtualMethod(VirtualMethodTarget::GetRenderTargetView)
        );
    }

    #[test]
    fn shows_every_d3d11_field_when_all_methods_fail() {
        let raw = RawAccessExtraction {
            virtual_methods: vec![
                virtual_method(
                    VirtualMethodTarget::GetPhysicalBackBuffer,
                    ExtractionStatus::NoVftable,
                    None,
                    VirtualMethodAbiStatus::Unavailable,
                ),
                virtual_method(
                    VirtualMethodTarget::GetD3D11Resource,
                    ExtractionStatus::Ok,
                    Some(19),
                    VirtualMethodAbiStatus::Ok,
                ),
                virtual_method(
                    VirtualMethodTarget::GetCurrentBackBuffer,
                    ExtractionStatus::NoVftable,
                    None,
                    VirtualMethodAbiStatus::Unavailable,
                ),
                virtual_method(
                    VirtualMethodTarget::GetRenderTargetView,
                    ExtractionStatus::NoVftable,
                    None,
                    VirtualMethodAbiStatus::Unavailable,
                ),
            ],
            field_offsets: vec![],
            virtual_bases: vec![],
        };

        let reports = build_access_reports(&raw);
        let report = access(&reports, ProfileAccessTarget::D3D11Resource);
        let AccessResult::AllFailed(rows) = &report.result else {
            panic!("expected all methods");
        };

        assert_eq!(rows.len(), 4);
        assert_eq!(rows[0].status, AccessStatus::NoVftable);
        assert_eq!(rows[1].status, AccessStatus::Ok);
        assert_eq!(rows[2].status, AccessStatus::NoVftable);
        assert_eq!(rows[3].status, AccessStatus::NoVftable);
    }

    #[test]
    fn virtual_method_abi_failure_rejects_method_and_keeps_index() {
        let raw = RawAccessExtraction {
            virtual_methods: vec![
                virtual_method(
                    VirtualMethodTarget::GetPhysicalBackBuffer,
                    ExtractionStatus::Ok,
                    Some(24),
                    VirtualMethodAbiStatus::Ok,
                ),
                virtual_method(
                    VirtualMethodTarget::GetD3D11Resource,
                    ExtractionStatus::Ok,
                    Some(19),
                    VirtualMethodAbiStatus::Mismatch,
                ),
            ],
            field_offsets: vec![],
            virtual_bases: vec![],
        };

        let reports = build_access_reports(&raw);
        let report = access(&reports, ProfileAccessTarget::D3D11Resource);
        let AccessResult::AllFailed(rows) = &report.result else {
            panic!("expected all methods");
        };

        assert_eq!(rows[1].status, AccessStatus::AbiMismatch);
        assert_eq!(rows[1].value.as_deref(), Some("19"));
    }

    #[test]
    fn direct_field_success_hides_virtual_base_method() {
        let raw = RawAccessExtraction {
            virtual_methods: vec![],
            field_offsets: vec![field_offset(
                FieldOffsetTarget::HardwareProtected,
                ExtractionStatus::Ok,
                Some(0x64),
                Some(1),
            )],
            virtual_bases: vec![virtual_base(ExtractionStatus::NoDisp, None)],
        };

        let reports = build_access_reports(&raw);
        let report = access(&reports, ProfileAccessTarget::HardwareProtected);
        let AccessResult::Selected(rows) = &report.result else {
            panic!("expected selected method");
        };

        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].value.as_deref(), Some("0x64"));
    }

    #[test]
    fn direct_field_load_size_mismatch_falls_back_to_virtual_base_method() {
        let raw = RawAccessExtraction {
            virtual_methods: vec![virtual_method(
                VirtualMethodTarget::IsHardwareProtected,
                ExtractionStatus::Ok,
                Some(3),
                VirtualMethodAbiStatus::Ok,
            )],
            field_offsets: vec![field_offset(
                FieldOffsetTarget::HardwareProtected,
                ExtractionStatus::Ok,
                Some(0x64),
                Some(4),
            )],
            virtual_bases: vec![virtual_base(
                ExtractionStatus::Ok,
                Some(DeviceResourceVirtualBaseAccess {
                    vbptr_offset: 8,
                    vbtable_entry_offset: 0x0c,
                }),
            )],
        };

        let reports = build_access_reports(&raw);
        let report = access(&reports, ProfileAccessTarget::HardwareProtected);
        let AccessResult::Selected(rows) = &report.result else {
            panic!("expected selected method");
        };

        assert_eq!(rows.len(), 3);
        assert_eq!(super::access_field_label(rows[0].field), "vbptr_offset");
        assert_eq!(
            super::access_field_label(rows[1].field),
            "vbtable_entry_offset"
        );
        assert_eq!(
            rows[2].field,
            AccessFieldTarget::VirtualMethod(VirtualMethodTarget::IsHardwareProtected)
        );
    }

    #[test]
    fn virtual_base_failure_expands_both_offsets() {
        let raw = RawAccessExtraction {
            virtual_methods: vec![virtual_method(
                VirtualMethodTarget::IsHardwareProtected,
                ExtractionStatus::Ambiguous,
                None,
                VirtualMethodAbiStatus::Unavailable,
            )],
            field_offsets: vec![field_offset(
                FieldOffsetTarget::HardwareProtected,
                ExtractionStatus::NoDisp,
                None,
                None,
            )],
            virtual_bases: vec![virtual_base(ExtractionStatus::NoDisp, None)],
        };

        let reports = build_access_reports(&raw);
        let report = access(&reports, ProfileAccessTarget::HardwareProtected);
        let AccessResult::AllFailed(rows) = &report.result else {
            panic!("expected all methods");
        };

        assert_eq!(rows.len(), 4);
        assert_eq!(super::access_field_label(rows[1].field), "vbptr_offset");
        assert_eq!(rows[1].status, AccessStatus::NoDisp);
        assert_eq!(
            super::access_field_label(rows[2].field),
            "vbtable_entry_offset"
        );
        assert_eq!(rows[2].status, AccessStatus::NoDisp);
        assert_eq!(rows[3].status, AccessStatus::Ambiguous);
    }
}
