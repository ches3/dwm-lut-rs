use std::collections::HashSet;
use std::fmt;
use std::fs;
use std::path::{Path, PathBuf};

pub use dwm_lut_payload::{ColorMode, MonitorIdentity, MonitorTarget};
use dwm_lut_payload::{HookPayload, PayloadAssignment, PayloadError, validate_payload};
use serde::Deserialize;

use crate::lut::parse_lut;
use crate::monitor::resolve_monitor_identity;

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

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct FileConfig {
    pub assignments: Vec<FileAssignment>,
}

#[derive(Debug)]
pub enum ConfigError {
    Io(std::io::Error),
    Parse {
        line: Option<usize>,
        message: String,
    },
    Unsupported(&'static str),
    InvalidLut {
        path: PathBuf,
        source: PayloadError,
    },
    InvalidPayload(PayloadError),
}

impl fmt::Display for ConfigError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Io(err) => write!(f, "io error: {err}"),
            Self::Parse {
                line: Some(line),
                message,
            } => write!(f, "parse error at line {line}: {message}"),
            Self::Parse {
                line: None,
                message,
            } => write!(f, "parse error: {message}"),
            Self::Unsupported(message) => write!(f, "unsupported: {message}"),
            Self::InvalidLut { path, source } => {
                write!(f, "invalid LUT {}: {source}", path.display())
            }
            Self::InvalidPayload(source) => write!(f, "invalid payload: {source}"),
        }
    }
}

impl std::error::Error for ConfigError {}

impl From<std::io::Error> for ConfigError {
    fn from(value: std::io::Error) -> Self {
        Self::Io(value)
    }
}

impl ConfigError {
    pub(crate) fn parse_at(line: usize, message: impl Into<String>) -> Self {
        Self::Parse {
            line: Some(line),
            message: message.into(),
        }
    }

