use serde::{de::DeserializeOwned, Serialize};
use std::fs;
use std::path::{Path, PathBuf};

pub fn load_or_default<T: DeserializeOwned + Default>(path: &Path) -> T {
    fs::read_to_string(path)
        .ok()
        .and_then(|raw| serde_json::from_str(&raw).ok())
        .unwrap_or_default()
}

pub fn save_json<T: Serialize>(path: &Path, value: &T, provider: &str) -> Result<(), String> {
    let parent = path
        .parent()
        .ok_or_else(|| format!("Invalid {provider} credential path"))?;
    fs::create_dir_all(parent)
        .map_err(|e| format!("Failed to prepare {provider} credential storage: {e}"))?;
    let json = serde_json::to_vec_pretty(value)
        .map_err(|e| format!("Failed to encode {provider} credentials: {e}"))?;
    let temporary = temporary_path(path);
    fs::write(&temporary, json)
        .map_err(|e| format!("Failed to save {provider} credentials: {e}"))?;
    restrict_file(&temporary)?;
    if path.exists() {
        fs::remove_file(path)
            .map_err(|e| format!("Failed to replace {provider} credentials: {e}"))?;
    }
    fs::rename(&temporary, path)
        .map_err(|e| format!("Failed to finish saving {provider} credentials: {e}"))?;
    Ok(())
}

fn temporary_path(path: &Path) -> PathBuf {
    let mut temporary = path.as_os_str().to_os_string();
    temporary.push(".tmp");
    PathBuf::from(temporary)
}

#[cfg(unix)]
fn restrict_file(path: &Path) -> Result<(), String> {
    use std::os::unix::fs::PermissionsExt;
    fs::set_permissions(path, fs::Permissions::from_mode(0o600))
        .map_err(|e| format!("Failed to protect credential file: {e}"))
}

#[cfg(not(unix))]
fn restrict_file(_path: &Path) -> Result<(), String> {
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde::{Deserialize, Serialize};

    #[derive(Default, Deserialize, Serialize, PartialEq, Debug)]
    struct Row {
        value: String,
    }

    #[test]
    fn round_trips_json_atomically() {
        let path = std::env::temp_dir().join(format!("theta-storage-{}.json", std::process::id()));
        let row = Row {
            value: "secret".into(),
        };
        save_json(&path, &row, "test").unwrap();
        assert_eq!(load_or_default::<Row>(&path), row);
        let _ = fs::remove_file(path);
    }
}
