use std::mem::size_of;
use std::path::PathBuf;

use dwm_lut_payload::{
    DwmLutStatusSnapshot, HOOK_STATUS_ABI_VERSION, HookStatusSnapshot, MAX_PROFILE_NAME_BYTES,
};

use super::injector::{is_staged_hook_module, module_export_path};
use super::win32::{
    OwnedHandle, find_remote_modules_by_name, open_status_process, read_process_memory,
    resolve_remote_module_export_address,
};
use crate::inject::{HookStatus, InjectError, InjectionStep};

const MAX_STATUS_QUERY_ATTEMPTS: usize = 5;

pub(crate) fn query_hook_status(pid: u32) -> Result<HookStatusSnapshot, InjectError> {
    let remote_hook_modules = find_remote_modules_by_name(
        pid,
        InjectionStep::ResolveStatusExport,
        is_staged_hook_module,
    )?;
    if remote_hook_modules.is_empty() {
        return Ok(HookStatusSnapshot::Inactive);
    }
    let process = open_status_process(pid)?;

    let mut aggregation = HookStatusAggregation::default();
    for remote_hook_module in remote_hook_modules {
        let module_path = PathBuf::from(module_export_path(
            &remote_hook_module.path,
            &remote_hook_module.name,
        ));
        let remote_status_address = match resolve_remote_module_export_address(
            &process,
            remote_hook_module.module.base_address,
            "dwm_lut_status",
            InjectionStep::ResolveStatusExport,
            &module_path,
        ) {
            Ok(address) => address,
            Err(error) => {
                aggregation.record_failure(module_path, error);
                continue;
            }
        };

        let reader = ProcessStatusReader { process: &process };
        let snapshot = match query_remote_hook_status(&reader, remote_status_address) {
            Ok(snapshot) => snapshot,
            Err(error) => {
                aggregation.record_failure(module_path, error);
                continue;
            }
        };

        aggregation.record_snapshot(snapshot);
    }

    aggregation.finish()
}

trait StatusMemoryReader {
    fn read(&self, address: usize, len: usize) -> Result<Vec<u8>, InjectError>;
}

struct ProcessStatusReader<'a> {
    process: &'a OwnedHandle,
}

impl StatusMemoryReader for ProcessStatusReader<'_> {
    fn read(&self, address: usize, len: usize) -> Result<Vec<u8>, InjectError> {
        read_process_memory(
            self.process,
            address,
            len,
            InjectionStep::ReadStatusSnapshot,
        )
    }
}

fn query_remote_hook_status<R: StatusMemoryReader>(
    reader: &R,
    remote_status_address: usize,
) -> Result<HookStatusSnapshot, InjectError> {
    const SEQUENCE_SIZE: usize = size_of::<u32>();
    const BODY_SIZE: usize = size_of::<DwmLutStatusSnapshot>() - SEQUENCE_SIZE;

    for _ in 0..MAX_STATUS_QUERY_ATTEMPTS {
        let sequence_before = read_status_u32(&reader.read(remote_status_address, SEQUENCE_SIZE)?)?;
        if sequence_before % 2 != 0 {
            continue;
        }

        let body = reader.read(remote_status_address + SEQUENCE_SIZE, BODY_SIZE)?;
        let sequence_after = read_status_u32(&reader.read(remote_status_address, SEQUENCE_SIZE)?)?;
        if sequence_before != sequence_after || sequence_after % 2 != 0 {
            continue;
        }

        return parse_status_snapshot_body(&body);
    }

    Err(InjectError::InvalidHookStatusSnapshot(format!(
        "status snapshot remained unstable after {MAX_STATUS_QUERY_ATTEMPTS} attempts"
    )))
}

