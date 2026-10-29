//! Ecovacs cloud authentication and device listing.
//!
//! A port of the login flow in the Python `deebot-client` library: a signed
//! password login, an optional emailed new-device verification, and a token
//! exchange that yields the portal credentials used for authenticated calls.

use std::collections::BTreeMap;
use std::fmt;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use anyhow::{anyhow, bail, Context, Result};
use base64::{engine::general_purpose::STANDARD as B64, Engine};
use md5::{Digest, Md5};
use rsa::pkcs1::DecodeRsaPublicKey;
use rsa::pkcs8::DecodePublicKey;
use rsa::rand_core::OsRng;
use rsa::{Pkcs1v15Encrypt, RsaPublicKey};
use serde_json::{json, Value};

use crate::continents::COUNTRIES_TO_CONTINENTS;

const CLIENT_KEY: &str = "1520391301804";
const CLIENT_SECRET: &str = "6c319b2a5cd3e66e39159c2e28f2fce9";
const AUTH_CLIENT_KEY: &str = "1520391491841";
const AUTH_CLIENT_SECRET: &str = "77ef58ce3afbe337da74aa8c5ab963a9";
const REALM: &str = "ecouser.net";
const PATH_API_USERS_USER: &str = "users/user.do";
const PATH_API_APPSVR_APP: &str = "appsvr/app.do";
const PATH_API_IOT_DEVMANAGER: &str = "iot/devmanager.do";
const USER_AGENT: &str = "Dalvik/2.1.0 (Linux; U; Android 5.1.1; A5010 Build/LMY48Z)";
const PUBLIC_KEY_CONFIG: &str = "PUBLIC.KEY.CONFIG";
const COUNTRY_CHINA: &str = "CN";
const ANDROID_MODEL: &str = "Pixel 7";
const ANDROID_SYSTEM: &str = "Android 14";
const MAX_RETRIES: u32 = 3;

/// Errors Ecovacs reports in the login response that callers react to.
#[derive(Debug)]
pub enum AuthError {
    InvalidAuthentication(String),
    InvalidVerificationCode(String),
    DeviceVerificationRequired(String),
    Failure { code: String, message: String },
}

impl fmt::Display for AuthError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::InvalidAuthentication(msg) => write!(f, "incorrect account or password: {msg}"),
            Self::InvalidVerificationCode(msg) => write!(f, "invalid verification code: {msg}"),
            Self::DeviceVerificationRequired(msg) => {
                write!(f, "device verification required: {msg}")
            }
            Self::Failure { code, message } => write!(f, "login failed, code {code}: {message}"),
        }
    }
}

impl std::error::Error for AuthError {}

/// Region-specific endpoints derived from the account's country.
pub struct Config {
    device_id: String,
    country: String,
    portal_url: String,
    login_url: String,
    auth_code_url: String,
}

impl Config {
    pub fn new(device_id: String, alpha_2_country: &str) -> Self {
        let alpha_2 = alpha_2_country.to_uppercase();
        let continent_postfix = if alpha_2 == COUNTRY_CHINA {
            String::new()
        } else {
            let continent = COUNTRIES_TO_CONTINENTS
                .iter()
                .find(|(code, _)| *code == alpha_2)
                .map_or("ww", |(_, continent)| *continent);
            format!("-{continent}")
        };
        let country = if alpha_2 == "GB" { "UK".to_string() } else { alpha_2.clone() };
        let country_url = country.to_lowercase();
        let tld = if alpha_2 == COUNTRY_CHINA { country_url.as_str() } else { "com" };

        Self {
            portal_url: format!("https://portal{continent_postfix}.ecouser.net"),
            login_url: format!("https://gl-{country_url}-api.ecovacs.{tld}"),
            auth_code_url: format!("https://gl-{country_url}-openapi.ecovacs.{tld}"),
            device_id,
            country,
        }
    }
}

/// Portal credentials used for authenticated API calls.
pub struct Credentials {
    token: String,
    pub user_id: String,
    /// Seconds since the Unix epoch.
    pub expires_at: u64,
    account: AccountCredentials,
}

impl Credentials {
    /// The password-free account token, which can mint new portal credentials.
    pub fn account(&self) -> &AccountCredentials {
        &self.account
    }
}

#[derive(Clone)]
pub struct AccountCredentials {
    pub access_token: String,
    pub user_id: String,
}

/// A device returned by the account's device lists.
pub struct Device {
    pub did: String,
    pub name: String,
    pub nick: Option<String>,
    pub class: String,
    pub company: String,
    resource: String,
}

