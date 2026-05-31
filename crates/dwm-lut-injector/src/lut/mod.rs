mod common;
mod cube;
mod eecolor;

use std::fs;
use std::path::Path;

use dwm_lut_payload::{PayloadLut, validate_lut};

use crate::config::ConfigError;

pub use cube::parse_cube_str;
pub use eecolor::parse_eecolor_str;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum LutFormat {
    Cube,
    EeColor,
}

pub fn parse_lut(path: &Path) -> Result<PayloadLut, ConfigError> {
    let contents = fs::read_to_string(path)?;
    let lut = parse_lut_str(&contents)?;
    validate_lut(&lut).map_err(|source| ConfigError::InvalidLut {
        path: path.to_path_buf(),
        source,
    })?;
    Ok(lut)
}

pub fn parse_lut_str(contents: &str) -> Result<PayloadLut, ConfigError> {
    match detect_format(contents)? {
        LutFormat::Cube => parse_cube_str(contents),
        LutFormat::EeColor => parse_eecolor_str(contents),
    }
}

fn detect_format(contents: &str) -> Result<LutFormat, ConfigError> {
    for (index, raw_line) in contents.lines().enumerate() {
        let line = common::strip_comments(raw_line).trim();
        if line.is_empty() {
            continue;
        }

        let tokens: Vec<&str> = line.split_whitespace().collect();
        return match tokens.first() {
            Some(&"LUT_3D_SIZE") | Some(&"TITLE") | Some(&"DOMAIN_MIN") | Some(&"DOMAIN_MAX") => {
                Ok(LutFormat::Cube)
            }
            Some(&"LUT_1D_SIZE") => {
                Err(ConfigError::Unsupported("1D .cube LUTs are not supported"))
            }
            _ => match tokens.len() {
                6 => Ok(LutFormat::EeColor),
                3 => Err(ConfigError::parse_at(
                    index + 1,
                    "encountered LUT data before LUT_3D_SIZE",
                )),
                count => Err(ConfigError::parse_at(
                    index + 1,
                    format!("expected 3 or 6 LUT values, found {count}"),
                )),
            },
        };
    }

    Err(ConfigError::parse_message("LUT file is empty"))
}

#[cfg(test)]
mod tests {
    use super::{LutFormat, detect_format, parse_lut_str};
    use crate::config::ConfigError;
    use crate::lut::eecolor::synthetic_eecolor;

    #[test]
    fn detect_format_recognizes_standard_cube_header() {
        let format = detect_format("LUT_3D_SIZE 2\n0 0 0\n").expect("cube header should parse");
        assert_eq!(format, LutFormat::Cube);
    }

    #[test]
    fn detect_format_recognizes_eecolor_without_header() {
        let format = detect_format("0 0 0 0 0 0\n").expect("eeColor should parse");
        assert_eq!(format, LutFormat::EeColor);
    }

    #[test]
    fn parse_lut_str_dispatches_to_cube_parser() {
        let lut = parse_lut_str(
            r#"
LUT_3D_SIZE 2
0 0 0
1 0 0
0 1 0
1 1 0
0 0 1
1 0 1
0 1 1
1 1 1
"#,
        )
        .expect("cube should parse");

        assert_eq!(lut.size, 2);
        assert_eq!(lut.values.len(), 8);
    }

    #[test]
    fn parse_lut_str_dispatches_to_eecolor_parser() {
        let lut = parse_lut_str(&synthetic_eecolor(2)).expect("eeColor should parse");
        assert_eq!(lut.size, 2);
        assert_eq!(lut.values[1], [1.0, 0.0, 0.0]);
    }

    #[test]
    fn parse_lut_str_reorders_eecolor_output_values() {
        let lut = parse_lut_str(
            r#"
0 0 0 0.10 0.11 0.12
0 1 0 0.20 0.21 0.22
1 0 0 0.30 0.31 0.32
1 1 0 0.40 0.41 0.42
0 0 1 0.50 0.51 0.52
0 1 1 0.60 0.61 0.62
1 0 1 0.70 0.71 0.72
1 1 1 0.80 0.81 0.82
"#,
        )
        .expect("eeColor should parse");

        assert_eq!(lut.size, 2);
        assert_eq!(
            lut.values,
            vec![
                [0.10, 0.11, 0.12],
                [0.30, 0.31, 0.32],
                [0.20, 0.21, 0.22],
                [0.40, 0.41, 0.42],
                [0.50, 0.51, 0.52],
                [0.70, 0.71, 0.72],
                [0.60, 0.61, 0.62],
                [0.80, 0.81, 0.82],
            ]
        );
    }

    #[test]
    fn detect_format_rejects_bare_cube_data_lines() {
        let error = detect_format("0 0 0\n").expect_err("bare triplet should fail");
        match error {
            ConfigError::Parse {
                line: Some(1),
                message,
            } => assert!(message.contains("before LUT_3D_SIZE")),
            other => panic!("unexpected error: {other}"),
        }
    }
}
