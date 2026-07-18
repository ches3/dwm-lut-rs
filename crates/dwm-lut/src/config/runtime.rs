use std::collections::{HashMap, HashSet};
use std::fs;
use std::path::{Path, PathBuf};

use dwm_lut_payload::{HookPayload, PayloadAssignment, validate_payload};

use crate::backend::monitor::resolve_monitor_identity;
use crate::lut::parse_lut;

use super::document::{ConfigAssignmentDocument, parse_config_document_str};
use super::{ColorMode, ConfigError, MonitorIdentity, MonitorTarget};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LutAssignment {
    pub target: MonitorTarget,
    pub lut_path: PathBuf,
}

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct LutConfig {
    pub assignments: Vec<LutAssignment>,
}

impl LutConfig {
    pub fn empty() -> Self {
        Self::default()
    }

    pub fn add(&mut self, assignment: LutAssignment) {
        self.assignments.push(assignment);
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FileAssignment {
    pub monitor_device_path: String,
    pub color_mode: ColorMode,
    pub lut_path: PathBuf,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FileProfileConfig {
    pub assignments: Vec<FileAssignment>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FileConfig {
    pub default_profile: String,
    pub profiles: HashMap<String, FileProfileConfig>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LoadedConfig {
    pub profile_name: String,
    pub config: LutConfig,
}

#[derive(Debug, Clone, PartialEq)]
pub struct LoadedPayload {
    pub profile_name: String,
    pub payload: HookPayload,
}

fn profile_map_key<'a, T>(profiles: &'a HashMap<String, T>, name: &str) -> Option<&'a str> {
    profiles
        .keys()
        .find(|key| key.eq_ignore_ascii_case(name))
        .map(String::as_str)
}

impl FileConfig {
    pub fn resolve_profile<'a>(
        &'a self,
        profile: Option<&str>,
    ) -> Result<(&'a str, &'a [FileAssignment]), ConfigError> {
        let lookup_name = match profile {
            Some(name) => name.trim(),
            None => self.default_profile.trim(),
        };

        let key = profile_map_key(&self.profiles, lookup_name).ok_or_else(|| {
            if profile.is_some() {
                ConfigError::parse_message(format!("profile not found: {lookup_name}"))
            } else {
                ConfigError::parse_message(format!(
                    "default_profile={} is not defined in profiles",
                    self.default_profile
                ))
            }
        })?;

        let profile = self.profiles.get(key).ok_or_else(|| {
            ConfigError::parse_message(format!(
                "internal error: profile key {key:?} missing after lookup"
            ))
        })?;
        Ok((key, profile.assignments.as_slice()))
    }
}

pub fn load_config(path: &Path, profile: Option<&str>) -> Result<LoadedConfig, ConfigError> {
    let contents = fs::read_to_string(path)?;
    let base_dir = path.parent().unwrap_or_else(|| Path::new("."));
    let file_config = parse_config_str(base_dir, &contents)?;
    let (profile_name, config) =
        resolve_file_config(&file_config, profile, resolve_monitor_identity)?;
    Ok(LoadedConfig {
        profile_name,
        config,
    })
}

pub fn load_payload(path: &Path, profile: Option<&str>) -> Result<LoadedPayload, ConfigError> {
    let loaded = load_config(path, profile)?;
    let payload = config_to_payload(&loaded.config)?;
    Ok(LoadedPayload {
        profile_name: loaded.profile_name,
        payload,
    })
}

pub fn config_to_payload(config: &LutConfig) -> Result<HookPayload, ConfigError> {
    let mut assignments = Vec::with_capacity(config.assignments.len());
    for assignment in &config.assignments {
        assignments.push(PayloadAssignment {
            target: assignment.target,
            lut: parse_lut(&assignment.lut_path)?,
        });
    }

    let payload = HookPayload { assignments };
    validate_payload(&payload).map_err(ConfigError::InvalidPayload)?;
    Ok(payload)
}

pub fn parse_config_str(base_dir: &Path, contents: &str) -> Result<FileConfig, ConfigError> {
    let document = parse_config_document_str(contents)?;
    let mut profiles = HashMap::with_capacity(document.profiles.len());
    for (profile_name, profile_document) in document.profiles {
        profiles.insert(
            profile_name,
            FileProfileConfig {
                assignments: parse_profile_assignments(base_dir, profile_document.assignments)?,
            },
        );
    }

    Ok(FileConfig {
        default_profile: document.default_profile,
        profiles,
    })
}

fn parse_profile_assignments(
    base_dir: &Path,
    assignments: Vec<ConfigAssignmentDocument>,
) -> Result<Vec<FileAssignment>, ConfigError> {
    let mut parsed = Vec::with_capacity(assignments.len());

    for assignment in assignments {
        let lut_path = if assignment.lut_path.is_absolute() {
            assignment.lut_path
        } else {
            base_dir.join(assignment.lut_path)
        };

        parsed.push(FileAssignment {
            monitor_device_path: assignment.monitor_device_path,
            color_mode: assignment.color_mode.into(),
            lut_path,
        });
    }

    Ok(parsed)
}

pub fn resolve_file_config(
    file_config: &FileConfig,
    profile: Option<&str>,
    mut resolve: impl FnMut(&str) -> Result<MonitorIdentity, ConfigError>,
) -> Result<(String, LutConfig), ConfigError> {
    let (profile_name, assignments) = file_config.resolve_profile(profile)?;

    let mut config = LutConfig::empty();
    let mut identity_keys = HashSet::new();

    for assignment in assignments {
        let identity = resolve(&assignment.monitor_device_path)?;

        let target = MonitorTarget {
            identity,
            color_mode: assignment.color_mode,
        };

        let identity_key = (identity, assignment.color_mode);
        if !identity_keys.insert(identity_key) {
            return Err(ConfigError::parse_message(format!(
                "duplicate assignment for monitor adapter_luid={}, target_id={}, color_mode={:?}",
                identity.adapter_luid, identity.target_id, assignment.color_mode
            )));
        }

        config.add(LutAssignment {
            target,
            lut_path: assignment.lut_path.clone(),
        });
    }

    Ok((profile_name.to_string(), config))
}

#[cfg(test)]
mod tests {
    use std::collections::HashMap;
    use std::path::{Path, PathBuf};

    use super::{
        ColorMode, ConfigError, FileConfig, MonitorIdentity, load_config, parse_config_str,
        resolve_file_config,
    };
    use dwm_lut_payload::AdapterLuid;

    fn test_monitor_device_path() -> &'static str {
        r"\\?\DISPLAY#TEST#5&2b0371&0&UID4357#{e6f07b5f-ee97-4a90-b076-33f57bf4eaa7}"
    }

    fn alternate_monitor_device_path() -> &'static str {
        r"\\?\DISPLAY#TEST#5&2b0371&0&UID4358#{e6f07b5f-ee97-4a90-b076-33f57bf4eaa7}"
    }

