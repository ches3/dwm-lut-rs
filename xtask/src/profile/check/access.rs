use dwm_lut_profile::{
    AccessFieldTarget, DirectFieldWidth, FieldOffsetExtractSpec, ProfileAccess, ProfileAccessField,
    ProfileAccessTarget,
};

use super::super::analysis::access::RawAccessExtraction;
use super::super::analysis::model::{ExtractionStatus, VirtualMethodAbiStatus};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AccessCheckReport {
    pub target: ProfileAccessTarget,
    pub rows: Vec<AccessCheckRow>,
}

impl AccessCheckReport {
    pub fn failed_count(&self) -> usize {
        self.rows.iter().filter(|row| !row.status.is_ok()).count()
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AccessCheckRow {
    pub field: AccessFieldTarget,
    pub status: AccessCheckStatus,
    pub extracted: Option<String>,
    pub profile: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AccessCheckStatus {
    Missing,
    Extraction(ExtractionStatus),
    Comparison {
        value_matches: bool,
        abi_status: Option<VirtualMethodAbiStatus>,
    },
}

impl AccessCheckStatus {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Missing => "missing",
            Self::Extraction(status) => status.as_str(),
            Self::Comparison {
                value_matches: true,
                abi_status: None | Some(VirtualMethodAbiStatus::Ok),
            } => "ok",
            Self::Comparison {
                value_matches: false,
                abi_status: None | Some(VirtualMethodAbiStatus::Ok),
            } => "mismatch",
            Self::Comparison {
                value_matches: true,
                abi_status: Some(VirtualMethodAbiStatus::Mismatch),
            } => "abi_mismatch",
            Self::Comparison {
                value_matches: false,
                abi_status: Some(VirtualMethodAbiStatus::Mismatch),
            } => "mismatch+abi_mismatch",
            Self::Comparison {
                value_matches: true,
                abi_status: Some(VirtualMethodAbiStatus::Unsupported),
            } => "abi_unsupported",
            Self::Comparison {
                value_matches: false,
                abi_status: Some(VirtualMethodAbiStatus::Unsupported),
            } => "mismatch+abi_unsupported",
            Self::Comparison {
                value_matches: true,
                abi_status: Some(VirtualMethodAbiStatus::Unavailable),
            } => "abi_unavailable",
            Self::Comparison {
                value_matches: false,
                abi_status: Some(VirtualMethodAbiStatus::Unavailable),
            } => "mismatch+abi_unavailable",
        }
    }

    pub const fn is_ok(self) -> bool {
        matches!(
            self,
            Self::Comparison {
                value_matches: true,
                abi_status: None | Some(VirtualMethodAbiStatus::Ok),
            }
        )
    }
}

pub fn build_access_check_reports(
    accesses: Vec<ProfileAccess>,
    raw: &RawAccessExtraction,
) -> Vec<AccessCheckReport> {
    accesses
        .into_iter()
        .map(|access| AccessCheckReport {
            target: access.target,
            rows: access
                .fields
                .into_iter()
                .map(|field| check_field(field, raw))
                .collect(),
        })
        .collect()
}

fn check_field(field: ProfileAccessField, raw: &RawAccessExtraction) -> AccessCheckRow {
    match field {
        ProfileAccessField::VirtualMethodIndex { target, index } => {
            let Some(extracted) = raw.virtual_methods.iter().find(|row| row.target == target)
            else {
                return missing_row(field);
            };
            let extracted_value = extracted.value.map(|value| value.to_string());
            let status = if extracted.status == ExtractionStatus::Ok {
                AccessCheckStatus::Comparison {
                    value_matches: extracted.value == Some(index),
                    abi_status: Some(extracted.abi_status),
                }
            } else {
                AccessCheckStatus::Extraction(extracted.status)
            };
            checked_row(field, status, extracted_value)
        }
        ProfileAccessField::FieldOffset { target, offset } => {
            let Some(extracted) = raw.field_offsets.iter().find(|row| row.target == target) else {
                return missing_row(field);
            };
            let extracted_value = extracted.value.map(|value| format!("{value:#x}"));
            let status = if extracted.status == ExtractionStatus::Ok {
                let offset_matches =
                    extracted.value.and_then(|value| i32::try_from(value).ok()) == Some(offset);
                let width_matches = match target.extract_spec() {
                    FieldOffsetExtractSpec::GetterDisp { width, .. } => {
                        extracted.load_size == Some(direct_field_width_bytes(width))
                    }
                    FieldOffsetExtractSpec::CtorFirstParamStore { .. } => true,
                };
                AccessCheckStatus::Comparison {
                    value_matches: offset_matches && width_matches,
                    abi_status: None,
                }
            } else {
                AccessCheckStatus::Extraction(extracted.status)
            };
            checked_row(field, status, extracted_value)
        }
        ProfileAccessField::VbptrOffset { target, offset } => {
            let Some(extracted) = raw.virtual_bases.iter().find(|row| row.target == target) else {
                return missing_row(field);
            };
            let extracted_value = extracted
                .value
                .map(|value| format!("{:#x}", value.vbptr_offset));
            let status = if extracted.status == ExtractionStatus::Ok {
                AccessCheckStatus::Comparison {
                    value_matches: extracted.value.map(|value| value.vbptr_offset) == Some(offset),
                    abi_status: None,
                }
            } else {
                AccessCheckStatus::Extraction(extracted.status)
            };
            checked_row(field, status, extracted_value)
        }
        ProfileAccessField::VbtableEntryOffset { target, offset } => {
            let Some(extracted) = raw.virtual_bases.iter().find(|row| row.target == target) else {
                return missing_row(field);
            };
            let extracted_value = extracted
                .value
                .map(|value| format!("{:#x}", value.vbtable_entry_offset));
            let status = if extracted.status == ExtractionStatus::Ok {
                AccessCheckStatus::Comparison {
                    value_matches: extracted.value.map(|value| value.vbtable_entry_offset)
                        == Some(offset),
                    abi_status: None,
                }
            } else {
                AccessCheckStatus::Extraction(extracted.status)
            };
            checked_row(field, status, extracted_value)
        }
    }
}

fn checked_row(
    field: ProfileAccessField,
    status: AccessCheckStatus,
    extracted: Option<String>,
) -> AccessCheckRow {
    AccessCheckRow {
        field: field.target(),
        status,
        extracted,
        profile: format_profile_value(field),
    }
}

fn missing_row(field: ProfileAccessField) -> AccessCheckRow {
    checked_row(field, AccessCheckStatus::Missing, None)
}

fn format_profile_value(field: ProfileAccessField) -> String {
    match field {
        ProfileAccessField::VirtualMethodIndex { index, .. } => index.to_string(),
        ProfileAccessField::FieldOffset { offset, .. }
        | ProfileAccessField::VbptrOffset { offset, .. } => format!("{offset:#x}"),
        ProfileAccessField::VbtableEntryOffset { offset, .. } => format!("{offset:#x}"),
    }
}

const fn direct_field_width_bytes(width: DirectFieldWidth) -> usize {
    match width {
        DirectFieldWidth::U8 => 1,
    }
}

#[cfg(test)]
mod tests {
    use dwm_lut_profile::{
        DeviceResourceVirtualBaseAccess, FieldOffsetTarget, ProfileAccess, ProfileAccessField,
        ProfileAccessTarget, VirtualBaseTarget, VirtualMethodTarget,
    };

