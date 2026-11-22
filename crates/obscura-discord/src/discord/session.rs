use std::fs::{self, OpenOptions};
use std::io::Write;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use uuid::Uuid;

use super::headers::DEFAULT_SESSION_PATH;

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct SessionStore {
    #[serde(default)]
    pub token: String,
    #[serde(default)]
    pub user_id: String,
    #[serde(default)]
    pub username: String,
}

impl SessionStore {
    pub fn load(path: &Path) -> Option<Self> {
        let bytes = fs::read(path).ok()?;
        serde_json::from_slice(&bytes).ok()
    }

    pub fn save(&self, path: &Path) -> std::io::Result<()> {
        if let Some(parent) = path.parent() {
            if !parent.as_os_str().is_empty() {
                fs::create_dir_all(parent)?;
            }
        }
        let tmp = path.with_extension(format!(
            "json.tmp.{}.{}",
            std::process::id(),
            Uuid::new_v4()
        ));
        let data = serde_json::to_vec_pretty(self)
            .map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidData, e))?;
        let write_result = (|| {
            let mut options = OpenOptions::new();
            options.create(true).truncate(true).write(true);
            #[cfg(unix)]
            {
                use std::os::unix::fs::OpenOptionsExt;
                options.mode(0o600);
            }
            let mut file = options.open(&tmp)?;
            file.write_all(&data)?;
            file.sync_all()?;
            Ok::<_, std::io::Error>(())
        })();
        if let Err(error) = write_result {
            let _ = fs::remove_file(&tmp);
            return Err(error);
        }
        fs::rename(&tmp, path)?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let _ = fs::set_permissions(path, fs::Permissions::from_mode(0o600));
        }
        Ok(())
    }
}

pub fn resolve_session_path(raw: Option<&str>) -> PathBuf {
    let s = raw
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .unwrap_or(DEFAULT_SESSION_PATH);
    PathBuf::from(s)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::{SystemTime, UNIX_EPOCH};

    #[test]
    fn session_roundtrip() {
        let dir = std::env::temp_dir().join(format!(
            "discord-pruner-sess-{}",
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        let _ = fs::create_dir_all(&dir);
        let path = dir.join("session.json");
        let store = SessionStore {
            token: "mfa.test.token.value.here.123456".into(),
            user_id: "12345".into(),
            username: "tester".into(),
        };
        store.save(&path).unwrap();
        let loaded = SessionStore::load(&path).unwrap();
        assert_eq!(loaded.user_id, "12345");
        assert_eq!(loaded.username, "tester");
        let _ = fs::remove_dir_all(&dir);
    }
}
