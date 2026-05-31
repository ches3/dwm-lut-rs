use std::collections::HashSet;
use std::fmt;
use std::fs;
use std::path::{Path, PathBuf};

pub use dwm_lut_payload::{ColorMode, MonitorIdentity, MonitorTarget, PayloadLut};
use dwm_lut_payload::{
    HookPayload, PayloadAssignment, PayloadError, validate_lut, validate_payload,
};
use serde::Deserialize;

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

pub type LutCube = PayloadLut;

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
    fn parse_at(line: usize, message: impl Into<String>) -> Self {
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
            lut: parse_cube(&assignment.lut_path)?,
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

pub fn parse_cube(path: &Path) -> Result<LutCube, ConfigError> {
    let contents = fs::read_to_string(path)?;
    let lut = parse_cube_str(&contents)?;
    validate_lut(&lut).map_err(|source| ConfigError::InvalidLut {
        path: path.to_path_buf(),
        source,
    })?;
    Ok(lut)
}

pub fn parse_cube_str(contents: &str) -> Result<LutCube, ConfigError> {
    let mut size = None;
    let mut domain_min = [0.0, 0.0, 0.0];
    let mut domain_max = [1.0, 1.0, 1.0];
    let mut domain_min_seen = false;
    let mut domain_max_seen = false;
    let mut lut_data_started = false;
    let mut values = Vec::new();

    for (index, raw_line) in contents.lines().enumerate() {
        let line_no = index + 1;
        let line = strip_comments(raw_line).trim();

        if line.is_empty() {
            continue;
        }

        let tokens: Vec<&str> = line.split_whitespace().collect();
        match tokens[0] {
            "TITLE" => continue,
            "LUT_1D_SIZE" => {
                return Err(ConfigError::Unsupported("1D .cube LUTs are not supported"));
            }
            "LUT_3D_SIZE" => {
                if size.is_some() {
                    return Err(ConfigError::parse_at(
                        line_no,
                        "LUT_3D_SIZE must appear only once",
                    ));
                }

                size = Some(parse_u32_directive("LUT_3D_SIZE", line_no, &tokens[1..])?);
            }
            "DOMAIN_MIN" => {
                if lut_data_started {
                    return Err(ConfigError::parse_at(
                        line_no,
                        "DOMAIN_MIN must appear before LUT data",
                    ));
                }
                if domain_min_seen {
                    return Err(ConfigError::parse_at(
                        line_no,
                        "DOMAIN_MIN must appear only once",
                    ));
                }

                domain_min = parse_triplet_directive("DOMAIN_MIN", line_no, &tokens[1..])?;
                domain_min_seen = true;
            }
            "DOMAIN_MAX" => {
                if lut_data_started {
                    return Err(ConfigError::parse_at(
                        line_no,
                        "DOMAIN_MAX must appear before LUT data",
                    ));
                }
                if domain_max_seen {
                    return Err(ConfigError::parse_at(
                        line_no,
                        "DOMAIN_MAX must appear only once",
                    ));
                }

                domain_max = parse_triplet_directive("DOMAIN_MAX", line_no, &tokens[1..])?;
                domain_max_seen = true;
            }
            _ => {
                let lut_size = size.ok_or_else(|| {
                    ConfigError::parse_at(line_no, "encountered LUT data before LUT_3D_SIZE")
                })?;
                let expected_entries = expected_entry_count(lut_size)?;

                values.push(parse_triplet_directive("LUT value", line_no, &tokens)?);
                lut_data_started = true;

                if values.len() > expected_entries {
                    return Err(ConfigError::parse_at(
                        line_no,
                        format!("too many LUT entries: expected {expected_entries}, found more"),
                    ));
                }
            }
        }
    }

    let size = size.ok_or_else(|| ConfigError::parse_message("missing LUT_3D_SIZE directive"))?;

    Ok(LutCube {
        size,
        domain_min,
        domain_max,
        values,
    })
}

fn strip_comments(line: &str) -> &str {
    match line.find('#') {
        Some(index) => &line[..index],
        None => line,
    }
}

fn parse_u32_directive(directive: &str, line_no: usize, args: &[&str]) -> Result<u32, ConfigError> {
    if args.len() != 1 {
        return Err(ConfigError::parse_at(
            line_no,
            format!("{directive} expects exactly 1 value"),
        ));
    }

    let value = args[0].parse::<u32>().map_err(|_| {
        ConfigError::parse_at(line_no, format!("{directive} must be an unsigned integer"))
    })?;

    if value == 0 {
        return Err(ConfigError::parse_at(
            line_no,
            format!("{directive} must be greater than 0"),
        ));
    }

    Ok(value)
}

fn parse_triplet_directive(
    directive: &str,
    line_no: usize,
    args: &[&str],
) -> Result<[f32; 3], ConfigError> {
    if args.len() != 3 {
        return Err(ConfigError::parse_at(
            line_no,
            format!("{directive} expects exactly 3 values"),
        ));
    }

    Ok([
        parse_f32(line_no, directive, args[0])?,
        parse_f32(line_no, directive, args[1])?,
        parse_f32(line_no, directive, args[2])?,
    ])
}

fn parse_f32(line_no: usize, directive: &str, value: &str) -> Result<f32, ConfigError> {
    let parsed = value.parse::<f32>().map_err(|_| {
        ConfigError::parse_at(
            line_no,
            format!("{directive} contains an invalid float: {value}"),
        )
    })?;

    Ok(parsed)
}

fn expected_entry_count(size: u32) -> Result<usize, ConfigError> {
    let size = usize::try_from(size)
        .map_err(|_| ConfigError::parse_message("LUT_3D_SIZE does not fit into usize"))?;

    size.checked_mul(size)
        .and_then(|value| value.checked_mul(size))
        .ok_or_else(|| ConfigError::parse_message("LUT_3D_SIZE is too large"))
}

#[cfg(test)]
mod tests {
    use std::path::{Path, PathBuf};

    use super::{
        ColorMode, ConfigError, MonitorIdentity, parse_config_str, parse_cube_str,
        resolve_file_config,
    };
    use dwm_lut_payload::AdapterLuid;

    #[test]
    fn parse_cube_accepts_comments_and_fractional_values() {
        let cube = parse_cube_str(
            r#"
# generated by test
TITLE "sample"
LUT_3D_SIZE 2
DOMAIN_MIN -1.0 0.0 .5
DOMAIN_MAX 1.0 1.0 1.0

0.0 0.0 0.0
.5 -.25 1.0
0.1 0.2 0.3
0.4 0.5 0.6
0.7 0.8 0.9
1.0 1.0 1.0
-0.2 .25 .75
0.9 0.1 0.2 # trailing comment
"#,
        )
        .expect("cube should parse");

        assert_eq!(cube.size, 2);
        assert_eq!(cube.domain_min, [-1.0, 0.0, 0.5]);
        assert_eq!(cube.domain_max, [1.0, 1.0, 1.0]);
        assert_eq!(cube.values.len(), 8);
        assert_eq!(cube.values[1], [0.5, -0.25, 1.0]);
        assert_eq!(cube.values[6], [-0.2, 0.25, 0.75]);
    }

    #[test]
    fn parse_cube_reports_missing_size() {
        let error = parse_cube_str("0.0 0.0 0.0").expect_err("parse should fail");

        match error {
            ConfigError::Parse {
                line: Some(1),
                message,
            } => {
                assert!(message.contains("before LUT_3D_SIZE"));
            }
            other => panic!("unexpected error: {other}"),
        }
    }

    #[test]
    fn parse_cube_rejects_duplicate_size_directive() {
        let error = parse_cube_str(
            r#"
LUT_3D_SIZE 2
LUT_3D_SIZE 3
"#,
        )
        .expect_err("parse should fail");

        match error {
            ConfigError::Parse {
                line: Some(3),
                message,
            } => {
                assert!(message.contains("must appear only once"));
            }
            other => panic!("unexpected error: {other}"),
        }
    }

    #[test]
    fn parse_cube_rejects_domain_directive_after_lut_data() {
        let error = parse_cube_str(
            r#"
LUT_3D_SIZE 2
0.0 0.0 0.0
DOMAIN_MAX 1.0 1.0 1.0
"#,
        )
        .expect_err("parse should fail");

        match error {
            ConfigError::Parse {
                line: Some(4),
                message,
            } => {
                assert!(message.contains("DOMAIN_MAX must appear before LUT data"));
            }
            other => panic!("unexpected error: {other}"),
        }
    }

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
