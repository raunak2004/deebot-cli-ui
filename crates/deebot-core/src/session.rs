//! Logging in, and the saved session that lets later runs skip the password.

use std::fs;
use std::io::Write;
use std::os::unix::fs::{OpenOptionsExt, PermissionsExt};
use std::path::PathBuf;
use std::sync::{Arc, OnceLock};

use anyhow::{Context, Result};
use rsa::rand_core::{OsRng, RngCore};
use serde_json::{json, Value};

use crate::ecovacs::{AccountCredentials, Client, Config, Credentials, Device};
use crate::robot::Robot;

/// A logged-in Ecovacs account.
pub struct Login {
    pub client: Client,
    pub credentials: Credentials,
    pub country: String,
}

impl Login {
    /// The devices on the account.
    pub async fn devices(&self) -> Result<Vec<Device>> {
        self.client.get_devices(&self.credentials).await
    }

    /// Pick a robot (by id, name or nickname; default the first supported one).
    pub async fn robot(self: &Arc<Self>, selector: Option<&str>) -> Result<Robot> {
        let device = pick_device(self.devices().await?, selector)?;
        Ok(Robot::new(Arc::clone(self), device))
    }
}

/// Only robots on Ecovacs's newer cloud protocol are supported.
pub fn pick_device(devices: Vec<Device>, selector: Option<&str>) -> Result<Device> {
    let mut candidates = devices.into_iter().filter(|d| d.company == "eco-ng");
    match selector {
        None => candidates.next().context("no supported robot on this account (see `devices`)"),
        Some(selector) => candidates
            .find(|d| d.did == selector || d.name == selector || d.nick.as_deref() == Some(selector))
            .with_context(|| format!("no supported robot matching {selector:?} (see `devices`)")),
    }
}

/// A saved login: the password-free account token, never the password itself.
pub struct SavedSession {
    pub email: String,
    pub country: String,
    pub account: AccountCredentials,
}

/// Where the session and client id are kept, when the platform has no usable home directory.
static DATA_DIR: OnceLock<PathBuf> = OnceLock::new();

/// Store the session in `dir` instead of `~/.config/deebot-cli` (Android has no home directory).
/// Call before anything else; the first call wins.
pub fn set_data_dir(dir: PathBuf) {
    let _ = DATA_DIR.set(dir);
}

fn config_dir() -> Result<PathBuf> {
    if let Some(dir) = DATA_DIR.get() {
        return Ok(dir.clone());
    }
    Ok(dirs::home_dir().context("cannot find the home directory")?.join(".config/deebot-cli"))
}

fn session_path() -> Result<PathBuf> {
    Ok(config_dir()?.join("session.json"))
}

pub fn load_session() -> Option<SavedSession> {
    let text = fs::read_to_string(session_path().ok()?).ok()?;
    let json: Value = serde_json::from_str(&text).ok()?;
    let field = |name: &str| json.get(name).and_then(Value::as_str).map(str::to_string);
    Some(SavedSession {
        email: field("email")?,
        country: field("country")?,
        account: AccountCredentials { access_token: field("access_token")?, user_id: field("user_id")? },
    })
}

/// Write the session readable by the owner only; it grants access to the account.
pub fn save_session(email: &str, country: &str, account: &AccountCredentials) -> Result<()> {
    let path = session_path()?;
    fs::create_dir_all(path.parent().expect("path has a parent"))?;
    let body = json!({
        "email": email,
        "country": country,
        "access_token": account.access_token,
        "user_id": account.user_id,
    });
    let mut file = fs::OpenOptions::new().write(true).create(true).truncate(true).mode(0o600).open(&path)?;
    file.set_permissions(fs::Permissions::from_mode(0o600))?;
    file.write_all(body.to_string().as_bytes())?;
    Ok(())
}

/// Delete the saved session; returns whether there was one.
pub fn delete_session() -> Result<bool> {
    match fs::remove_file(session_path()?) {
        Ok(()) => Ok(true),
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => Ok(false),
        Err(err) => Err(err.into()),
    }
}

/// A stable client id; Ecovacs ties its new-device verification to it.
fn load_device_id() -> Result<String> {
    let path = config_dir()?.join("device_id");
    if let Ok(existing) = fs::read_to_string(&path) {
        let existing = existing.trim();
        if !existing.is_empty() {
            return Ok(existing.to_string());
        }
    }
    let mut bytes = [0u8; 16];
    OsRng.fill_bytes(&mut bytes);
    let device_id: String = bytes.iter().map(|b| format!("{b:02x}")).collect();
    fs::create_dir_all(path.parent().expect("path has a parent"))?;
    fs::write(&path, &device_id)?;
    Ok(device_id)
}

/// Who is logging in and from which region, combining what the caller gave with the saved session.
pub struct Identity {
    /// None when neither the caller nor a saved session supplied an email
    pub email: Option<String>,
    pub country: String,
    /// A saved session for this email and country, if one exists
    pub saved: Option<SavedSession>,
}

pub fn resolve_identity(email: Option<&str>, country: Option<&str>) -> Identity {
    let saved = load_session();
    let email = match (email, &saved) {
        (Some(email), _) if !email.is_empty() => Some(email.to_string()),
        (_, Some(saved)) => Some(saved.email.clone()),
        _ => None,
    };
    let saved = saved.filter(|s| Some(&s.email) == email.as_ref());
    let country = country
        .map(str::to_string)
        .or_else(|| saved.as_ref().map(|s| s.country.clone()))
        .unwrap_or_else(|| "US".to_string())
        .to_uppercase();
    let saved = saved.filter(|s| s.country == country);
    Identity { email, country, saved }
}

/// Log in with a saved session token, without the password.
pub async fn resume(email: &str, country: &str, saved: &SavedSession) -> Result<Login> {
    let client = Client::new(Config::new(load_device_id()?, country), email.to_string(), None)?;
    let credentials = client.login_with_account(&saved.account).await?;
    Ok(Login { client, credentials, country: country.to_string() })
}

/// Log in with the password and save the session. `ask_code` is called if Ecovacs asks to verify a
/// new device with an emailed code.
pub async fn password_login(
    email: &str,
    country: &str,
    password: &str,
    ask_code: impl FnMut() -> Result<String>,
) -> Result<Login> {
    let mut client = Client::new(Config::new(load_device_id()?, country), email.to_string(), Some(password))?;
    let credentials = client.authenticate(ask_code).await?;
    if let Err(err) = save_session(email, country, credentials.account()) {
        eprintln!("warning: could not save the session, you'll be asked for the password next time: {err}");
    }
    Ok(Login { client, credentials, country: country.to_string() })
}
