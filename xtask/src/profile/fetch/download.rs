use std::error::Error;
use std::fs::OpenOptions;
use std::io::{ErrorKind, Write};
use std::path::Path;

use reqwest::blocking::Client;

pub fn http_client() -> Result<Client, Box<dyn Error>> {
    Ok(Client::builder()
        .user_agent("dwm-lut-rs-xtask/profile-fetch")
        .build()?)
}

pub fn download_bytes(client: &Client, url: &str) -> Result<Vec<u8>, Box<dyn Error>> {
    let response = client
        .get(url)
        .send()
        .map_err(|error| format!("request failed for {url}: {error}"))?;
    let status = response.status();
    if !status.is_success() {
        return Err(format!("download failed for {url}: HTTP {status}").into());
    }
    let bytes = response
        .bytes()
        .map_err(|error| format!("failed to read body from {url}: {error}"))?;
    Ok(bytes.to_vec())
}

pub fn download_to_file(client: &Client, url: &str, path: &Path) -> Result<(), Box<dyn Error>> {
    let bytes = download_bytes(client, url)?;
    write_new_file(path, &bytes)
}

fn write_new_file(path: &Path, bytes: &[u8]) -> Result<(), Box<dyn Error>> {
    let mut file = match OpenOptions::new().write(true).create_new(true).open(path) {
        Ok(file) => file,
        Err(error) if error.kind() == ErrorKind::AlreadyExists => {
            return Err(format!(
                "{} already exists; remove it before fetching",
                path.display()
            )
            .into());
        }
        Err(error) => {
            return Err(format!("failed to create {}: {error}", path.display()).into());
        }
    };
    if let Err(error) = file.write_all(bytes) {
        drop(file);
        let _ = std::fs::remove_file(path);
        return Err(format!("failed to write {}: {error}", path.display()).into());
    }
    Ok(())
}