pub struct Client {
    http: reqwest::Client,
    config: Config,
    account_id: String,
    password_hash: String,
    public_key: Option<RsaPublicKey>,
}

fn md5_hex(text: &str) -> String {
    Md5::digest(text.as_bytes()).iter().map(|b| format!("{b:02x}")).collect()
}

fn now_secs() -> f64 {
    SystemTime::now().duration_since(UNIX_EPOCH).map_or(0.0, |d| d.as_secs_f64())
}

fn value_to_string(value: &Value) -> Option<String> {
    match value {
        Value::String(s) => Some(s.clone()),
        Value::Number(n) => Some(n.to_string()),
        _ => None,
    }
}

/// Accept the key as either X.509 SubjectPublicKeyInfo or a bare PKCS#1 key.
fn parse_public_key(der: &[u8]) -> Result<RsaPublicKey> {
    RsaPublicKey::from_public_key_der(der)
        .or_else(|_| RsaPublicKey::from_pkcs1_der(der))
        .map_err(|e| anyhow!("invalid Ecovacs public key: {e}"))
}

impl Client {
    /// `password` may be omitted when resuming from a saved session token.
    pub fn new(config: Config, account_id: String, password: Option<&str>) -> Result<Self> {
        let http = reqwest::Client::builder().timeout(Duration::from_secs(60)).build()?;
        let password_hash = password.map(md5_hex).unwrap_or_default();
        Ok(Self { http, config, account_id, password_hash, public_key: None })
    }

    /// Log in, going through the emailed new-device verification when Ecovacs asks for it.
    pub async fn authenticate(
        &mut self,
        mut ask_code: impl FnMut() -> Result<String>,
    ) -> Result<Credentials> {
        match self.login().await {
            Err(err) if matches!(err.downcast_ref::<AuthError>(), Some(AuthError::DeviceVerificationRequired(_))) => {
                self.request_device_verification_code().await?;
                let code = ask_code()?;
                self.verify_device(&code).await
            }
            other => other,
        }
    }

    async fn login(&self) -> Result<Credentials> {
        let mut endpoint = "user/login".to_string();
        if self.config.country == COUNTRY_CHINA {
            endpoint.push_str("CheckMobile");
        }
        let params = BTreeMap::from([
            ("account".to_string(), self.account_id.clone()),
            ("password".to_string(), self.password_hash.clone()),
        ]);
        let data = self.call_private_api(&endpoint, params).await?;
        self.complete_login(&data, "invalid login response").await
    }

    async fn request_device_verification_code(&mut self) -> Result<()> {
        let encrypted_email = self.encrypt_account().await?;
        let params = BTreeMap::from([
            ("encryptEmail".to_string(), encrypted_email),
            ("verifyType".to_string(), "EMAIL_VERIFY_DEVICE".to_string()),
            ("supportChar".to_string(), "N".to_string()),
            ("isForce".to_string(), "N".to_string()),
        ]);
        self.call_private_api("user/sendEmailVerifyCode", params).await?;
        Ok(())
    }

    async fn verify_device(&mut self, code: &str) -> Result<Credentials> {
        let encrypted_account = self.encrypt_account().await?;
        let params = BTreeMap::from([
            ("encryptAccount".to_string(), encrypted_account),
            ("backUpEmail".to_string(), String::new()),
            ("verifyCode".to_string(), code.trim().to_string()),
            ("model".to_string(), ANDROID_MODEL.to_string()),
            ("system".to_string(), ANDROID_SYSTEM.to_string()),
        ]);
        let data = self.call_private_api("user/verifyDevice", params).await?;
        self.complete_login(&data, "invalid verifyDevice response").await
    }

    async fn complete_login(&self, data: &Value, error: &str) -> Result<Credentials> {
        let account = data
            .as_object()
            .and_then(|obj| {
                Some(AccountCredentials {
                    access_token: value_to_string(obj.get("accessToken")?)?,
                    user_id: value_to_string(obj.get("uid")?)?,
                })
            })
            .ok_or_else(|| anyhow!("{error}"))?;
        self.login_with_account(&account).await
    }

    /// Mint portal credentials from the account's access token.
    pub async fn login_with_account(&self, account: &AccountCredentials) -> Result<Credentials> {
        let auth_code = self.call_auth_api(account).await?;
        let resp = self.call_login_by_it_token(&account.user_id, &auth_code).await?;

        let user_id = resp.get("userId").and_then(value_to_string).unwrap_or_else(|| account.user_id.clone());
        let token = resp.get("token").and_then(value_to_string).context("login response has no token")?;
        // "last" is the validity in milliseconds (usually 7 days); expire at 99% of it
        let validity_ms = resp.get("last").and_then(value_to_string).and_then(|s| s.parse::<f64>().ok()).unwrap_or(604_800.0);
        let expires_at = (now_secs() + validity_ms / 1000.0 * 0.99) as u64;
        Ok(Credentials { token, user_id, expires_at, account: account.clone() })
    }

