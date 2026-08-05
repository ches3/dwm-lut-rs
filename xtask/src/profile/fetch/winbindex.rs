use std::error::Error;
use std::io::Read;
use std::sync::{Arc, OnceLock};

use flate2::read::GzDecoder;
use reqwest::blocking::Client;
use serde::Deserialize;
use serde_json::{Map, Value};

use crate::profile::pe::FileVersion;

use super::download::download_bytes;

const WINBINDEX_DWMCORE_INDEX_URL: &str =
    "https://winbindex.m417z.com/data/by_filename_compressed/dwmcore.dll.json.gz";
const MSDL_SYMBOLS_BASE: &str = "https://msdl.microsoft.com/download/symbols";
const MACHINE_AMD64: u32 = 0x8664;

static CACHED_INDEX: OnceLock<Arc<DwmcoreIndex>> = OnceLock::new();

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

#[derive(Debug)]
pub struct DwmcoreIndex {
    entries: Map<String, Value>,
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

struct Amd64IndexHit {
    sha256: String,
    version: String,
    timestamp: u32,
    virtual_size: u32,
}

impl DwmcoreIndex {
    pub fn load(client: &Client) -> Result<Arc<Self>, Box<dyn Error>> {
        if let Some(index) = CACHED_INDEX.get() {
            return Ok(Arc::clone(index));
        }
        let loaded = Arc::new(Self::fetch(client)?);
        match CACHED_INDEX.set(Arc::clone(&loaded)) {
            Ok(()) => Ok(loaded),
            Err(_) => {
                Ok(Arc::clone(CACHED_INDEX.get().expect(
                    "Winbindex index cache must be populated after a raced set",
                )))
            }
        }
    }

    pub fn from_json_str(json: &str) -> Result<Self, Box<dyn Error>> {
        let root: Value = serde_json::from_str(json)
            .map_err(|error| format!("failed to parse Winbindex index JSON: {error}"))?;
        let Value::Object(object) = root else {
            return Err("Winbindex index root must be a JSON object".into());
        };
        Ok(Self { entries: object })
    }

    fn fetch(client: &Client) -> Result<Self, Box<dyn Error>> {
        let compressed = download_bytes(client, WINBINDEX_DWMCORE_INDEX_URL)?;
        let mut decoder = GzDecoder::new(compressed.as_slice());
        let mut json = String::new();
        decoder
            .read_to_string(&mut json)
            .map_err(|error| format!("failed to decompress Winbindex index: {error}"))?;
        Self::from_json_str(&json)
    }

    fn amd64_hits(&self) -> Result<Vec<Amd64IndexHit>, Box<dyn Error>> {
        let mut hits = Vec::new();
        for (sha256, value) in &self.entries {
            let entry: IndexEntry = serde_json::from_value(value.clone())
                .map_err(|error| format!("invalid Winbindex entry {sha256}: {error}"))?;
            let Some(info) = entry.file_info else {
                continue;
            };
            if info.machine_type != Some(MACHINE_AMD64) {
                continue;
            }
            let Some(version) = info.version else {
                continue;
            };
            let (Some(timestamp), Some(virtual_size)) = (info.timestamp, info.virtual_size) else {
                continue;
            };
            hits.push(Amd64IndexHit {
                sha256: sha256.clone(),
                version,
                timestamp,
                virtual_size,
            });
        }
        Ok(hits)
    }

    pub fn latest_amd64_version_for_build(
        &self,
        build: u16,
    ) -> Result<FileVersion, Box<dyn Error>> {
        let mut latest: Option<FileVersion> = None;
        for hit in self.amd64_hits()? {
            let Ok(version) = parse_file_version(&hit.version) else {
                continue;
            };
            if version.build != build {
                continue;
            }
            latest = Some(match latest {
                Some(current) if current >= version => current,
                _ => version,
            });
        }
        latest.ok_or_else(|| {
            format!("no amd64 Winbindex entry for dwmcore.dll build {build} with a FileVersion")
                .into()
        })
    }

