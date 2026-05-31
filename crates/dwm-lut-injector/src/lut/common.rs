use crate::config::ConfigError;

pub(crate) fn strip_comments(line: &str) -> &str {
    match line.find('#') {
        Some(index) => &line[..index],
        None => line,
    }
}

pub(crate) fn parse_u32_directive(
    directive: &str,
    line_no: usize,
    args: &[&str],
) -> Result<u32, ConfigError> {
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

pub(crate) fn parse_triplet_directive(
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

pub(crate) fn parse_f32(line_no: usize, directive: &str, value: &str) -> Result<f32, ConfigError> {
    value.parse::<f32>().map_err(|_| {
        ConfigError::parse_at(
            line_no,
            format!("{directive} contains an invalid float: {value}"),
        )
    })
}

pub(crate) fn parse_sextet_directive(
    directive: &str,
    line_no: usize,
    args: &[&str],
) -> Result<[f32; 6], ConfigError> {
    if args.len() != 6 {
        return Err(ConfigError::parse_at(
            line_no,
            format!("{directive} expects exactly 6 values"),
        ));
    }

    Ok([
        parse_f32(line_no, directive, args[0])?,
        parse_f32(line_no, directive, args[1])?,
        parse_f32(line_no, directive, args[2])?,
        parse_f32(line_no, directive, args[3])?,
        parse_f32(line_no, directive, args[4])?,
        parse_f32(line_no, directive, args[5])?,
    ])
}

pub(crate) fn expected_entry_count(size: u32) -> Result<usize, ConfigError> {
    let size = usize::try_from(size)
        .map_err(|_| ConfigError::parse_message("LUT size does not fit into usize"))?;

    size.checked_mul(size)
        .and_then(|value| value.checked_mul(size))
        .ok_or_else(|| ConfigError::parse_message("LUT size is too large"))
}

pub(crate) fn infer_cube_size_from_line_count(line_count: usize) -> Result<u32, ConfigError> {
    let size = (line_count as f64).cbrt().round() as u32;
    if size < 2 {
        return Err(ConfigError::parse_message(format!(
            "eeColor LUT line count {line_count} is not a perfect cube"
        )));
    }

    let expected = (size as usize).checked_pow(3).ok_or_else(|| {
        ConfigError::parse_message("eeColor LUT cube size does not fit into usize")
    })?;
    if expected != line_count {
        return Err(ConfigError::parse_message(format!(
            "eeColor LUT line count {line_count} is not a perfect cube"
        )));
    }

    Ok(size)
}

pub(crate) const EECOLOR_INPUT_TOLERANCE: f32 = 1e-4;