    fn meta(&self) -> BTreeMap<String, String> {
        BTreeMap::from([
            ("lang".to_string(), "EN".to_string()),
            ("appCode".to_string(), "global_e".to_string()),
            ("appVersion".to_string(), "3.14.0".to_string()),
            ("channel".to_string(), "google_play".to_string()),
            ("deviceType".to_string(), "1".to_string()),
            ("country".to_string(), self.config.country.to_lowercase()),
            ("deviceId".to_string(), self.config.device_id.clone()),
        ])
    }

    /// Add `authSign` and `authAppkey`. The signed text covers `additional` too,
    /// but only `params` are sent.
    fn sign(params: &mut BTreeMap<String, String>, additional: &BTreeMap<String, String>, key: &str, secret: &str) {
        let mut sign_data = additional.clone();
        sign_data.extend(params.clone());
        let mut text = key.to_string();
        for (k, v) in &sign_data {
            text.push_str(&format!("{k}={v}"));
        }
        text.push_str(secret);
        params.insert("authSign".to_string(), md5_hex(&text));
        params.insert("authAppkey".to_string(), key.to_string());
    }

    async fn do_auth_request(&self, url: &str, params: &BTreeMap<String, String>) -> Result<Value> {
        let body = self.http.get(url).query(params).send().await?.error_for_status()?.text().await?;
        // Ecovacs returns JSON but labels it as text
        let json: Value = serde_json::from_str(&body).context("invalid authentication response")?;
        let code = json.get("code").and_then(value_to_string).unwrap_or_default();
        let message = json.get("msg").and_then(Value::as_str).unwrap_or("").to_string();
        match code.as_str() {
            "0000" => Ok(json.get("data").cloned().unwrap_or(Value::Null)),
            "1005" | "1010" => Err(AuthError::InvalidAuthentication(message).into()),
            "1012" => Err(AuthError::InvalidVerificationCode(message).into()),
            "1013" => Err(AuthError::DeviceVerificationRequired(message).into()),
            _ => Err(AuthError::Failure { code, message }.into()),
        }
    }

    async fn call_private_api(&self, endpoint: &str, mut params: BTreeMap<String, String>) -> Result<Value> {
        let meta = self.meta();
        let url = format!(
            "{}/v1/private/{}/{}/{}/{}/{}/{}/{}/{}",
            self.config.login_url,
            meta["country"], meta["lang"], meta["deviceId"], meta["appCode"],
            meta["appVersion"], meta["channel"], meta["deviceType"], endpoint
        );
        let now = now_secs();
        params.insert("requestId".to_string(), md5_hex(&now.to_string()));
        params.insert("authTimespan".to_string(), ((now * 1000.0) as u64).to_string());
        params.insert("authTimeZone".to_string(), "GMT-8".to_string());
        Self::sign(&mut params, &meta, CLIENT_KEY, CLIENT_SECRET);
        self.do_auth_request(&url, &params).await
    }

    async fn call_auth_api(&self, account: &AccountCredentials) -> Result<String> {
        let mut params = BTreeMap::from([
            ("uid".to_string(), account.user_id.clone()),
            ("accessToken".to_string(), account.access_token.clone()),
            ("bizType".to_string(), "ECOVACS_IOT".to_string()),
            ("deviceId".to_string(), self.config.device_id.clone()),
            ("authTimespan".to_string(), ((now_secs() * 1000.0) as u64).to_string()),
        ]);
        let additional = BTreeMap::from([("openId".to_string(), "global".to_string())]);
        Self::sign(&mut params, &additional, AUTH_CLIENT_KEY, AUTH_CLIENT_SECRET);
        let url = format!("{}/v1/global/auth/getAuthCode", self.config.auth_code_url);
        let data = self.do_auth_request(&url, &params).await?;
        data.get("authCode").and_then(value_to_string).context("invalid auth code response")
    }