    use super::{AccessCheckStatus, RawAccessExtraction, build_access_check_reports};
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

    fn d3d11_access(fields: Vec<ProfileAccessField>) -> Vec<ProfileAccess> {
        vec![ProfileAccess {
            target: ProfileAccessTarget::D3D11Resource,
            fields,
        }]
    }

    fn method_index(target: VirtualMethodTarget, index: usize) -> ProfileAccessField {
        ProfileAccessField::VirtualMethodIndex { target, index }
    }

    #[test]
    fn checks_only_fields_selected_by_profile() {
        let accesses = d3d11_access(vec![
            method_index(VirtualMethodTarget::GetPhysicalBackBuffer, 24),
            method_index(VirtualMethodTarget::GetD3D11Resource, 19),
        ]);
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
            ],
            field_offsets: vec![],
            virtual_bases: vec![],
        };

        let reports = build_access_check_reports(accesses, &raw);

        assert_eq!(reports[0].rows.len(), 2);
        assert!(reports[0].rows.iter().all(|row| row.status.is_ok()));
    }

    #[test]
    fn combines_value_and_abi_mismatches() {
        let accesses = d3d11_access(vec![method_index(
            VirtualMethodTarget::GetD3D11Resource,
            19,
        )]);
        let raw = RawAccessExtraction {
            virtual_methods: vec![virtual_method(
                VirtualMethodTarget::GetD3D11Resource,
                ExtractionStatus::Ok,
                Some(20),
                VirtualMethodAbiStatus::Mismatch,
            )],
            field_offsets: vec![],
            virtual_bases: vec![],
        };

        let reports = build_access_check_reports(accesses, &raw);
        let row = &reports[0].rows[0];

        assert_eq!(
            row.status,
            AccessCheckStatus::Comparison {
                value_matches: false,
                abi_status: Some(VirtualMethodAbiStatus::Mismatch),
            }
        );
        assert_eq!(row.status.as_str(), "mismatch+abi_mismatch");
        assert_eq!(row.extracted.as_deref(), Some("20"));
        assert_eq!(row.profile, "19");
    }

    #[test]
    fn keeps_extraction_failure_without_derived_abi_status() {
        let accesses = d3d11_access(vec![method_index(
            VirtualMethodTarget::GetPhysicalBackBuffer,
            24,
        )]);
        let raw = RawAccessExtraction {
            virtual_methods: vec![virtual_method(
                VirtualMethodTarget::GetPhysicalBackBuffer,
                ExtractionStatus::NoVftable,
                None,
                VirtualMethodAbiStatus::Unavailable,
            )],
            field_offsets: vec![],
            virtual_bases: vec![],
        };

        let reports = build_access_check_reports(accesses, &raw);

        assert_eq!(reports[0].rows[0].status.as_str(), "no_vftable");
    }

    #[test]
    fn compares_virtual_base_components_independently() {
        let target = VirtualBaseTarget::DeviceResourceFromOverlaySwapChain;
        let accesses = vec![ProfileAccess {
            target: ProfileAccessTarget::HardwareProtected,
            fields: vec![
                ProfileAccessField::VbptrOffset { target, offset: 8 },
                ProfileAccessField::VbtableEntryOffset {
                    target,
                    offset: 0x0c,
                },
            ],
        }];
        let raw = RawAccessExtraction {
            virtual_methods: vec![],
            field_offsets: vec![FieldOffsetExtractionRow {
                target: FieldOffsetTarget::HardwareProtected,
                status: ExtractionStatus::NoDisp,
                value: None,
                load_size: None,
            }],
            virtual_bases: vec![VirtualBaseExtractionRow {
                target,
                status: ExtractionStatus::Ok,
                value: Some(DeviceResourceVirtualBaseAccess {
                    vbptr_offset: 8,
                    vbtable_entry_offset: 8,
                }),
            }],
        };

        let reports = build_access_check_reports(accesses, &raw);

        assert_eq!(reports[0].rows[0].status.as_str(), "ok");
        assert_eq!(reports[0].rows[1].status.as_str(), "mismatch");
        assert_eq!(reports[0].failed_count(), 1);
    }

    #[test]
    fn field_offset_requires_matching_load_size() {
        let accesses = vec![ProfileAccess {
            target: ProfileAccessTarget::HardwareProtected,
            fields: vec![ProfileAccessField::FieldOffset {
                target: FieldOffsetTarget::HardwareProtected,
                offset: 0x64,
            }],
        }];
        let raw = RawAccessExtraction {
            virtual_methods: vec![],
            field_offsets: vec![FieldOffsetExtractionRow {
                target: FieldOffsetTarget::HardwareProtected,
                status: ExtractionStatus::Ok,
                value: Some(0x64),
                load_size: Some(4),
            }],
            virtual_bases: vec![],
        };

        let reports = build_access_check_reports(accesses, &raw);
        let row = &reports[0].rows[0];

        assert_eq!(row.status.as_str(), "mismatch");
        assert_eq!(row.extracted.as_deref(), Some("0x64"));
        assert_eq!(row.profile, "0x64");
    }
}
