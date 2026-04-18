//! Allowlist configuration at `~/.config/dkdc-io/imessage/access.toml`.
//!
//! The allowlist decides:
//! - which chat GUIDs can be replied to (must resolve through allowlisted
//!   handles or `self.chat_id`), and
//! - which inbound messages `list_messages` / `read_message` will surface.
//!
//! Empty allowlist = fail closed. The server still starts, but every tool
//! returns an error pointing at this file.

use std::fs;
use std::io::Write;
use std::os::unix::fs::{OpenOptionsExt, PermissionsExt};
use std::path::PathBuf;

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};

/// Loaded access config. Handles are lowercased on load for case-insensitive
/// matching.
#[derive(Debug, Clone, Default)]
pub struct Access {
    /// The owner's preferred chat GUID for self-chat (e.g. `iMessage;-;+15551234567`).
    /// When set, `reply` with no `chat_id` defaults here.
    pub self_chat_id: Option<String>,
    /// Handles that belong to the owner. Messages from these handles are
    /// treated as self (skip allowlist gate on the read path).
    pub self_handles: Vec<String>,
    /// Other handles (people, groups) allowed to interact with the LLM.
    pub allow_from: Vec<String>,
}

impl Access {
    pub fn is_empty(&self) -> bool {
        self.self_chat_id.is_none() && self.self_handles.is_empty() && self.allow_from.is_empty()
    }

    pub fn all_handles_lower(&self) -> Vec<String> {
        let mut out: Vec<String> = self
            .allow_from
            .iter()
            .chain(self.self_handles.iter())
            .map(|h| h.to_lowercase())
            .collect();
        out.sort();
        out.dedup();
        out
    }
}

#[derive(Debug, Default, Serialize, Deserialize)]
struct AccessToml {
    #[serde(default, rename = "self", skip_serializing_if = "Option::is_none")]
    self_: Option<SelfToml>,
    #[serde(default)]
    allow_from: Vec<String>,
}

#[derive(Debug, Default, Serialize, Deserialize)]
struct SelfToml {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    chat_id: Option<String>,
    #[serde(default)]
    handles: Vec<String>,
}

pub fn state_dir() -> Result<PathBuf> {
    if let Ok(p) = std::env::var("DKDC_IO_STATE_DIR") {
        return Ok(PathBuf::from(p));
    }
    if let Ok(xdg) = std::env::var("XDG_CONFIG_HOME")
        && !xdg.is_empty()
    {
        return Ok(PathBuf::from(xdg).join("dkdc-io").join("imessage"));
    }
    let home = dirs::home_dir().context("no home dir available")?;
    Ok(home.join(".config").join("dkdc-io").join("imessage"))
}

pub fn access_file() -> Result<PathBuf> {
    if let Ok(p) = std::env::var("DKDC_IO_ACCESS_FILE") {
        return Ok(PathBuf::from(p));
    }
    Ok(state_dir()?.join("access.toml"))
}

pub fn load() -> Access {
    let path = match access_file() {
        Ok(p) => p,
        Err(_) => return Access::default(),
    };
    let raw = match fs::read_to_string(&path) {
        Ok(s) => s,
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => return Access::default(),
        Err(err) => {
            tracing::warn!(path = %path.display(), error = %err, "access.toml unreadable");
            return Access::default();
        }
    };
    parse(&raw).unwrap_or_else(|err| {
        tracing::warn!(path = %path.display(), error = %err, "access.toml corrupt; using defaults");
        Access::default()
    })
}

pub fn parse(raw: &str) -> Result<Access> {
    let parsed: AccessToml = toml::from_str(raw).context("parse access.toml")?;
    let (self_chat_id, self_handles) = match parsed.self_ {
        Some(s) => (s.chat_id, s.handles),
        None => (None, Vec::new()),
    };
    Ok(Access {
        self_chat_id,
        self_handles: self_handles
            .into_iter()
            .map(|h| normalize_handle(&h))
            .collect(),
        allow_from: parsed
            .allow_from
            .into_iter()
            .map(|h| normalize_handle(&h))
            .collect(),
    })
}

pub fn save(a: &Access) -> Result<()> {
    let dir = state_dir()?;
    fs::create_dir_all(&dir).with_context(|| format!("create {}", dir.display()))?;
    fs::set_permissions(&dir, fs::Permissions::from_mode(0o700)).ok();

    let path = access_file()?;
    let tmp = path.with_extension("toml.tmp");
    let out = AccessToml {
        self_: match (&a.self_chat_id, &a.self_handles) {
            (None, h) if h.is_empty() => None,
            (cid, handles) => Some(SelfToml {
                chat_id: cid.clone(),
                handles: handles.clone(),
            }),
        },
        allow_from: a.allow_from.clone(),
    };
    let body = toml::to_string_pretty(&out).context("serialize access.toml")?;
    {
        let mut f = fs::OpenOptions::new()
            .write(true)
            .create(true)
            .truncate(true)
            .mode(0o600)
            .open(&tmp)
            .with_context(|| format!("open {}", tmp.display()))?;
        f.write_all(body.as_bytes())?;
        f.flush()?;
    }
    fs::rename(&tmp, &path).with_context(|| format!("rename -> {}", path.display()))?;
    Ok(())
}

pub fn normalize_handle(h: &str) -> String {
    h.trim().to_lowercase()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_empty_toml() {
        let a = parse("").expect("empty toml parses");
        assert!(a.is_empty());
    }

    #[test]
    fn parse_full_toml_lowercases() {
        let a = parse(
            r#"
allow_from = ["A@B.com", "+15551234567"]
[self]
chat_id = "iMessage;-;+15550000000"
handles = ["Self@X.com"]
"#,
        )
        .expect("parse");
        assert_eq!(a.allow_from, vec!["a@b.com", "+15551234567"]);
        assert_eq!(a.self_handles, vec!["self@x.com"]);
        assert_eq!(a.self_chat_id.as_deref(), Some("iMessage;-;+15550000000"));
    }

    #[test]
    fn roundtrip_save_load() {
        let tmp = tempfile::tempdir().unwrap();
        // Scope env vars to this test.
        let _state = scoped_env("DKDC_IO_STATE_DIR", tmp.path().to_str().unwrap());
        let _file = scoped_env("DKDC_IO_ACCESS_FILE", "");
        let a = Access {
            self_chat_id: Some("iMessage;-;+15550000000".to_string()),
            self_handles: vec!["me@x.com".to_string()],
            allow_from: vec!["them@x.com".to_string()],
        };
        save(&a).unwrap();
        let b = load();
        assert_eq!(b.allow_from, a.allow_from);
        assert_eq!(b.self_handles, a.self_handles);
        assert_eq!(b.self_chat_id, a.self_chat_id);
    }

    struct ScopedEnv {
        key: &'static str,
        prior: Option<String>,
    }
    impl Drop for ScopedEnv {
        fn drop(&mut self) {
            match &self.prior {
                Some(v) => unsafe { std::env::set_var(self.key, v) },
                None => unsafe { std::env::remove_var(self.key) },
            }
        }
    }
    fn scoped_env(key: &'static str, val: &str) -> ScopedEnv {
        let prior = std::env::var(key).ok();
        if val.is_empty() {
            unsafe { std::env::remove_var(key) };
        } else {
            unsafe { std::env::set_var(key, val) };
        }
        ScopedEnv { key, prior }
    }
}