    async fn call_login_by_it_token(&self, user_id: &str, auth_code: &str) -> Result<Value> {
        let china = self.config.country == COUNTRY_CHINA;
        let body = json!({
            "edition": "ECOGLOBLE",
            "userId": user_id,
            "token": auth_code,
            "realm": REALM,
            "resource": self.config.device_id,
            "org": if china { "ECOCN" } else { "ECOWW" },
            "last": "",
            "country": if china { "Chinese" } else { self.config.country.as_str() },
            "todo": "loginByItToken",
        });
        for _ in 0..MAX_RETRIES {
            let resp = self.post(PATH_API_USERS_USER, body.clone(), None).await?;
            let error = resp.get("error").and_then(Value::as_str).unwrap_or("");
            match resp.get("result").and_then(Value::as_str) {
                Some("ok") => return Ok(resp),
                // Ecovacs occasionally fails to set the token; the call is safe to repeat
                Some("fail") if error == "set token error." => continue,
                _ => {
                    let errno = resp.get("errno").and_then(value_to_string).unwrap_or_default();
                    bail!("login by token failed: {error} ({errno})");
                }
            }
        }
        bail!("failed to login with token")
    }

    async fn post(&self, path: &str, body: Value, credentials: Option<&Credentials>) -> Result<Value> {
        self.post_with(path, body, credentials, &[], None).await
    }

    async fn post_with(
        &self,
        path: &str,
        mut body: Value,
        credentials: Option<&Credentials>,
        query: &[(&str, &str)],
        user_agent: Option<&str>,
    ) -> Result<Value> {
        let url = format!("{}/api/{path}", self.config.portal_url);
        if let Some(creds) = credentials {
            body["auth"] = json!({
                "with": "users",
                "userid": creds.user_id,
                "realm": REALM,
                "token": creds.token,
                "resource": self.config.device_id,
            });
        }
        for _ in 0..MAX_RETRIES {
            let mut request = self.http.post(&url).query(query).json(&body);
            if let Some(user_agent) = user_agent {
                request = request.header(reqwest::header::USER_AGENT, user_agent);
            }
            let res = request.send().await?;
            if res.status() == reqwest::StatusCode::BAD_GATEWAY {
                // The Ecovacs API is known to be unreliable; wait and retry
                tokio::time::sleep(Duration::from_secs(10)).await;
                continue;
            }
            return Ok(res.error_for_status()?.json().await?);
        }
        bail!("Ecovacs API kept returning 502")
    }

    async fn public_key(&mut self) -> Result<&RsaPublicKey> {
        if self.public_key.is_none() {
            let params = BTreeMap::from([("keys".to_string(), PUBLIC_KEY_CONFIG.to_string())]);
            let response = self.call_private_api("common/getConfig", params).await?;
            let value = response
                .as_array()
                .and_then(|entries| {
                    entries.iter().find(|e| e.get("key").and_then(Value::as_str) == Some(PUBLIC_KEY_CONFIG))
                })
                .and_then(|entry| entry.get("value").and_then(Value::as_str))
                .context("Ecovacs public key configuration is missing")?;
            let config: Value = serde_json::from_str(value).context("invalid Ecovacs public key")?;
            let der = B64
                .decode(config["publicKey"].as_str().context("invalid Ecovacs public key")?)
                .context("invalid Ecovacs public key encoding")?;
            self.public_key = Some(parse_public_key(&der)?);
        }
        Ok(self.public_key.as_ref().expect("set above"))
    }

    async fn encrypt_account(&mut self) -> Result<String> {
        let account = self.account_id.clone();
        let key = self.public_key().await?;
        // PKCS1v15 padding is required by the Ecovacs API
        let encrypted = key.encrypt(&mut OsRng, Pkcs1v15Encrypt, account.as_bytes())?;
        Ok(B64.encode(encrypted))
    }

    /// Send a command to a robot through the Ecovacs cloud and return the robot's response.
    pub async fn send_command(
        &self,
        credentials: &Credentials,
        device: &Device,
        cmd_name: &str,
        args: Option<Value>,
    ) -> Result<Value> {
        let mut payload = json!({
            "header": { "pri": "1", "ts": now_secs(), "tzm": 480, "ver": "0.0.50" },
        });
        if let Some(args) = args {
            payload["body"] = json!({ "data": args });
        }
        let body = json!({
            "cmdName": cmd_name,
            "payload": payload,
            "payloadType": "j",
            "td": "q",
            "toId": device.did,
            "toRes": device.resource,
            "toType": device.class,
        });
        let query = [
            ("mid", device.class.as_str()),
            ("did", device.did.as_str()),
            ("td", "q"),
            ("u", credentials.user_id.as_str()),
            ("cv", "1.67.3"),
            ("t", "a"),
            ("av", "1.3.1"),
        ];
        let response = self
            .post_with(PATH_API_IOT_DEVMANAGER, body, Some(credentials), &query, Some(USER_AGENT))
            .await?;

        if response.get("ret").and_then(Value::as_str) == Some("ok") {
            return Ok(response.get("resp").cloned().unwrap_or(response));
        }
        match response.get("errno").and_then(Value::as_i64) {
            Some(4200) => bail!("the robot is offline"),
            Some(500) => bail!("the robot did not respond (it may be asleep or out of Wi-Fi range)"),
            _ => bail!("command {cmd_name} failed: {response}"),
        }
    }