fn parse_status_snapshot_body(body: &[u8]) -> Result<HookStatusSnapshot, InjectError> {
    const PROFILE_OFFSET: usize = 16;

    let abi_version = read_status_u32(body)?;
    if abi_version != HOOK_STATUS_ABI_VERSION {
        return Err(InjectError::InvalidHookStatusSnapshot(format!(
            "unsupported ABI version {abi_version}"
        )));
    }
    let struct_size = read_status_u32(&body[4..])?;
    if struct_size as usize != size_of::<DwmLutStatusSnapshot>() {
        return Err(InjectError::InvalidHookStatusSnapshot(format!(
            "unexpected struct size {struct_size}"
        )));
    }
    let status_code = read_status_u32(&body[8..])?;
    let Some(status) = HookStatus::from_code(status_code) else {
        return Err(InjectError::InvalidHookStatusSnapshot(format!(
            "unknown hook status {status_code:#x}"
        )));
    };
    let profile_name_len = read_status_u32(&body[12..])? as usize;
    if profile_name_len > MAX_PROFILE_NAME_BYTES {
        return Err(InjectError::InvalidHookStatusSnapshot(format!(
            "profile name length {profile_name_len} exceeds {MAX_PROFILE_NAME_BYTES}"
        )));
    }
    let profile_storage = body
        .get(PROFILE_OFFSET..PROFILE_OFFSET + MAX_PROFILE_NAME_BYTES)
        .ok_or_else(|| {
            InjectError::InvalidHookStatusSnapshot("status snapshot body was truncated".to_string())
        })?;
    let profile_name = std::str::from_utf8(&profile_storage[..profile_name_len])
        .map_err(|error| {
            InjectError::InvalidHookStatusSnapshot(format!(
                "profile name is not valid UTF-8: {error}"
            ))
        })?
        .to_string();

    match status {
        HookStatus::Active if profile_name.is_empty() => {
            Err(InjectError::InvalidHookStatusSnapshot(
                "active hook did not report a profile name".to_string(),
            ))
        }
        HookStatus::Active => Ok(HookStatusSnapshot::Active { profile_name }),
        HookStatus::Inactive if profile_name.is_empty() => Ok(HookStatusSnapshot::Inactive),
        HookStatus::Transitioning if profile_name.is_empty() => {
            Ok(HookStatusSnapshot::Transitioning)
        }
        HookStatus::Inactive | HookStatus::Transitioning => {
            Err(InjectError::InvalidHookStatusSnapshot(format!(
                "{status:?} hook reported a profile name"
            )))
        }
    }
}

fn read_status_u32(bytes: &[u8]) -> Result<u32, InjectError> {
    let bytes = bytes.get(..size_of::<u32>()).ok_or_else(|| {
        InjectError::InvalidHookStatusSnapshot("status field was truncated".to_string())
    })?;
    Ok(u32::from_le_bytes(
        bytes
            .try_into()
            .expect("slice length was checked to be four"),
    ))
}

#[derive(Debug, Default)]
struct HookStatusAggregation {
    active_profile_name: Option<String>,
    profile_mismatch: Option<(String, String)>,
    transitioning: bool,
    failures: Vec<(PathBuf, InjectError)>,
}

impl HookStatusAggregation {
    fn record_snapshot(&mut self, snapshot: HookStatusSnapshot) {
        match snapshot {
            HookStatusSnapshot::Active { profile_name } => {
                if let Some(current) = &self.active_profile_name
                    && current != &profile_name
                {
                    self.profile_mismatch = Some((current.clone(), profile_name));
                } else {
                    self.active_profile_name = Some(profile_name);
                }
            }
            HookStatusSnapshot::Transitioning => self.transitioning = true,
            HookStatusSnapshot::Inactive => {}
        }
    }

    fn record_failure(&mut self, module_path: PathBuf, error: InjectError) {
        self.failures.push((module_path, error));
    }

    fn finish(self) -> Result<HookStatusSnapshot, InjectError> {
        if let Some((first, second)) = self.profile_mismatch {
            return Err(InjectError::HookStatusProfileMismatch { first, second });
        }
        if let Some(profile_name) = self.active_profile_name {
            return Ok(HookStatusSnapshot::Active { profile_name });
        }
        if !self.failures.is_empty() {
            return Err(InjectError::HookStatusModulesFailed {
                failures: self.failures,
            });
        }
        if self.transitioning {
            Ok(HookStatusSnapshot::Transitioning)
        } else {
            Ok(HookStatusSnapshot::Inactive)
        }
    }
}