    fn test_monitor_device_path_json() -> &'static str {
        r"\\\\?\\DISPLAY#TEST#5&2b0371&0&UID4357#{e6f07b5f-ee97-4a90-b076-33f57bf4eaa7}"
    }

    fn alternate_monitor_device_path_json() -> &'static str {
        r"\\\\?\\DISPLAY#TEST#5&2b0371&0&UID4358#{e6f07b5f-ee97-4a90-b076-33f57bf4eaa7}"
    }

    fn test_monitor_identity() -> MonitorIdentity {
        MonitorIdentity {
            adapter_luid: AdapterLuid {
                high_part: 0,
                low_part: 0x14e02,
            },
            target_id: 4357,
        }
    }

    fn alternate_monitor_identity() -> MonitorIdentity {
        MonitorIdentity {
            adapter_luid: AdapterLuid {
                high_part: 0,
                low_part: 0x14e03,
            },
            target_id: 4358,
        }
    }

    fn resolve_test_monitor(path: &str) -> Result<MonitorIdentity, ConfigError> {
        if path.eq_ignore_ascii_case(test_monitor_device_path()) {
            Ok(test_monitor_identity())
        } else if path.eq_ignore_ascii_case(alternate_monitor_device_path()) {
            Ok(alternate_monitor_identity())
        } else {
            Err(ConfigError::parse_message(format!(
                "monitor_device_path not found: {path}"
            )))
        }
    }

    fn profile_config(default_profile: &str, assignments_json: &str) -> String {
        format!(
            r#"{{
  "default_profile": "{default_profile}",
  "profiles": {{
    "{default_profile}": {{
      "assignments": {assignments_json}
    }}
  }}
}}"#
        )
    }

    fn multi_profile_config(default_profile: &str, profiles_json: &str) -> String {
        format!(
            r#"{{
  "default_profile": "{default_profile}",
  "profiles": {profiles_json}
}}"#
        )
    }

    #[test]
    fn parse_config_resolves_relative_lut_paths() {
        let file_config = parse_config_str(
            Path::new(r"C:\work\profiles"),
            &profile_config(
                "work",
                &format!(
                    r#"[{{
      "monitor_device_path": "{}",
      "color_mode": "sdr",
      "lut_path": "panel.cube"
    }}]"#,
                    test_monitor_device_path_json()
                ),
            ),
        )
        .expect("config should parse");

        assert_eq!(file_config.default_profile, "work");
        let profile = file_config
            .resolve_profile(Some("work"))
            .expect("work profile should exist")
            .1;
        assert_eq!(profile.len(), 1);
        assert_eq!(profile[0].monitor_device_path, test_monitor_device_path());
        assert_eq!(profile[0].color_mode, ColorMode::Sdr);
        assert_eq!(
            profile[0].lut_path,
            PathBuf::from(r"C:\work\profiles").join("panel.cube")
        );

        let (_, config) = resolve_file_config(&file_config, None, resolve_test_monitor)
            .expect("config should resolve");
        assert_eq!(
            config.assignments[0].target.identity,
            test_monitor_identity()
        );
    }

    #[test]
    fn parse_config_rejects_duplicate_monitor_device_path_for_same_color_mode() {
        let error = parse_config_str(
            Path::new(r"C:\work\profiles"),
            &profile_config(
                "work",
                &format!(
                    r#"[{{
      "monitor_device_path": "{}",
      "color_mode": "sdr",
      "lut_path": "panel-a.cube"
    }}, {{
      "monitor_device_path": "{}",
      "color_mode": "sdr",
      "lut_path": "panel-b.cube"
    }}]"#,
                    test_monitor_device_path_json(),
                    test_monitor_device_path_json().to_ascii_uppercase()
                ),
            ),
        )
        .expect_err("duplicate monitor path should fail");

        match error {
            ConfigError::Parse {
                line: None,
                message,
            } => assert!(message.contains("duplicate assignment for monitor_device_path")),
            other => panic!("unexpected error: {other}"),
        }
    }

    #[test]
    fn resolve_file_config_rejects_duplicate_runtime_monitor_identity_for_same_color_mode() {
        let file_config = parse_config_str(
            Path::new(r"C:\work\profiles"),
            &profile_config(
                "work",
                &format!(
                    r#"[{{
      "monitor_device_path": "{}",
      "color_mode": "sdr",
      "lut_path": "panel-a.cube"
    }}, {{
      "monitor_device_path": "{}",
      "color_mode": "sdr",
      "lut_path": "panel-b.cube"
    }}]"#,
                    test_monitor_device_path_json(),
                    alternate_monitor_device_path_json()
                ),
            ),
        )
        .expect("distinct monitor paths should parse");

        let error = resolve_file_config(&file_config, None, |_| Ok(test_monitor_identity()))
            .expect_err("duplicate runtime monitor identity should fail");

        match error {
            ConfigError::Parse {
                line: None,
                message,
            } => assert!(message.contains("duplicate assignment for monitor")),
            other => panic!("unexpected error: {other}"),
        }
    }

    #[test]
    fn resolve_file_config_accepts_same_monitor_device_path_for_sdr_and_hdr() {
        let file_config = parse_config_str(
            Path::new(r"C:\work\profiles"),
            &profile_config(
                "work",
                &format!(
                    r#"[{{
      "monitor_device_path": "{}",
      "color_mode": "sdr",
      "lut_path": "panel-sdr.cube"
    }}, {{
      "monitor_device_path": "{}",
      "color_mode": "hdr",
      "lut_path": "panel-hdr.cube"
    }}]"#,
                    test_monitor_device_path_json(),
                    test_monitor_device_path_json()
                ),
            ),
        )
        .expect("SDR and HDR assignments should coexist for one monitor path");

        let (_, config) = resolve_file_config(&file_config, None, resolve_test_monitor)
            .expect("config should resolve");

        assert_eq!(config.assignments.len(), 2);
        assert_eq!(
            config.assignments[0].target.identity,
            config.assignments[1].target.identity
        );
        assert_ne!(
            config.assignments[0].target.color_mode,
            config.assignments[1].target.color_mode
        );
    }

    #[test]
    fn parse_config_requires_monitor_device_path() {
        let error = parse_config_str(
            Path::new(r"C:\work\profiles"),
            &profile_config(
                "work",
                r#"[{
      "color_mode": "sdr",
      "lut_path": "panel.cube"
    }]"#,
            ),
        )
        .expect_err("missing monitor_device_path should fail");

        match error {
            ConfigError::Parse {
                line: Some(_),
                message,
            } => assert!(message.contains("missing field `monitor_device_path`")),
            other => panic!("unexpected error: {other}"),
        }
    }

    #[test]
    fn parse_config_rejects_unknown_assignment_fields() {
        let error = parse_config_str(
            Path::new(r"C:\work\profiles"),
            &profile_config(
                "work",
                &format!(
                    r#"[{{
      "monitor_device_path": "{}",
      "desktop_left": 0,
      "color_mode": "sdr",
      "lut_path": "panel.cube"
    }}]"#,
                    test_monitor_device_path_json()
                ),
            ),
        )
        .expect_err("unknown assignment field should fail");

        match error {
            ConfigError::Parse {
                line: Some(_),
                message,
            } => assert!(message.contains("unknown field")),
            other => panic!("unexpected error: {other}"),
        }
    }

    #[test]
    fn parse_config_requires_default_profile_and_profiles_fields() {
        let error = parse_config_str(Path::new(r"C:\work\profiles"), "{}")
            .expect_err("missing root fields should fail");

        match error {
            ConfigError::Parse {
                line: Some(1),
                message,
            } => assert!(message.contains("missing field `default_profile`")),
            other => panic!("unexpected error: {other}"),
        }
    }

    #[test]
    fn parse_config_accepts_empty_assignments_array() {
        let config = parse_config_str(
            Path::new(r"C:\work\profiles"),
            &profile_config("work", "[]"),
        )
        .expect("empty assignments array should still parse");

        assert_eq!(
            config
                .resolve_profile(Some("work"))
                .expect("work profile should exist")
                .1
                .len(),
            0
        );
    }

    #[test]
    fn parse_config_rejects_empty_default_profile() {
        let error = parse_config_str(
            Path::new(r"C:\work\profiles"),
            r#"{ "default_profile": "", "profiles": { "work": { "assignments": [] } } }"#,
        )
        .expect_err("empty default_profile should fail");

        match error {
            ConfigError::Parse {
                line: None,
                message,
            } => assert!(message.contains("default_profile must not be empty")),
            other => panic!("unexpected error: {other}"),
        }
    }

    #[test]
    fn parse_config_rejects_empty_profiles_object() {
        let error = parse_config_str(
            Path::new(r"C:\work\profiles"),
            r#"{ "default_profile": "work", "profiles": {} }"#,
        )
        .expect_err("empty profiles should fail");

        match error {
            ConfigError::Parse {
                line: None,
                message,
            } => assert!(message.contains("profiles must not be empty")),
            other => panic!("unexpected error: {other}"),
        }
    }

    #[test]
    fn parse_config_rejects_default_profile_missing_from_profiles() {
        let error = parse_config_str(
            Path::new(r"C:\work\profiles"),
            r#"{ "default_profile": "work", "profiles": { "gaming": { "assignments": [] } } }"#,
        )
        .expect_err("missing default profile entry should fail");

        match error {
            ConfigError::Parse {
                line: None,
                message,
            } => assert!(message.contains("default_profile=work is not defined in profiles")),
            other => panic!("unexpected error: {other}"),
        }
    }

    #[test]
    fn resolve_file_config_uses_default_profile_when_profile_is_not_specified() {
        let file_config = parse_config_str(
            Path::new(r"C:\work\profiles"),
            &multi_profile_config(
                "work",
                &format!(
                    r#"{{
    "work": {{
      "assignments": [{{
        "monitor_device_path": "{}",
        "color_mode": "sdr",
        "lut_path": "work.cube"
      }}]
    }},
    "gaming": {{
      "assignments": [{{
        "monitor_device_path": "{}",
        "color_mode": "hdr",
        "lut_path": "gaming.cube"
      }}]
    }}
  }}"#,
                    test_monitor_device_path_json(),
                    test_monitor_device_path_json()
                ),
            ),
        )
        .expect("config should parse");

        let (profile_name, config) = resolve_file_config(&file_config, None, resolve_test_monitor)
            .expect("default profile should resolve");
        assert_eq!(profile_name, "work");
        assert_eq!(config.assignments.len(), 1);
        assert_eq!(config.assignments[0].target.color_mode, ColorMode::Sdr);
        assert_eq!(
            config.assignments[0].lut_path,
            PathBuf::from(r"C:\work\profiles").join("work.cube")
        );
    }

    #[test]
    fn resolve_file_config_selects_named_profile() {
        let file_config = parse_config_str(
            Path::new(r"C:\work\profiles"),
            &multi_profile_config(
                "work",
                &format!(
                    r#"{{
    "work": {{
      "assignments": [{{
        "monitor_device_path": "{}",
        "color_mode": "sdr",
        "lut_path": "work.cube"
      }}]
    }},
    "gaming": {{
      "assignments": [{{
        "monitor_device_path": "{}",
        "color_mode": "hdr",
        "lut_path": "gaming.cube"
      }}]
    }}
  }}"#,
                    test_monitor_device_path_json(),
                    test_monitor_device_path_json()
                ),
            ),
        )
        .expect("config should parse");

        let (profile_name, config) =
            resolve_file_config(&file_config, Some("gaming"), resolve_test_monitor)
                .expect("named profile should resolve");
        assert_eq!(profile_name, "gaming");
        assert_eq!(config.assignments.len(), 1);
        assert_eq!(config.assignments[0].target.color_mode, ColorMode::Hdr);
        assert_eq!(
            config.assignments[0].lut_path,
            PathBuf::from(r"C:\work\profiles").join("gaming.cube")
        );
    }

    #[test]
    fn resolve_file_config_selects_named_profile_case_insensitively() {
        let file_config = parse_config_str(
            Path::new(r"C:\work\profiles"),
            &multi_profile_config(
                "work",
                &format!(
                    r#"{{
    "work": {{
      "assignments": [{{
        "monitor_device_path": "{}",
        "color_mode": "sdr",
        "lut_path": "work.cube"
      }}]
    }},
    "gaming": {{
      "assignments": [{{
        "monitor_device_path": "{}",
        "color_mode": "hdr",
        "lut_path": "gaming.cube"
      }}]
    }}
  }}"#,
                    test_monitor_device_path_json(),
                    test_monitor_device_path_json()
                ),
            ),
        )
        .expect("config should parse");

        let (profile_name, config) =
            resolve_file_config(&file_config, Some("GAMING"), resolve_test_monitor)
                .expect("named profile should resolve case-insensitively");
        assert_eq!(profile_name, "gaming");
        assert_eq!(config.assignments.len(), 1);
        assert_eq!(config.assignments[0].target.color_mode, ColorMode::Hdr);
    }

    #[test]
    fn parse_config_rejects_duplicate_profile_names_differing_only_by_case() {
        let error = parse_config_str(
            Path::new(r"C:\work\profiles"),
            r#"{ "default_profile": "work", "profiles": { "work": { "assignments": [] }, "WORK": { "assignments": [] } } }"#,
        )
        .expect_err("case-insensitive duplicate profile names should fail");

        match error {
            ConfigError::Parse {
                line: None,
                message,
            } => assert!(
                message.contains("duplicate profile name:"),
                "unexpected message: {message}"
            ),
            other => panic!("unexpected error: {other}"),
        }
    }

    #[test]
    fn resolve_file_config_matches_default_profile_case_insensitively() {
        let file_config = parse_config_str(
            Path::new(r"C:\work\profiles"),
            &multi_profile_config(
                "Work",
                &format!(
                    r#"{{
    "work": {{
      "assignments": [{{
        "monitor_device_path": "{}",
        "color_mode": "sdr",
        "lut_path": "work.cube"
      }}]
    }}
  }}"#,
                    test_monitor_device_path_json()
                ),
            ),
        )
        .expect("config should parse");

        let (profile_name, config) = resolve_file_config(&file_config, None, resolve_test_monitor)
            .expect("default profile should resolve case-insensitively");
        assert_eq!(profile_name, "work");
        assert_eq!(config.assignments.len(), 1);
    }

    #[test]
    fn resolve_file_config_rejects_unknown_profile_name() {
        let file_config = parse_config_str(
            Path::new(r"C:\work\profiles"),
            &profile_config("work", "[]"),
        )
        .expect("config should parse");

        let error = resolve_file_config(&file_config, Some("missing"), resolve_test_monitor)
            .expect_err("unknown profile should fail");

        match error {
            ConfigError::Parse {
                line: None,
                message,
            } => assert!(message.contains("profile not found: missing")),
            other => panic!("unexpected error: {other}"),
        }
    }

    #[test]
    fn parse_config_allows_duplicate_assignments_across_profiles() {
        let file_config = parse_config_str(
            Path::new(r"C:\work\profiles"),
            &multi_profile_config(
                "work",
                &format!(
                    r#"{{
    "work": {{
      "assignments": [{{
        "monitor_device_path": "{}",
        "color_mode": "sdr",
        "lut_path": "work.cube"
      }}]
    }},
    "gaming": {{
      "assignments": [{{
        "monitor_device_path": "{}",
        "color_mode": "sdr",
        "lut_path": "gaming.cube"
      }}]
    }}
  }}"#,
                    test_monitor_device_path_json(),
                    test_monitor_device_path_json()
                ),
            ),
        )
        .expect("duplicate assignments across profiles should parse");

        assert_eq!(
            file_config
                .resolve_profile(Some("work"))
                .expect("work profile should exist")
                .1
                .len(),
            1
        );
        assert_eq!(
            file_config
                .resolve_profile(Some("gaming"))
                .expect("gaming profile should exist")
                .1
                .len(),
            1
        );
    }

    #[test]
    fn resolve_file_config_reports_unknown_monitor_device_path() {
        let file_config = parse_config_str(
            Path::new(r"C:\work\profiles"),
            &profile_config(
                "work",
                r#"[{
      "monitor_device_path": "\\\\?\\DISPLAY#MISSING#0&0&0&UID0#{e6f07b5f-ee97-4a90-b076-33f57bf4eaa7}",
      "color_mode": "sdr",
      "lut_path": "panel.cube"
    }]"#,
            ),
        )
        .expect("config should parse");

        let error = resolve_file_config(&file_config, None, |_| {
            Err(ConfigError::parse_message("monitor_device_path not found"))
        })
        .expect_err("unknown monitor path should fail");

        match error {
            ConfigError::Parse {
                line: None,
                message,
            } => assert!(message.contains("monitor_device_path not found")),
            other => panic!("unexpected error: {other}"),
        }
    }

    #[test]
    fn file_config_resolve_profile_matches_default() {
        let file_config = FileConfig {
            default_profile: "work".to_string(),
            profiles: HashMap::from([(
                "work".to_string(),
                super::FileProfileConfig {
                    assignments: Vec::new(),
                },
            )]),
        };

        let (profile_name, assignments) = file_config
            .resolve_profile(None)
            .expect("default profile should resolve");
        assert_eq!(profile_name, "work");
        assert!(assignments.is_empty());
    }

    #[test]
    fn parse_config_trims_default_profile_and_profile_keys() {
        let file_config = parse_config_str(
            Path::new(r"C:\work\profiles"),
            &format!(
                r#"{{
  "default_profile": " work ",
  "profiles": {{
    " work ": {{
      "assignments": [{{
        "monitor_device_path": "{}",
        "color_mode": "sdr",
        "lut_path": "work.cube"
      }}]
    }},
    " gaming ": {{
      "assignments": []
    }}
  }}
}}"#,
                test_monitor_device_path_json()
            ),
        )
        .expect("whitespace in profile names should be trimmed");

        assert_eq!(file_config.default_profile, "work");
        assert!(file_config.profiles.contains_key("work"));
        assert!(file_config.profiles.contains_key("gaming"));

        let (profile_name, config) = resolve_file_config(&file_config, None, resolve_test_monitor)
            .expect("trimmed default profile should resolve");
        assert_eq!(profile_name, "work");
        assert_eq!(config.assignments.len(), 1);

        let (profile_name, config) =
            resolve_file_config(&file_config, Some("gaming"), resolve_test_monitor)
                .expect("trimmed profile key should resolve");
        assert_eq!(profile_name, "gaming");
        assert!(config.assignments.is_empty());
    }

    #[test]
    fn parse_config_rejects_duplicate_profile_names_after_trim() {
        let error = parse_config_str(
            Path::new(r"C:\work\profiles"),
            r#"{ "default_profile": "work", "profiles": { "work": { "assignments": [] }, " work ": { "assignments": [] } } }"#,
        )
        .expect_err("profile names differing only by surrounding whitespace should fail");

        match error {
            ConfigError::Parse {
                line: None,
                message,
            } => assert!(
                message.contains("duplicate profile name:"),
                "unexpected message: {message}"
            ),
            other => panic!("unexpected error: {other}"),
        }
    }

    #[test]
    fn parse_config_rejects_whitespace_default_profile() {
        let error = parse_config_str(
            Path::new(r"C:\work\profiles"),
            r#"{ "default_profile": "   ", "profiles": { "work": { "assignments": [] } } }"#,
        )
        .expect_err("whitespace default_profile should fail");

        match error {
            ConfigError::Parse {
                line: None,
                message,
            } => assert!(message.contains("default_profile must not be empty")),
            other => panic!("unexpected error: {other}"),
        }
    }

    #[test]
    fn parse_config_rejects_empty_profile_name_key() {
        let error = parse_config_str(
            Path::new(r"C:\work\profiles"),
            r#"{ "default_profile": "work", "profiles": { "": { "assignments": [] }, "work": { "assignments": [] } } }"#,
        )
        .expect_err("empty profile name should fail");

        match error {
            ConfigError::Parse {
                line: None,
                message,
            } => assert!(message.contains("profile name must not be empty")),
            other => panic!("unexpected error: {other}"),
        }
    }

    #[test]
    fn parse_config_rejects_whitespace_profile_name_key() {
        let error = parse_config_str(
            Path::new(r"C:\work\profiles"),
            r#"{ "default_profile": "work", "profiles": { "   ": { "assignments": [] }, "work": { "assignments": [] } } }"#,
        )
        .expect_err("whitespace profile name should fail");

        match error {
            ConfigError::Parse {
                line: None,
                message,
            } => assert!(message.contains("profile name must not be empty")),
            other => panic!("unexpected error: {other}"),
        }
    }

    #[test]
    fn load_config_selects_named_profile_case_insensitively_from_file() {
        use std::fs;

        let dir =
            std::env::temp_dir().join(format!("dwm-lut-test-{}-profile-case", std::process::id()));
        fs::create_dir_all(&dir).expect("temp dir should be created");
        let path = dir.join("config.json");
        fs::write(
            &path,
            multi_profile_config(
                "work",
                r#"{
    "work": { "assignments": [] },
    "gaming": { "assignments": [] }
  }"#,
            ),
        )
        .expect("config file should be written");

        let loaded = load_config(&path, Some("GAMING")).expect("mixed-case profile should load");
        assert_eq!(loaded.profile_name, "gaming");
        assert!(loaded.config.assignments.is_empty());

        fs::remove_dir_all(&dir).expect("temp dir should be removed");
    }

    #[test]
    fn load_config_reads_named_profile_from_file() {
        use std::fs;

        let dir = std::env::temp_dir().join(format!("dwm-lut-test-{}", std::process::id()));
        fs::create_dir_all(&dir).expect("temp dir should be created");
        let path = dir.join("config.json");
        fs::write(
            &path,
            multi_profile_config(
                "work",
                r#"{
    "work": { "assignments": [] },
    "gaming": { "assignments": [] }
  }"#,
            ),
        )
        .expect("config file should be written");

        let loaded = load_config(&path, Some("gaming")).expect("named profile should load");
        assert_eq!(loaded.profile_name, "gaming");
        assert!(loaded.config.assignments.is_empty());

        fs::remove_dir_all(&dir).expect("temp dir should be removed");
    }
}
