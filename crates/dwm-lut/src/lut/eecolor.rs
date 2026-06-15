use dwm_lut_payload::PayloadLut;

use crate::config::ConfigError;

use super::common::{
    EECOLOR_INPUT_TOLERANCE, infer_cube_size_from_line_count, parse_sextet_directive,
    strip_comments,
};

pub fn parse_eecolor_str(contents: &str) -> Result<PayloadLut, ConfigError> {
    let mut lines = Vec::new();

    for (index, raw_line) in contents.lines().enumerate() {
        let line_no = index + 1;
        let line = strip_comments(raw_line).trim();
        if line.is_empty() {
            continue;
        }

        let tokens: Vec<&str> = line.split_whitespace().collect();
        lines.push((
            line_no,
            parse_sextet_directive("eeColor LUT value", line_no, &tokens)?,
        ));
    }

    if lines.is_empty() {
        return Err(ConfigError::parse_message("eeColor LUT file is empty"));
    }

    let size = infer_cube_size_from_line_count(lines.len())?;
    let n = size as usize;
    let step = 1.0 / (size - 1) as f32;

    for (line_idx, (line_no, values)) in lines.iter().enumerate() {
        let green = line_idx % n;
        let red = (line_idx / n) % n;
        let blue = line_idx / (n * n);
        let expected = [red as f32 * step, green as f32 * step, blue as f32 * step];

        for (axis, (&actual, expected_value)) in values[..3].iter().zip(expected).enumerate() {
            if (actual - expected_value).abs() > EECOLOR_INPUT_TOLERANCE {
                return Err(ConfigError::parse_at(
                    *line_no,
                    format!(
                        "eeColor input coordinates do not match expected grid at (r={red}, g={green}, b={blue}): axis {axis} got {actual}, expected {expected_value}"
                    ),
                ));
            }
        }
    }

    let mut values = Vec::with_capacity(lines.len());
    for blue in 0..n {
        for green in 0..n {
            for red in 0..n {
                let source_index = green + n * (red + n * blue);
                let (_, line) = &lines[source_index];
                values.push([line[3], line[4], line[5]]);
            }
        }
    }

    Ok(PayloadLut {
        size,
        domain_min: [0.0, 0.0, 0.0],
        domain_max: [1.0, 1.0, 1.0],
        values,
    })
}

#[cfg(test)]
pub(crate) fn synthetic_eecolor(size: u32) -> String {
    let n = size as usize;
    let step = 1.0 / (size - 1) as f32;
    let mut lines = Vec::with_capacity(n * n * n);

    for blue in 0..n {
        for red in 0..n {
            for green in 0..n {
                let input = [red as f32 * step, green as f32 * step, blue as f32 * step];
                lines.push(format!(
                    "{:.6} {:.6} {:.6} {:.6} {:.6} {:.6}",
                    input[0], input[1], input[2], input[0], input[1], input[2]
                ));
            }
        }
    }

    lines.join("\n")
}

#[cfg(test)]
mod tests {
    use super::{parse_eecolor_str, synthetic_eecolor};
    use crate::config::ConfigError;

    #[test]
    fn parse_eecolor_reorders_to_standard_cube_layout() {
        let lut = parse_eecolor_str(&synthetic_eecolor(2)).expect("eeColor should parse");

        assert_eq!(lut.size, 2);
        assert_eq!(lut.domain_min, [0.0, 0.0, 0.0]);
        assert_eq!(lut.domain_max, [1.0, 1.0, 1.0]);
        assert_eq!(lut.values.len(), 8);
        assert_eq!(lut.values[0], [0.0, 0.0, 0.0]);
        assert_eq!(lut.values[1], [1.0, 0.0, 0.0]);
        assert_eq!(lut.values[2], [0.0, 1.0, 0.0]);
        assert_eq!(lut.values[3], [1.0, 1.0, 0.0]);
        assert_eq!(lut.values[4], [0.0, 0.0, 1.0]);
        assert_eq!(lut.values[7], [1.0, 1.0, 1.0]);
    }

    #[test]
    fn parse_eecolor_rejects_non_cubic_line_count() {
        let error = parse_eecolor_str("0 0 0 0 0 0\n0 0 0 0 0 0\n")
            .expect_err("non-cubic eeColor should fail");

        match error {
            ConfigError::Parse {
                line: None,
                message,
            } => {
                assert!(message.contains("not a perfect cube"));
            }
            other => panic!("unexpected error: {other}"),
        }
    }

    #[test]
    fn parse_eecolor_rejects_mismatched_input_grid() {
        let contents = "0 0 0 0 0 0\n0.5 0 0 0 0 0\n0 1 0 0 1 0\n1 1 0 1 1 0\n0 0 1 0 0 1\n1 1 1 1 1 1\n0 0 0 0 0 0\n1 1 1 1 1 1\n";
        let error = parse_eecolor_str(contents).expect_err("bad grid should fail");

        match error {
            ConfigError::Parse {
                line: Some(line),
                message,
            } => {
                assert_eq!(line, 2);
                assert!(message.contains("input coordinates"));
            }
            other => panic!("unexpected error: {other}"),
        }
    }

    #[test]
    fn parse_eecolor_rejects_wrong_token_count() {
        let error = parse_eecolor_str("0 0 0 0 0\n").expect_err("short line should fail");

        match error {
            ConfigError::Parse {
                line: Some(1),
                message,
            } => {
                assert!(message.contains("6 values"));
            }
            other => panic!("unexpected error: {other}"),
        }
    }
}