#[cfg(test)]
mod tests {
    use std::collections::VecDeque;
    use std::path::PathBuf;
    use std::sync::Mutex;

    use dwm_lut_payload::{
        DwmLutStatusSnapshot, HOOK_STATUS_ABI_VERSION, HookStatusSnapshot, MAX_PROFILE_NAME_BYTES,
    };

    use crate::inject::HookStatus;
    use crate::inject::InjectError;

    use super::{
        HookStatusAggregation, MAX_STATUS_QUERY_ATTEMPTS, StatusMemoryReader,
        query_remote_hook_status,
    };

    const STATUS_ADDRESS: usize = 0x1234_0000;

    struct FakeStatusReader {
        reads: Mutex<VecDeque<Vec<u8>>>,
    }

    impl FakeStatusReader {
        fn new(reads: impl IntoIterator<Item = Vec<u8>>) -> Self {
            Self {
                reads: Mutex::new(reads.into_iter().collect()),
            }
        }
    }

    impl StatusMemoryReader for FakeStatusReader {
        fn read(&self, _address: usize, _len: usize) -> Result<Vec<u8>, InjectError> {
            Ok(self
                .reads
                .lock()
                .unwrap()
                .pop_front()
                .expect("unexpected status memory read"))
        }
    }

    fn sequence(value: u32) -> Vec<u8> {
        value.to_le_bytes().to_vec()
    }

    fn snapshot_body(status: u32, profile_name: &[u8]) -> Vec<u8> {
        let mut body = Vec::with_capacity(std::mem::size_of::<DwmLutStatusSnapshot>() - 4);
        body.extend_from_slice(&HOOK_STATUS_ABI_VERSION.to_le_bytes());
        body.extend_from_slice(&(std::mem::size_of::<DwmLutStatusSnapshot>() as u32).to_le_bytes());
        body.extend_from_slice(&status.to_le_bytes());
        body.extend_from_slice(&(profile_name.len() as u32).to_le_bytes());
        body.extend_from_slice(profile_name);
        body.resize(16 + MAX_PROFILE_NAME_BYTES, 0);
        body
    }

    fn stable_query(body: Vec<u8>) -> Result<HookStatusSnapshot, InjectError> {
        query_remote_hook_status(
            &FakeStatusReader::new([sequence(2), body, sequence(2)]),
            STATUS_ADDRESS,
        )
    }

    fn status_snapshot(status: HookStatus, profile_name: Option<&str>) -> HookStatusSnapshot {
        match status {
            HookStatus::Inactive => HookStatusSnapshot::Inactive,
            HookStatus::Active => HookStatusSnapshot::Active {
                profile_name: profile_name
                    .expect("active snapshot needs a profile")
                    .to_string(),
            },
            HookStatus::Transitioning => HookStatusSnapshot::Transitioning,
        }
    }

    #[test]
    fn direct_status_read_accepts_a_stable_snapshot() {
        assert_eq!(
            stable_query(snapshot_body(HookStatus::Active as u32, b"gaming")).unwrap(),
            HookStatusSnapshot::Active {
                profile_name: "gaming".to_string()
            }
        );
    }

    #[test]
    fn direct_status_read_retries_odd_and_changed_sequences() {
        let body = snapshot_body(HookStatus::Inactive as u32, b"");
        let reader = FakeStatusReader::new([
            sequence(1),
            sequence(2),
            body.clone(),
            sequence(4),
            sequence(6),
            body,
            sequence(6),
        ]);

        assert_eq!(
            query_remote_hook_status(&reader, STATUS_ADDRESS).unwrap(),
            HookStatusSnapshot::Inactive
        );
    }

    #[test]
    fn direct_status_read_rejects_snapshot_after_retry_budget_is_exhausted() {
        let reader = FakeStatusReader::new((0..MAX_STATUS_QUERY_ATTEMPTS).map(|_| sequence(1)));

        assert!(matches!(
            query_remote_hook_status(&reader, STATUS_ADDRESS),
            Err(InjectError::InvalidHookStatusSnapshot(_))
        ));
    }

