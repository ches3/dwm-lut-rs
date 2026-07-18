use std::collections::{BTreeMap, HashSet};
use std::fs;
use std::os::windows::ffi::OsStrExt;
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};
use windows_sys::Win32::Storage::FileSystem::{
    MOVEFILE_REPLACE_EXISTING, MOVEFILE_WRITE_THROUGH, MoveFileExW,
};

use super::{ColorMode, ConfigError};

fn reject_invalid_profile_name(name: &str, context: &str) -> Result<(), ConfigError> {
    if name.trim().is_empty() {
        return Err(ConfigError::parse_message(format!(
            "{context} must not be empty"
        )));
    }
    Ok(())
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct ConfigDocument {
    pub(crate) default_profile: String,
    pub(crate) profiles: BTreeMap<String, ProfileDocument>,
}

#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct ProfileDocument {
    pub(crate) assignments: Vec<ConfigAssignmentDocument>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct ConfigAssignmentDocument {
    pub(crate) monitor_device_path: String,
    pub(crate) color_mode: ConfigColorMode,
    pub(crate) lut_path: PathBuf,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum ConfigColorMode {
    Sdr,
    Hdr,
}

impl From<ConfigColorMode> for ColorMode {
    fn from(value: ConfigColorMode) -> Self {
        match value {
            ConfigColorMode::Sdr => Self::Sdr,
            ConfigColorMode::Hdr => Self::Hdr,
        }
    }
}

impl Default for ConfigDocument {
    fn default() -> Self {
        Self {
            default_profile: "default".to_string(),
            profiles: BTreeMap::from([("default".to_string(), ProfileDocument::default())]),
        }
    }
}

impl ConfigDocument {
    pub(crate) fn add_profile(&mut self, name: &str) -> Result<String, ConfigError> {
        let name = normalized_profile_name(name)?;
        if self.profile_key(&name).is_some() {
            return Err(ConfigError::parse_message(format!(
                "profile already exists: {name}"
            )));
        }
        self.profiles
            .insert(name.clone(), ProfileDocument::default());
        Ok(name)
    }

    pub(crate) fn rename_profile(
        &mut self,
        old_name: &str,
        new_name: &str,
    ) -> Result<String, ConfigError> {
        let new_name = normalized_profile_name(new_name)?;
        let old_key = self
            .profile_key(old_name)
            .ok_or_else(|| ConfigError::parse_message(format!("profile not found: {old_name}")))?;
        if let Some(existing) = self.profile_key(&new_name)
            && !existing.eq_ignore_ascii_case(&old_key)
        {
            return Err(ConfigError::parse_message(format!(
                "profile already exists: {new_name}"
            )));
        }

        let profile = self
            .profiles
            .remove(&old_key)
            .expect("profile key was found");
        if self.default_profile.eq_ignore_ascii_case(&old_key) {
            self.default_profile = new_name.clone();
        }
        self.profiles.insert(new_name.clone(), profile);
        Ok(new_name)
    }

    pub(crate) fn delete_profile(&mut self, name: &str) -> Result<String, ConfigError> {
        if self.profiles.len() == 1 {
            return Err(ConfigError::parse_message(
                "the last profile cannot be deleted",
            ));
        }
        let key = self
            .profile_key(name)
            .ok_or_else(|| ConfigError::parse_message(format!("profile not found: {name}")))?;
        if self.default_profile.eq_ignore_ascii_case(&key) {
            return Err(ConfigError::parse_message(
                "select another default profile before deleting this profile",
            ));
        }
        self.profiles.remove(&key);
        Ok(self
            .profiles
            .keys()
            .next()
            .expect("more than one profile existed")
            .clone())
    }

    pub(crate) fn set_default_profile(&mut self, name: &str) -> Result<(), ConfigError> {
        let key = self
            .profile_key(name)
            .ok_or_else(|| ConfigError::parse_message(format!("profile not found: {name}")))?;
        self.default_profile = key;
        Ok(())
    }

    pub(crate) fn set_assignment(
        &mut self,
        profile: &str,
        monitor_device_path: &str,
        color_mode: ConfigColorMode,
        lut_path: PathBuf,
    ) -> Result<(), ConfigError> {
        let key = self
            .profile_key(profile)
            .ok_or_else(|| ConfigError::parse_message(format!("profile not found: {profile}")))?;
        let assignments = &mut self
            .profiles
            .get_mut(&key)
            .expect("profile key was found")
            .assignments;
        if let Some(existing) = assignments.iter_mut().find(|assignment| {
            assignment
                .monitor_device_path
                .eq_ignore_ascii_case(monitor_device_path)
                && assignment.color_mode == color_mode
        }) {
            existing.lut_path = lut_path;
        } else {
            assignments.push(ConfigAssignmentDocument {
                monitor_device_path: monitor_device_path.to_string(),
                color_mode,
                lut_path,
            });
        }
        assignments.sort_by(|left, right| {
            left.monitor_device_path
                .to_ascii_uppercase()
                .cmp(&right.monitor_device_path.to_ascii_uppercase())
                .then(left.color_mode.cmp(&right.color_mode))
        });
        Ok(())
    }

    pub(crate) fn clear_assignment(
        &mut self,
        profile: &str,
        monitor_device_path: &str,
        color_mode: ConfigColorMode,
    ) -> Result<(), ConfigError> {
        let key = self
            .profile_key(profile)
            .ok_or_else(|| ConfigError::parse_message(format!("profile not found: {profile}")))?;
        self.profiles
            .get_mut(&key)
            .expect("profile key was found")
            .assignments
            .retain(|assignment| {
                !assignment
                    .monitor_device_path
                    .eq_ignore_ascii_case(monitor_device_path)
                    || assignment.color_mode != color_mode
            });
        Ok(())
    }

    pub(crate) fn profile_key(&self, name: &str) -> Option<String> {
        self.profiles
            .keys()
            .find(|key| key.eq_ignore_ascii_case(name))
            .cloned()
    }
}

fn normalized_profile_name(name: &str) -> Result<String, ConfigError> {
    let name = name.trim();
    if name.is_empty() {
        return Err(ConfigError::parse_message("profile name must not be empty"));
    }
    Ok(name.to_string())
}

pub(crate) fn load_config_document(path: &Path) -> Result<ConfigDocument, ConfigError> {
    match fs::read_to_string(path) {
        Ok(contents) => parse_config_document_str(&contents),
        Err(source) if source.kind() == std::io::ErrorKind::NotFound => {
            Ok(ConfigDocument::default())
        }
        Err(source) => Err(ConfigError::Io(source)),
    }
}

pub(crate) fn save_config_document(
    path: &Path,
    document: &ConfigDocument,
) -> Result<(), ConfigError> {
    validate_config_document(document)?;
    let bytes = serde_json::to_vec_pretty(document)
        .map_err(|error| ConfigError::parse_message(format!("serialize config failed: {error}")))?;
    let base_dir = path.parent().unwrap_or_else(|| Path::new("."));
    fs::create_dir_all(base_dir)?;
    atomic_write(path, &bytes)
}

pub(super) fn parse_config_document_str(contents: &str) -> Result<ConfigDocument, ConfigError> {
    let document: ConfigDocument = serde_json::from_str(contents).map_err(json_parse_error)?;
    normalize_config_document(document)
}

fn normalize_config_document(mut document: ConfigDocument) -> Result<ConfigDocument, ConfigError> {
    reject_invalid_profile_name(&document.default_profile, "default_profile")?;
    document.default_profile = document.default_profile.trim().to_owned();
    if document.profiles.is_empty() {
        return Err(ConfigError::parse_message("profiles must not be empty"));
    }

    let mut profiles = BTreeMap::new();
    let mut canonical_profile_names = HashSet::new();
    for (profile_name, profile) in document.profiles {
        reject_invalid_profile_name(&profile_name, "profile name")?;
        let profile_name = profile_name.trim().to_owned();
        if !canonical_profile_names.insert(profile_name.to_ascii_uppercase()) {
            return Err(ConfigError::parse_message(format!(
                "duplicate profile name: {profile_name}"
            )));
        }
        validate_assignment_duplicates(&profile.assignments)?;
        profiles.insert(profile_name, profile);
    }
    document.profiles = profiles;
    let default_profile = document
        .profile_key(&document.default_profile)
        .ok_or_else(|| {
            ConfigError::parse_message(format!(
                "default_profile={} is not defined in profiles",
                document.default_profile
            ))
        })?;
    document.default_profile = default_profile;
    Ok(document)
}

fn validate_config_document(document: &ConfigDocument) -> Result<(), ConfigError> {
    normalize_config_document(document.clone()).map(|_| ())
}

fn validate_assignment_duplicates(
    assignments: &[ConfigAssignmentDocument],
) -> Result<(), ConfigError> {
    let mut assignment_keys = HashSet::new();
    for assignment in assignments {
        let key = (
            assignment.monitor_device_path.to_ascii_uppercase(),
            assignment.color_mode,
        );
        if !assignment_keys.insert(key) {
            return Err(ConfigError::parse_message(format!(
                "duplicate assignment for monitor_device_path={}, color_mode={:?}",
                assignment.monitor_device_path, assignment.color_mode
            )));
        }
    }
    Ok(())
}

fn json_parse_error(error: serde_json::Error) -> ConfigError {
    ConfigError::Parse {
        line: match error.line() {
            0 => None,
            line => Some(line),
        },
        message: error.to_string(),
    }
}

fn atomic_write(path: &Path, bytes: &[u8]) -> Result<(), ConfigError> {
    let unique = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos();
    let temp_path = path.with_extension(format!("tmp-{}-{unique}", std::process::id()));
    let result = (|| {
        let mut file = fs::File::create(&temp_path)?;
        std::io::Write::write_all(&mut file, bytes)?;
        file.sync_all()?;
        let temp = wide_null(&temp_path);
        let target = wide_null(path);
        let ok = unsafe {
            MoveFileExW(
                temp.as_ptr(),
                target.as_ptr(),
                MOVEFILE_REPLACE_EXISTING | MOVEFILE_WRITE_THROUGH,
            )
        };
        if ok == 0 {
            return Err(ConfigError::Io(std::io::Error::last_os_error()));
        }
        Ok(())
    })();
    if result.is_err() {
        let _ = fs::remove_file(&temp_path);
    }
    result
}

fn wide_null(path: &Path) -> Vec<u16> {
    path.as_os_str().encode_wide().chain(Some(0)).collect()
}
#[cfg(test)]
mod document_tests {
    use super::*;

    #[test]
    fn document_profile_edits_preserve_invariants() {
        let mut document = ConfigDocument::default();
        let gaming = document.add_profile(" gaming ").unwrap();
        document.set_default_profile(&gaming).unwrap();
        let desktop = document.rename_profile("gaming", " desktop ").unwrap();

        assert_eq!(desktop, "desktop");
        assert_eq!(document.default_profile, "desktop");
        assert!(document.delete_profile("desktop").is_err());
    }

    #[test]
    fn document_roundtrip_preserves_relative_lut_path() {
        let mut document = ConfigDocument::default();
        document
            .set_assignment(
                "default",
                r"\\?\DISPLAY#ONE",
                ConfigColorMode::Sdr,
                PathBuf::from("luts/desktop.cube"),
            )
            .unwrap();
        let json = serde_json::to_string(&document).unwrap();
        let parsed = parse_config_document_str(&json).unwrap();

        assert_eq!(
            parsed.profiles["default"].assignments[0].lut_path,
            PathBuf::from("luts/desktop.cube")
        );
    }
}