    pub(crate) fn parse_message(message: impl Into<String>) -> Self {
        Self::Parse {
            line: None,
            message: message.into(),
        }
    }
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct ConfigDocument {
    assignments: Vec<ConfigAssignmentDocument>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct ConfigAssignmentDocument {
    monitor_device_path: String,
    color_mode: ConfigColorMode,
    lut_path: PathBuf,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Deserialize)]
#[serde(rename_all = "snake_case")]
enum ConfigColorMode {
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

pub fn load_config(path: &Path) -> Result<LutConfig, ConfigError> {
    let contents = fs::read_to_string(path)?;
    let base_dir = path.parent().unwrap_or_else(|| Path::new("."));
    let file_config = parse_config_str(base_dir, &contents)?;
    resolve_file_config(&file_config, resolve_monitor_identity)
}

pub fn load_payload(path: &Path) -> Result<HookPayload, ConfigError> {
    let config = load_config(path)?;
    config_to_payload(&config)
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
    let document: ConfigDocument = serde_json::from_str(contents).map_err(|error| {
        let line = match error.line() {
            0 => None,
            line => Some(line),
        };

        ConfigError::Parse {
            line,
            message: error.to_string(),
        }
    })?;

    let mut config = FileConfig::default();
    let mut assignment_keys = HashSet::new();
    for assignment in document.assignments {
        let lut_path = if assignment.lut_path.is_absolute() {
            assignment.lut_path
        } else {
            base_dir.join(assignment.lut_path)
        };

        let color_mode = assignment.color_mode.into();
        let assignment_key = (
            assignment.monitor_device_path.to_ascii_uppercase(),
            color_mode,
        );
        if !assignment_keys.insert(assignment_key) {
            return Err(ConfigError::parse_message(format!(
                "duplicate assignment for monitor_device_path={}, color_mode={color_mode:?}",
                assignment.monitor_device_path
            )));
        }

        config.assignments.push(FileAssignment {
            monitor_device_path: assignment.monitor_device_path,
            color_mode,
            lut_path,
        });
    }

    Ok(config)
}

pub fn resolve_file_config(
    file_config: &FileConfig,
    mut resolve: impl FnMut(&str) -> Result<MonitorIdentity, ConfigError>,
) -> Result<LutConfig, ConfigError> {
    let mut config = LutConfig::empty();
    let mut identity_keys = HashSet::new();

    for assignment in &file_config.assignments {
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

    Ok(config)
}

#[cfg(test)]
mod tests {
    use std::path::{Path, PathBuf};

    use super::{ColorMode, ConfigError, MonitorIdentity, parse_config_str, resolve_file_config};
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

    #[test]
    fn parse_config_resolves_relative_lut_paths() {
        let file_config = parse_config_str(
            Path::new(r"C:\work\profiles"),
            &format!(
                r#"
{{
  "assignments": [
    {{
      "monitor_device_path": "{}",
      "color_mode": "sdr",
      "lut_path": "panel.cube"
    }}
  ]
}}
"#,
                test_monitor_device_path_json()
            ),
        )
        .expect("config should parse");

        assert_eq!(file_config.assignments.len(), 1);
        assert_eq!(
            file_config.assignments[0].monitor_device_path,
            test_monitor_device_path()
        );
        assert_eq!(file_config.assignments[0].color_mode, ColorMode::Sdr);
        assert_eq!(
            file_config.assignments[0].lut_path,
            PathBuf::from(r"C:\work\profiles").join("panel.cube")
        );

        let config =
            resolve_file_config(&file_config, resolve_test_monitor).expect("config should resolve");
        assert_eq!(
            config.assignments[0].target.identity,
            test_monitor_identity()
        );
    }

    #[test]
    fn parse_config_rejects_duplicate_monitor_device_path_for_same_color_mode() {
        let error = parse_config_str(
            Path::new(r"C:\work\profiles"),
            &format!(
                r#"
{{
  "assignments": [
    {{
      "monitor_device_path": "{}",
      "color_mode": "sdr",
      "lut_path": "panel-a.cube"
    }},
    {{
      "monitor_device_path": "{}",
      "color_mode": "sdr",
      "lut_path": "panel-b.cube"
    }}
  ]
}}
"#,
                test_monitor_device_path_json(),
                test_monitor_device_path_json().to_ascii_uppercase()
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
            &format!(
                r#"
{{
  "assignments": [
    {{
      "monitor_device_path": "{}",
      "color_mode": "sdr",
      "lut_path": "panel-a.cube"
    }},
    {{
      "monitor_device_path": "{}",
      "color_mode": "sdr",
      "lut_path": "panel-b.cube"
    }}
  ]
}}
"#,
                test_monitor_device_path_json(),
                alternate_monitor_device_path_json()
            ),
        )
        .expect("distinct monitor paths should parse");

        let error = resolve_file_config(&file_config, |_| Ok(test_monitor_identity()))
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
            &format!(
                r#"
{{
  "assignments": [
    {{
      "monitor_device_path": "{}",
      "color_mode": "sdr",
      "lut_path": "panel-sdr.cube"
    }},
    {{
      "monitor_device_path": "{}",
      "color_mode": "hdr",
      "lut_path": "panel-hdr.cube"
    }}
  ]
}}
"#,
                test_monitor_device_path_json(),
                test_monitor_device_path_json()
            ),
        )
        .expect("SDR and HDR assignments should coexist for one monitor path");

        let config =
            resolve_file_config(&file_config, resolve_test_monitor).expect("config should resolve");

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
            r#"
{
  "assignments": [
    {
      "color_mode": "sdr",
      "lut_path": "panel.cube"
    }
  ]
}
"#,
        )
        .expect_err("missing monitor_device_path should fail");

        match error {
            ConfigError::Parse {
                line: Some(7),
                message,
            } => assert!(message.contains("missing field `monitor_device_path`")),
            other => panic!("unexpected error: {other}"),
        }
    }

    #[test]
    fn parse_config_rejects_unknown_assignment_fields() {
        let error = parse_config_str(
            Path::new(r"C:\work\profiles"),
            &format!(
                r#"
{{
  "assignments": [
    {{
      "monitor_device_path": "{}",
      "desktop_left": 0,
      "color_mode": "sdr",
      "lut_path": "panel.cube"
    }}
  ]
}}
"#,
                test_monitor_device_path_json()
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
    fn parse_config_requires_assignments_field() {
        let error = parse_config_str(Path::new(r"C:\work\profiles"), "{}")
            .expect_err("missing assignments should fail");

        match error {
            ConfigError::Parse {
                line: Some(1),
                message,
            } => assert!(message.contains("missing field `assignments`")),
            other => panic!("unexpected error: {other}"),
        }
    }

    #[test]
    fn parse_config_accepts_empty_assignments_array() {
        let config = parse_config_str(Path::new(r"C:\work\profiles"), r#"{ "assignments": [] }"#)
            .expect("empty assignments array should still parse");

        assert!(config.assignments.is_empty());
    }

    #[test]
    fn resolve_file_config_reports_unknown_monitor_device_path() {
        let file_config = parse_config_str(
            Path::new(r"C:\work\profiles"),
            r#"
{
  "assignments": [
    {
      "monitor_device_path": "\\\\?\\DISPLAY#MISSING#0&0&0&UID0#{e6f07b5f-ee97-4a90-b076-33f57bf4eaa7}",
      "color_mode": "sdr",
      "lut_path": "panel.cube"
    }
  ]
}
"#,
        )
        .expect("config should parse");

        let error = resolve_file_config(&file_config, |_| {
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
}