    #[test]
    fn direct_status_read_rejects_invalid_snapshot_fields() {
        let mut invalid_abi = snapshot_body(HookStatus::Inactive as u32, b"");
        invalid_abi[..4].copy_from_slice(&99_u32.to_le_bytes());
        let mut invalid_size = snapshot_body(HookStatus::Inactive as u32, b"");
        invalid_size[4..8].copy_from_slice(&12_u32.to_le_bytes());
        let invalid_status = snapshot_body(99, b"");
        let mut invalid_length = snapshot_body(HookStatus::Active as u32, b"valid");
        invalid_length[12..16]
            .copy_from_slice(&((MAX_PROFILE_NAME_BYTES + 1) as u32).to_le_bytes());
        let inactive_with_name = snapshot_body(HookStatus::Inactive as u32, b"unexpected");
        let active_without_name = snapshot_body(HookStatus::Active as u32, b"");
        let invalid_utf8 = snapshot_body(HookStatus::Active as u32, &[0xff]);

        for (case, body) in [
            ("unsupported ABI", invalid_abi),
            ("unexpected struct size", invalid_size),
            ("unknown status", invalid_status),
            ("oversized profile name", invalid_length),
            ("inactive status with profile", inactive_with_name),
            ("active status without profile", active_without_name),
            ("invalid profile UTF-8", invalid_utf8),
        ] {
            assert!(
                matches!(
                    stable_query(body),
                    Err(InjectError::InvalidHookStatusSnapshot(_))
                ),
                "case should be rejected: {case}"
            );
        }
    }

    #[test]
    fn direct_status_read_rejects_partial_reads() {
        let reader = FakeStatusReader::new([vec![0; 3]]);
        assert!(matches!(
            query_remote_hook_status(&reader, STATUS_ADDRESS),
            Err(InjectError::InvalidHookStatusSnapshot(_))
        ));

        let reader = FakeStatusReader::new([sequence(2), vec![0; 20], sequence(2)]);
        assert!(matches!(
            query_remote_hook_status(&reader, STATUS_ADDRESS),
            Err(InjectError::InvalidHookStatusSnapshot(_))
        ));
    }

    #[test]
    fn hook_status_aggregation_prefers_active_over_failures() {
        let mut aggregation = HookStatusAggregation::default();
        aggregation.record_failure(
            PathBuf::from("old.dll"),
            InjectError::ExportNotFound {
                export: "dwm_lut_status".to_string(),
                dll_path: PathBuf::from("old.dll"),
            },
        );
        aggregation.record_snapshot(status_snapshot(HookStatus::Active, Some("gaming")));

        assert_eq!(
            aggregation.finish().unwrap(),
            status_snapshot(HookStatus::Active, Some("gaming"))
        );
    }

    #[test]
    fn hook_status_aggregation_reports_failures_without_active_module() {
        let mut aggregation = HookStatusAggregation::default();
        aggregation.record_snapshot(status_snapshot(HookStatus::Inactive, None));
        aggregation.record_failure(
            PathBuf::from("unknown.dll"),
            InjectError::InvalidHookStatusSnapshot("unknown hook status 0x63".to_string()),
        );

        assert!(matches!(
            aggregation.finish(),
            Err(InjectError::HookStatusModulesFailed { .. })
        ));
    }

    #[test]
    fn hook_status_aggregation_prefers_transitioning_over_inactive() {
        let mut aggregation = HookStatusAggregation::default();
        aggregation.record_snapshot(status_snapshot(HookStatus::Inactive, None));
        aggregation.record_snapshot(status_snapshot(HookStatus::Transitioning, None));

        assert_eq!(
            aggregation.finish().unwrap(),
            status_snapshot(HookStatus::Transitioning, None)
        );
    }

    #[test]
    fn hook_status_aggregation_rejects_different_active_profiles() {
        let mut aggregation = HookStatusAggregation::default();
        aggregation.record_snapshot(status_snapshot(HookStatus::Active, Some("gaming")));
        aggregation.record_snapshot(status_snapshot(HookStatus::Active, Some("editing")));

        assert!(matches!(
            aggregation.finish(),
            Err(InjectError::HookStatusProfileMismatch { .. })
        ));
    }
}
