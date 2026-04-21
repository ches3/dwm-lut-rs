use std::collections::HashSet;
use std::fmt;
use std::fs;
use std::path::{Path, PathBuf};

use serde::Deserialize;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ColorMode {
    Sdr,
    Hdr,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MonitorTarget {
    pub monitor_id: String,
    pub desktop_left: i32,
    pub desktop_top: i32,
    pub color_mode: ColorMode,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LutAssignment {
    pub target: MonitorTarget,
    pub lut_path: PathBuf,
    pub lut_size: u32,
}

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct LutManifest {
    pub assignments: Vec<LutAssignment>,
}

impl LutManifest {
    pub fn empty() -> Self {
        Self::default()
    }

    pub fn add(&mut self, assignment: LutAssignment) {
        self.assignments.push(assignment);
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct LutCube {
    pub size: u32,
    pub domain_min: [f32; 3],
    pub domain_max: [f32; 3],
    pub values: Vec<[f32; 3]>,
}

#[derive(Debug)]
pub enum ConfigError {
    Io(std::io::Error),
    Parse {
        line: Option<usize>,
        message: String,
    },
    Unsupported(&'static str),
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

    fn parse_message(message: impl Into<String>) -> Self {
        Self::Parse {
            line: None,
            message: message.into(),
        }
    }
}

#[derive(Debug, Deserialize)]
struct ManifestDocument {
    assignments: Vec<ManifestAssignmentDocument>,
}

#[derive(Debug, Deserialize)]
struct ManifestAssignmentDocument {
    monitor_id: String,
    desktop_left: i32,
    desktop_top: i32,
    color_mode: ManifestColorMode,
    lut_path: PathBuf,
    lut_size: u32,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Deserialize)]
#[serde(rename_all = "snake_case")]
enum ManifestColorMode {
    Sdr,
    Hdr,
}

impl From<ManifestColorMode> for ColorMode {
    fn from(value: ManifestColorMode) -> Self {
        match value {
            ManifestColorMode::Sdr => Self::Sdr,
            ManifestColorMode::Hdr => Self::Hdr,
        }
    }
}

pub fn load_manifest(path: &Path) -> Result<LutManifest, ConfigError> {
    let contents = fs::read_to_string(path)?;
    let base_dir = path.parent().unwrap_or_else(|| Path::new("."));
    load_manifest_str(base_dir, &contents)
}

pub fn load_manifest_str(base_dir: &Path, contents: &str) -> Result<LutManifest, ConfigError> {
    let document: ManifestDocument = serde_json::from_str(contents).map_err(|error| {
        let line = match error.line() {
            0 => None,
            line => Some(line),
        };

        ConfigError::Parse {
            line,
            message: error.to_string(),
        }
    })?;

    let mut manifest = LutManifest::empty();
    let mut selection_keys = HashSet::new();
    for assignment in document.assignments {
        if assignment.monitor_id.trim().is_empty() {
            return Err(ConfigError::parse_message("monitor_id must not be empty"));
        }

        if assignment.lut_size == 0 {
            return Err(ConfigError::parse_message(
                "lut_size must be greater than 0",
            ));
        }

        let lut_path = if assignment.lut_path.is_absolute() {
            assignment.lut_path
        } else {
            base_dir.join(assignment.lut_path)
        };

        let selection_key = (
            assignment.desktop_left,
            assignment.desktop_top,
            assignment.color_mode,
        );
        if !selection_keys.insert(selection_key) {
            return Err(ConfigError::parse_message(format!(
                "duplicate assignment for desktop_left={}, desktop_top={}, color_mode={:?}",
                assignment.desktop_left, assignment.desktop_top, assignment.color_mode
            )));
        }

        manifest.add(LutAssignment {
            target: MonitorTarget {
                monitor_id: assignment.monitor_id,
                desktop_left: assignment.desktop_left,
                desktop_top: assignment.desktop_top,
                color_mode: assignment.color_mode.into(),
            },
            lut_path,
            lut_size: assignment.lut_size,
        });
    }

    Ok(manifest)
}

pub fn parse_cube(path: &Path) -> Result<LutCube, ConfigError> {
    let contents = fs::read_to_string(path)?;
    parse_cube_str(&contents)
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
    let expected_entries = expected_entry_count(size)?;

    if values.len() != expected_entries {
        return Err(ConfigError::parse_message(format!(
            "expected {expected_entries} LUT entries for size {size}, found {}",
            values.len()
        )));
    }

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

    if !parsed.is_finite() {
        return Err(ConfigError::parse_at(
            line_no,
            format!("{directive} must contain only finite values"),
        ));
    }

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

    use super::{ColorMode, ConfigError, load_manifest_str, parse_cube_str};

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
    fn parse_cube_reports_incomplete_entry_count() {
        let error = parse_cube_str(
            r#"
LUT_3D_SIZE 2
0.0 0.0 0.0
0.1 0.1 0.1
"#,
        )
        .expect_err("parse should fail");

        match error {
            ConfigError::Parse {
                line: None,
                message,
            } => {
                assert!(message.contains("expected 8 LUT entries"));
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
    fn parse_cube_rejects_non_finite_values() {
        let error = parse_cube_str(
            r#"
LUT_3D_SIZE 2
DOMAIN_MIN NaN 0.0 0.0
0.0 0.0 0.0
0.1 0.1 0.1
0.2 0.2 0.2
0.3 0.3 0.3
0.4 0.4 0.4
0.5 0.5 0.5
0.6 0.6 0.6
0.7 0.7 0.7
"#,
        )
        .expect_err("parse should fail");

        match error {
            ConfigError::Parse {
                line: Some(3),
                message,
            } => {
                assert!(message.contains("finite values"));
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

    #[test]
    fn load_manifest_resolves_relative_lut_paths() {
        let manifest = load_manifest_str(
            Path::new(r"C:\work\profiles"),
            r#"
{
  "assignments": [
    {
      "monitor_id": "DISPLAY1",
      "desktop_left": 0,
      "desktop_top": 0,
      "color_mode": "sdr",
      "lut_path": "panel.cube",
      "lut_size": 33
    }
  ]
}
"#,
        )
        .expect("manifest should parse");

        assert_eq!(manifest.assignments.len(), 1);
        assert_eq!(manifest.assignments[0].target.monitor_id, "DISPLAY1");
        assert_eq!(manifest.assignments[0].target.color_mode, ColorMode::Sdr);
        assert_eq!(
            manifest.assignments[0].lut_path,
            PathBuf::from(r"C:\work\profiles").join("panel.cube")
        );
        assert_eq!(manifest.assignments[0].lut_size, 33);
    }

    #[test]
    fn load_manifest_rejects_empty_monitor_id() {
        let error = load_manifest_str(
            Path::new(r"C:\work\profiles"),
            r#"
{
  "assignments": [
    {
      "monitor_id": "  ",
      "desktop_left": 0,
      "desktop_top": 0,
      "color_mode": "sdr",
      "lut_path": "panel.cube",
      "lut_size": 33
    }
  ]
}
"#,
        )
        .expect_err("empty monitor_id should fail");

        match error {
            ConfigError::Parse {
                line: None,
                message,
            } => assert!(message.contains("monitor_id must not be empty")),
            other => panic!("unexpected error: {other}"),
        }
    }

    #[test]
    fn load_manifest_requires_assignments_field() {
        let error = load_manifest_str(Path::new(r"C:\work\profiles"), "{}")
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
    fn load_manifest_accepts_empty_assignments_array() {
        let manifest =
            load_manifest_str(Path::new(r"C:\work\profiles"), r#"{ "assignments": [] }"#)
                .expect("empty assignments array should still parse");

        assert!(manifest.assignments.is_empty());
    }

    #[test]
    fn load_manifest_rejects_duplicate_selection_keys() {
        let error = load_manifest_str(
            Path::new(r"C:\work\profiles"),
            r#"
{
  "assignments": [
    {
      "monitor_id": "DISPLAY1",
      "desktop_left": 0,
      "desktop_top": 0,
      "color_mode": "sdr",
      "lut_path": "panel-a.cube",
      "lut_size": 33
    },
    {
      "monitor_id": "DISPLAY2",
      "desktop_left": 0,
      "desktop_top": 0,
      "color_mode": "sdr",
      "lut_path": "panel-b.cube",
      "lut_size": 33
    }
  ]
}
"#,
        )
        .expect_err("duplicate selection keys should fail");

        match error {
            ConfigError::Parse {
                line: None,
                message,
            } => assert!(message.contains("duplicate assignment")),
            other => panic!("unexpected error: {other}"),
        }
    }
}
