use std::fmt;
use std::path::{Path, PathBuf};

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
    pub values: Vec<[f32; 3]>,
}

#[derive(Debug)]
pub enum ConfigError {
    Io(std::io::Error),
    Parse(&'static str),
    Unsupported(&'static str),
}

impl fmt::Display for ConfigError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Io(err) => write!(f, "io error: {err}"),
            Self::Parse(message) => write!(f, "parse error: {message}"),
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

pub fn load_manifest(_path: &Path) -> Result<LutManifest, ConfigError> {
    Err(ConfigError::Unsupported(
        "manifest loader is not implemented yet",
    ))
}

pub fn parse_cube(_path: &Path) -> Result<LutCube, ConfigError> {
    Err(ConfigError::Unsupported(
        ".cube parser is not implemented yet",
    ))
}