    pub fn resolve_amd64(
        &self,
        version: &FileVersion,
    ) -> Result<WinbindexPeCandidate, Box<dyn Error>> {
        let mut matches = Vec::new();
        for hit in self.amd64_hits()? {
            if !file_version_matches(&hit.version, version) {
                continue;
            }
            matches.push(WinbindexPeCandidate {
                sha256: hit.sha256,
                timestamp: hit.timestamp,
                virtual_size: hit.virtual_size,
                version: hit.version,
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
}

pub fn resolve_amd64_pe(
    client: &Client,
    version: &FileVersion,
) -> Result<WinbindexPeCandidate, Box<dyn Error>> {
    DwmcoreIndex::load(client)?.resolve_amd64(version)
}

pub fn resolve_latest_amd64_version_for_build(
    client: &Client,
    build: u16,
) -> Result<FileVersion, Box<dyn Error>> {
    DwmcoreIndex::load(client)?.latest_amd64_version_for_build(build)
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

/// Parse a Winbindex `fileInfo.version` string such as
/// `10.0.26100.4484 (WinBuild.160101.0800)`.
fn parse_file_version(text: &str) -> Result<FileVersion, Box<dyn Error>> {
    let label = version_label(text)
        .ok_or_else(|| format!("invalid Winbindex FileVersion string: {text}"))?;
    FileVersion::parse(label)
}

fn version_label(text: &str) -> Option<&str> {
    let text = text.trim();
    let end = text.find([' ', '(']).unwrap_or(text.len());
    let label = text.get(..end)?.trim_end();
    if label.is_empty() { None } else { Some(label) }
}

#[cfg(test)]
mod tests {
    use crate::profile::pe::FileVersion;

    use super::{DwmcoreIndex, file_version_matches, parse_file_version, pe_symbol_url};

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
    fn parses_winbindex_version_with_suffix() {
        let version = parse_file_version("10.0.26100.4484 (WinBuild.160101.0800)").expect("parse");
        assert_eq!(version.label(), "10.0.26100.4484");
    }

    #[test]
    fn rejects_invalid_winbindex_version() {
        assert!(parse_file_version("not-a-version").is_err());
        assert!(parse_file_version("26100.4484").is_err());
    }

    #[test]
    fn builds_pe_symbol_url() {
        assert_eq!(
            pe_symbol_url(0x76d2b289, 0x44e000),
            "https://msdl.microsoft.com/download/symbols/dwmcore.dll/76D2B28944e000/dwmcore.dll"
        );
    }

    #[test]
    fn latest_amd64_version_for_build_picks_maximum_in_build() {
        let index = DwmcoreIndex::from_json_str(
            r#"{
                "aaa": {
                    "fileInfo": {
                        "version": "10.0.26100.4484 (WinBuild.160101.0800)",
                        "machineType": 34404,
                        "timestamp": 1,
                        "virtualSize": 2
                    }
                },
                "bbb": {
                    "fileInfo": {
                        "version": "10.0.26100.8737",
                        "machineType": 34404,
                        "timestamp": 3,
                        "virtualSize": 4
                    }
                },
                "ccc": {
                    "fileInfo": {
                        "version": "10.0.28000.2525",
                        "machineType": 34404,
                        "timestamp": 5,
                        "virtualSize": 6
                    }
                },
                "ddd": {
                    "fileInfo": {
                        "version": "10.0.26100.9999",
                        "machineType": 332,
                        "timestamp": 7,
                        "virtualSize": 8
                    }
                }
            }"#,
        )
        .expect("parse fixture");
        let latest = index.latest_amd64_version_for_build(26100).expect("latest");
        assert_eq!(latest.label(), "10.0.26100.8737");
    }

    #[test]
    fn latest_amd64_version_for_build_ignores_incomplete_entries() {
        let index = DwmcoreIndex::from_json_str(
            r#"{
                "aaa": {
                    "fileInfo": {
                        "version": "10.0.26100.8737",
                        "machineType": 34404
                    }
                },
                "bbb": {
                    "fileInfo": {
                        "version": "10.0.26100.4484",
                        "machineType": 34404,
                        "timestamp": 1,
                        "virtualSize": 2
                    }
                }
            }"#,
        )
        .expect("parse fixture");
        let latest = index.latest_amd64_version_for_build(26100).expect("latest");
        assert_eq!(latest.label(), "10.0.26100.4484");
    }
}
