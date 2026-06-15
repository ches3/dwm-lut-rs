use crate::backend::{DesktopPosition, DesktopResolution, MonitorListing, list_monitor_listings};
use crate::error::InjectorError;

const UNKNOWN_NAME: &str = "(unknown)";

pub(crate) fn run_monitors() -> Result<(), InjectorError> {
    let listings = list_monitor_listings()?;
    println!("{}", format_monitor_list(&listings));
    Ok(())
}

fn format_monitor_list(listings: &[MonitorListing]) -> String {
    if listings.is_empty() {
        return "No active monitors.".to_string();
    }

    let mut rows: Vec<MonitorRow> = listings.iter().map(MonitorRow::from).collect();
    rows.sort_by_key(|row| row.number);

    let headers = ["#", "Name", "Model", "Position", "Resolution", "Path"];
    let mut widths = headers.map(str::len);
    for row in &rows {
        let cells = row.cells();
        for (width, cell) in widths.iter_mut().zip(cells) {
            *width = (*width).max(display_width(cell));
        }
    }

    let mut lines = Vec::new();
    lines.push(format!("Monitors ({} active)", rows.len()));
    lines.push(String::new());
    lines.push(format_row(&headers, &widths));
    lines.push(format_separator(&widths));
    for row in &rows {
        lines.push(format_row(&row.cells(), &widths));
    }

    lines.join("\n")
}

struct MonitorRow {
    number: u32,
    number_text: String,
    name: String,
    model: String,
    position: String,
    resolution: String,
    path: String,
}

impl From<&MonitorListing> for MonitorRow {
    fn from(listing: &MonitorListing) -> Self {
        Self {
            number: listing.number,
            number_text: listing.number.to_string(),
            name: display_name(&listing.friendly_name).to_string(),
            model: listing.edid_pnp_id.clone(),
            position: format_position(listing.position),
            resolution: format_resolution(listing.resolution),
            path: listing.monitor_device_path.clone(),
        }
    }
}

impl MonitorRow {
    fn cells(&self) -> [&str; 6] {
        [
            &self.number_text,
            &self.name,
            &self.model,
            &self.position,
            &self.resolution,
            &self.path,
        ]
    }
}

fn format_row(cells: &[&str; 6], widths: &[usize; 6]) -> String {
    let formatted = cells
        .iter()
        .zip(widths)
        .enumerate()
        .map(|(index, (cell, width))| {
            if index + 1 == cells.len() {
                (*cell).to_string()
            } else if index == 0 {
                align_right(cell, *width)
            } else {
                align_left(cell, *width)
            }
        })
        .collect::<Vec<_>>();

    formatted.join("  ")
}

fn format_separator(widths: &[usize; 6]) -> String {
    widths
        .iter()
        .map(|width| "-".repeat(*width))
        .collect::<Vec<_>>()
        .join("  ")
}

fn align_left(value: &str, width: usize) -> String {
    format!(
        "{value}{}",
        " ".repeat(width.saturating_sub(display_width(value)))
    )
}

fn align_right(value: &str, width: usize) -> String {
    format!(
        "{}{}",
        " ".repeat(width.saturating_sub(display_width(value))),
        value
    )
}

fn display_name(name: &str) -> &str {
    if name.is_empty() { UNKNOWN_NAME } else { name }
}

fn format_position(position: DesktopPosition) -> String {
    format!("({}, {})", position.x, position.y)
}

fn format_resolution(resolution: DesktopResolution) -> String {
    format!("{}x{}", resolution.width, resolution.height)
}

fn display_width(value: &str) -> usize {
    value.chars().count()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn listing(
        number: u32,
        friendly_name: &str,
        edid_pnp_id: &str,
        position: DesktopPosition,
        resolution: DesktopResolution,
        monitor_device_path: &str,
    ) -> MonitorListing {
        MonitorListing {
            number,
            friendly_name: friendly_name.to_string(),
            edid_pnp_id: edid_pnp_id.to_string(),
            position,
            resolution,
            monitor_device_path: monitor_device_path.to_string(),
        }
    }

    #[test]
    fn formats_empty_monitor_list() {
        assert_eq!(format_monitor_list(&[]), "No active monitors.");
    }

    #[test]
    fn formats_fixed_width_monitor_table_sorted_by_number() {
        let monitors = [
            listing(
                2,
                "",
                "BNQ7F59",
                DesktopPosition { x: 2560, y: 177 },
                DesktopResolution {
                    width: 1920,
                    height: 1080,
                },
                r"\\?\DISPLAY#BNQ7F59#UID2",
            ),
            listing(
                1,
                "P275MS PRO",
                "LHC91C1",
                DesktopPosition { x: 0, y: 0 },
                DesktopResolution {
                    width: 2560,
                    height: 1440,
                },
                r"\\?\DISPLAY#LHC91C1#UID1",
            ),
        ];

        assert_eq!(
            format_monitor_list(&monitors),
            concat!(
                "Monitors (2 active)\n",
                "\n",
                "#  Name        Model    Position     Resolution  Path\n",
                "-  ----------  -------  -----------  ----------  ------------------------\n",
                "1  P275MS PRO  LHC91C1  (0, 0)       2560x1440   \\\\?\\DISPLAY#LHC91C1#UID1\n",
                "2  (unknown)   BNQ7F59  (2560, 177)  1920x1080   \\\\?\\DISPLAY#BNQ7F59#UID2",
            )
        );
    }
}
