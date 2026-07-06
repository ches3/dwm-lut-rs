use std::fs;
use std::path::Path;

use sha2::{Digest, Sha256};

use crate::error::InjectorError;

pub(crate) fn current_build_hash() -> Result<String, InjectorError> {
    let exe_path = std::env::current_exe().map_err(|source| InjectorError::ControlPipe {
        operation: "resolve current executable",
        source,
    })?;
    file_build_hash(&exe_path)
}

pub(crate) fn file_build_hash(path: &Path) -> Result<String, InjectorError> {
    let bytes = fs::read(path).map_err(|source| InjectorError::ControlPipe {
        operation: "read executable image",
        source,
    })?;

    Ok(build_hash_from_bytes(&bytes))
}

fn build_hash_from_bytes(bytes: &[u8]) -> String {
    let hash = Sha256::digest(bytes);
    let mut output = String::with_capacity(hash.len() * 2);
    for byte in hash {
        output.push_str(&format!("{byte:02x}"));
    }
    output
}

#[cfg(test)]
mod tests {
    use super::build_hash_from_bytes;

    #[test]
    fn build_hash_is_sha256_hex() {
        assert_eq!(
            build_hash_from_bytes(b"dwm-lut"),
            "b9cad318f2296d332101d2895a1cd4d32d771499229494a8c53aee84dde68d5f"
        );
    }
}
