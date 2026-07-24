use std::error::Error;
use std::io::Read;

use flate2::read::GzDecoder;
use reqwest::blocking::Client;
use serde::Deserialize;
use serde_json::Value;

use crate::profile::pe::FileVersion;

use super::download::download_bytes;

const WINBINDEX_DWMCORE_INDEX_URL: &str =
    "https://winbindex.m417z.com/data/by_filename_compressed/dwmcore.dll.json.gz";
const MSDL_SYMBOLS_BASE: &str = "https://msdl.microsoft.com/download/symbols";
const MACHINE_AMD64: u32 = 0x8664;

#[derive(Debug, Clone)]
pub struct WinbindexPeCandidate {
    pub sha256: String,
    pub timestamp: u32,
    pub virtual_size: u32,
    pub version: String,
}

impl WinbindexPeCandidate {
    pub fn pe_url(&self) -> String {
        pe_symbol_url(self.timestamp, self.virtual_size)
    }
}

#[derive(Debug, Deserialize)]
struct IndexEntry {
    #[serde(rename = "fileInfo")]
    file_info: Option<FileInfo>,
}

#[derive(Debug, Deserialize)]
struct FileInfo {
    version: Option<String>,
    #[serde(rename = "machineType")]
    machine_type: Option<u32>,
    timestamp: Option<u32>,
    #[serde(rename = "virtualSize")]
    virtual_size: Option<u32>,
}

pub fn resolve_amd64_pe(
    client: &Client,
    version: &FileVersion,
) -> Result<WinbindexPeCandidate, Box<dyn Error>> {
    let compressed = download_bytes(client, WINBINDEX_DWMCORE_INDEX_URL)?;
    let mut decoder = GzDecoder::new(compressed.as_slice());
    let mut json = String::new();
    decoder
        .read_to_string(&mut json)
        .map_err(|error| format!("failed to decompress Winbindex index: {error}"))?;

    let root: Value = serde_json::from_str(&json)
        .map_err(|error| format!("failed to parse Winbindex index JSON: {error}"))?;
    let object = root
        .as_object()
        .ok_or("Winbindex index root must be a JSON object")?;

    let mut matches = Vec::new();
    for (sha256, value) in object {
        let entry: IndexEntry = serde_json::from_value(value.clone())
            .map_err(|error| format!("invalid Winbindex entry {sha256}: {error}"))?;
        let Some(info) = entry.file_info else {
            continue;
        };
        if info.machine_type != Some(MACHINE_AMD64) {
            continue;
        }
        let Some(version_text) = info.version.as_deref() else {
            continue;
        };
        if !file_version_matches(version_text, version) {
            continue;
        }
        let (Some(timestamp), Some(virtual_size)) = (info.timestamp, info.virtual_size) else {
            continue;
        };
        matches.push(WinbindexPeCandidate {
            sha256: sha256.clone(),
            timestamp,
            virtual_size,
            version: version_text.to_string(),
        });
    }

    match matches.len() {
        0 => Err(format!(
            "no amd64 Winbindex entry for dwmcore.dll FileVersion {}",
            version.label()
        )
        .into()),
        1 => Ok(matches.remove(0)),
        _ => {
            let mut lines = vec![format!(
                "ambiguous amd64 Winbindex entries for dwmcore.dll FileVersion {}:",
                version.label()
            )];
            for candidate in &matches {
                lines.push(format!(
                    "  sha256={} timestamp={:#x} virtual_size={:#x} version={}",
                    candidate.sha256,
                    candidate.timestamp,
                    candidate.virtual_size,
                    candidate.version
                ));
            }
            Err(lines.join("\n").into())
        }
    }
}

pub fn pe_symbol_url(timestamp: u32, virtual_size: u32) -> String {
    format!("{MSDL_SYMBOLS_BASE}/dwmcore.dll/{timestamp:08X}{virtual_size:x}/dwmcore.dll")
}

pub fn file_version_matches(raw: &str, version: &FileVersion) -> bool {
    let expected = version.label();
    let Some(rest) = raw.strip_prefix(&expected) else {
        return false;
    };
    rest.is_empty() || rest.starts_with(' ') || rest.starts_with('(')
}

#[cfg(test)]
mod tests {
    use crate::profile::pe::FileVersion;

    use super::{file_version_matches, pe_symbol_url};

    #[test]
    fn matches_winbindex_version_strings() {
        let version = FileVersion::parse("10.0.26100.4484").expect("parse");
        assert!(file_version_matches(
            "10.0.26100.4484 (WinBuild.160101.0800)",
            &version
        ));
        assert!(file_version_matches("10.0.26100.4484", &version));
        assert!(!file_version_matches(
            "10.0.26100.44840 (WinBuild.160101.0800)",
            &version
        ));
        assert!(!file_version_matches(
            "10.0.26100.1 (WinBuild.160101.0800)",
            &version
        ));
    }

    #[test]
    fn builds_pe_symbol_url() {
        assert_eq!(
            pe_symbol_url(0x76d2b289, 0x44e000),
            "https://msdl.microsoft.com/download/symbols/dwmcore.dll/76D2B28944e000/dwmcore.dll"
        );
    }
}