    /// List the account's devices from both the regular and global device lists.
    pub async fn get_devices(&self, credentials: &Credentials) -> Result<Vec<Device>> {
        let mut devices: BTreeMap<String, Device> = BTreeMap::new();
        for (path, todo) in [(PATH_API_USERS_USER, "GetDeviceList"), (PATH_API_APPSVR_APP, "GetGlobalDeviceList")] {
            let body = json!({ "userid": credentials.user_id, "todo": todo });
            let resp = self.post(path, body, Some(credentials)).await?;
            let Some(list) = resp.get("devices").and_then(Value::as_array) else { continue };
            for item in list {
                let field = |name: &str| item.get(name).and_then(Value::as_str).map(str::to_string);
                let Some(did) = field("did") else { continue };
                devices.insert(
                    did.clone(),
                    Device {
                        did,
                        name: field("name").unwrap_or_default(),
                        nick: field("nick"),
                        class: field("class").unwrap_or_default(),
                        company: field("company").unwrap_or_default(),
                        resource: field("resource").unwrap_or_default(),
                    },
                );
            }
        }
        Ok(devices.into_values().collect())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use rsa::pkcs1::EncodeRsaPublicKey;
    use rsa::pkcs8::EncodePublicKey;
    use rsa::RsaPrivateKey;

    fn test_client() -> Client {
        Client::new(Config::new("abc123".to_string(), "us"), "user@example.com".to_string(), Some("x")).unwrap()
    }

    #[test]
    fn md5_matches_known_vector() {
        assert_eq!(md5_hex("hello world"), "5eb63bbbe01eeed093cb22bb8f5acdc3");
    }

    #[test]
    fn signature_matches_python_deebot_client() {
        let mut params = BTreeMap::from([
            ("account".to_string(), "user@example.com".to_string()),
            ("password".to_string(), "5f4dcc3b5aa765d61d8327deb882cf99".to_string()),
            ("requestId".to_string(), "rid".to_string()),
            ("authTimespan".to_string(), "1700000000000".to_string()),
            ("authTimeZone".to_string(), "GMT-8".to_string()),
        ]);
        Client::sign(&mut params, &test_client().meta(), CLIENT_KEY, CLIENT_SECRET);
        // Reference value computed by deebot-client's own signing code
        assert_eq!(params["authSign"], "1cb76d56fa8ec050686bdb26eabd6698");
        assert_eq!(params["authAppkey"], CLIENT_KEY);
    }

    #[test]
    fn region_endpoints() {
        let us = Config::new("id".to_string(), "us");
        assert_eq!(us.portal_url, "https://portal-na.ecouser.net");
        assert_eq!(us.login_url, "https://gl-us-api.ecovacs.com");
        assert_eq!(us.auth_code_url, "https://gl-us-openapi.ecovacs.com");
        let gb = Config::new("id".to_string(), "GB");
        assert_eq!(gb.country, "UK");
        assert_eq!(gb.login_url, "https://gl-uk-api.ecovacs.com");
        let cn = Config::new("id".to_string(), "CN");
        assert_eq!(cn.portal_url, "https://portal.ecouser.net");
        assert_eq!(cn.login_url, "https://gl-cn-api.ecovacs.cn");
    }

    #[test]
    fn rsa_encryption_round_trips_for_both_key_formats() {
        let private = RsaPrivateKey::new(&mut OsRng, 2048).unwrap();
        let public = RsaPublicKey::from(&private);
        let spki = public.to_public_key_der().unwrap();
        let pkcs1 = public.to_pkcs1_der().unwrap();
        for der in [spki.as_bytes(), pkcs1.as_bytes()] {
            let key = parse_public_key(der).unwrap();
            let encrypted = key.encrypt(&mut OsRng, Pkcs1v15Encrypt, b"user@example.com").unwrap();
            assert_eq!(private.decrypt(Pkcs1v15Encrypt, &encrypted).unwrap(), b"user@example.com");
        }
    }
}
