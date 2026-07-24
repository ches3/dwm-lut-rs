use std::error::Error;
use std::path::Path;

use pelite::pe64::debug::CodeView;
use pelite::pe64::{Pe, PeFile};
use pelite::resources::FindError;
use uuid::Uuid;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CodeViewInfo {
    pub guid: Uuid,
    pub age: u32,
    pub pdb_file_name: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct FileVersion {
    pub major: u16,
    pub minor: u16,
    pub build: u16,
    pub revision: u16,
}

impl FileVersion {
    pub fn parse(text: &str) -> Result<Self, Box<dyn Error>> {
        let parts: Vec<_> = text.split('.').collect();
        if parts.len() != 4 {
            return Err(
                "version must be FileVersion 10.0.<build>.<revision> (example: 10.0.26100.4484)"
                    .into(),
            );
        }
        let parse = |part: &str, name: &str| {
            part.parse::<u16>()
                .map_err(|error| format!("invalid {name} '{part}': {error}"))
        };
        let version = Self {
            major: parse(parts[0], "major")?,
            minor: parse(parts[1], "minor")?,
            build: parse(parts[2], "build")?,
            revision: parse(parts[3], "revision")?,
        };
        if version.major != 10 || version.minor != 0 {
            return Err(
                "version must be FileVersion 10.0.<build>.<revision> (example: 10.0.26100.4484)"
                    .into(),
            );
        }
        Ok(version)
    }

    pub fn label(self) -> String {
        format!(
            "{}.{}.{}.{}",
            self.major, self.minor, self.build, self.revision
        )
    }
}

pub struct PeImage {
    pub image: Vec<u8>,
    pub image_base: u64,
    pub codeview: CodeViewInfo,
    pub file_version: FileVersion,
}

impl PeImage {
    pub fn load(path: &Path) -> Result<Self, String> {
        let file_bytes = std::fs::read(path)
            .map_err(|error| format!("failed to read {}: {error}", path.display()))?;
        let pe = PeFile::from_bytes(&file_bytes)
            .map_err(|error| format!("invalid PE {}: {error}", path.display()))?;

        let codeview = read_codeview(&pe)?;
        let file_version = read_file_version(&pe)?;
        let image_base = pe.optional_header().ImageBase;
        let size_of_image = pe.optional_header().SizeOfImage as usize;
        let image = build_mapped_image(&pe, &file_bytes, size_of_image)?;

        Ok(Self {
            image,
            image_base,
            codeview,
            file_version,
        })
    }

    pub fn bytes_at(&self, rva: u32, len: usize) -> Result<&[u8], String> {
        let start = rva as usize;
        let end = start
            .checked_add(len)
            .ok_or_else(|| format!("RVA overflow at {rva:#x}"))?;
        if end > self.image.len() {
            return Err(format!(
                "RVA range {rva:#x}+{len:#x} exceeds SizeOfImage {:#x}",
                self.image.len()
            ));
        }
        Ok(&self.image[start..end])
    }
}

fn read_codeview(pe: &PeFile<'_>) -> Result<CodeViewInfo, String> {
    let debug = pe
        .debug()
        .map_err(|error| format!("failed to read PE debug directory: {error}"))?;
    for entry in debug {
        let Ok(debug_entry) = entry.entry() else {
            continue;
        };
        let Some(CodeView::Cv70 {
            image,
            pdb_file_name,
        }) = debug_entry.as_code_view()
        else {
            continue;
        };
        return Ok(CodeViewInfo {
            guid: guid_to_uuid(image.Signature),
            age: image.Age,
            pdb_file_name: pdb_file_name.to_string(),
        });
    }
    Err("PE is missing RSDS CodeView debug info".into())
}

fn guid_to_uuid(guid: pelite::image::GUID) -> Uuid {
    Uuid::from_fields(guid.Data1, guid.Data2, guid.Data3, &guid.Data4)
}

fn read_file_version(pe: &PeFile<'_>) -> Result<FileVersion, String> {
    let resources = pe
        .resources()
        .map_err(|error| format!("failed to read PE resources: {error}"))?;
    let version_info = resources.version_info().map_err(|error| match error {
        FindError::NotFound => "PE is missing VERSIONINFO".to_string(),
        other => format!("failed to read VERSIONINFO: {other}"),
    })?;
    let fixed = version_info
        .fixed()
        .ok_or_else(|| "VERSIONINFO is missing fixed file info".to_string())?;
    // pelite's Display form is authoritative ("10.0.build.revision").
    parse_file_version(&fixed.dwFileVersion.to_string())
}

fn parse_file_version(text: &str) -> Result<FileVersion, String> {
    let parts: Vec<_> = text.split('.').collect();
    if parts.len() != 4 {
        return Err(format!("unexpected FileVersion string: {text}"));
    }
    let parse = |part: &str| {
        part.parse::<u16>()
            .map_err(|error| format!("invalid FileVersion component '{part}': {error}"))
    };
    Ok(FileVersion {
        major: parse(parts[0])?,
        minor: parse(parts[1])?,
        build: parse(parts[2])?,
        revision: parse(parts[3])?,
    })
}

fn build_mapped_image(
    pe: &PeFile<'_>,
    file_bytes: &[u8],
    size_of_image: usize,
) -> Result<Vec<u8>, String> {
    let mut image = vec![0u8; size_of_image];
    let header_size = (pe.optional_header().SizeOfHeaders as usize).min(file_bytes.len());
    if header_size > size_of_image {
        return Err("SizeOfHeaders exceeds SizeOfImage".into());
    }
    image[..header_size].copy_from_slice(&file_bytes[..header_size]);

    for section in pe.section_headers() {
        let va = section.VirtualAddress as usize;
        let raw = section.PointerToRawData as usize;
        let raw_size = section.SizeOfRawData as usize;
        if raw == 0 || raw_size == 0 || raw >= file_bytes.len() {
            continue;
        }
        let available = file_bytes.len() - raw;
        let copy_len = raw_size.min(available);
        let end = match va.checked_add(copy_len) {
            Some(end) if end <= image.len() => end,
            _ => {
                let clipped = image.len().saturating_sub(va);
                if clipped == 0 {
                    continue;
                }
                image[va..va + clipped].copy_from_slice(&file_bytes[raw..raw + clipped]);
                continue;
            }
        };
        image[va..end].copy_from_slice(&file_bytes[raw..raw + copy_len]);
    }
    Ok(image)
}

#[cfg(test)]
mod tests {
    use super::{FileVersion, parse_file_version};

    #[test]
    fn parses_dwmcore_file_version_string() {
        let version = parse_file_version("10.0.26100.8737").expect("parse");
        assert_eq!(version.build, 26100);
        assert_eq!(version.revision, 8737);
        assert_eq!(version.label(), "10.0.26100.8737");
    }

    #[test]
    fn parses_full_file_version() {
        let version = FileVersion::parse("10.0.26100.4484").expect("parse");
        assert_eq!(version.major, 10);
        assert_eq!(version.minor, 0);
        assert_eq!(version.build, 26100);
        assert_eq!(version.revision, 4484);
        assert_eq!(version.label(), "10.0.26100.4484");
    }

    #[test]
    fn rejects_short_build_revision_label() {
        assert!(FileVersion::parse("26100.4484").is_err());
    }
}
