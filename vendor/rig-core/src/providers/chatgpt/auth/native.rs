//! Native ChatGPT OAuth and token cache implementation.

use super::{AuthContext, AuthError, DeviceCodeHandler, DeviceCodePrompt};
use crate::providers::internal::auth::{
    AuthEntryToken, AuthFileType, ExpectedAuthEntry, FileIdentity, RepairKind, UnsafeEntryKind,
};
use base64::Engine;
use base64::engine::general_purpose::URL_SAFE_NO_PAD as BASE64_URL_SAFE_NO_PAD;
use fs2::FileExt;
use serde::{Deserialize, Deserializer, Serialize};
use std::fs::{File, OpenOptions};
use std::io::{self, Write};
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant, SystemTime};

const CHATGPT_AUTH_BASE: &str = "https://auth.openai.com";
const CHATGPT_OAUTH_TOKEN_URL: &str = "https://auth.openai.com/oauth/token";
const CHATGPT_CLIENT_ID: &str = "app_EMoamEEZ73f0CkXaXp7hrann";
const TOKEN_EXPIRY_SKEW_SECONDS: i64 = 60;
const DEVICE_CODE_TIMEOUT_SECONDS: i64 = 15 * 60;
const DEVICE_CODE_POLL_SLEEP_SECONDS: u64 = 5;
const LOCK_WAIT_BUDGET: Duration = Duration::from_millis(150);
const LOCK_RETRY: Duration = Duration::from_millis(15);
const REPAIR_DETAIL_MAX: usize = 200;

/// Redacted account view after an auth transaction.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AuthorizedAccount {
    pub account_id: Option<String>,
    pub refreshed: bool,
    pub entry: AuthEntryToken,
}

/// Device-flow completion payload (nonempty account id).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LoginCompletion {
    pub account_id: String,
    pub entry: AuthEntryToken,
}

/// Metadata-only stat of the auth-file entry.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AuthFileStat {
    pub exists: bool,
    pub token: Option<AuthEntryToken>,
}

/// Cross-process lock held around one auth-file transaction.
pub struct AuthFileGuard {
    auth_file: PathBuf,
    _lock: File,
}

impl std::fmt::Debug for AuthFileGuard {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("AuthFileGuard").finish_non_exhaustive()
    }
}

impl AuthFileGuard {
    /// Acquire `<auth_file>.lock` with a short bounded wait.
    pub fn acquire(auth_file: impl AsRef<Path>) -> Result<Self, AuthError> {
        let auth_file = validate_auth_path(auth_file.as_ref())?;
        let lock = acquire_auth_lock(&auth_file)?;
        Ok(Self {
            auth_file,
            _lock: lock,
        })
    }
    pub fn stat(&self) -> Result<AuthFileStat, AuthError> {
        inspect_entry(&self.auth_file)
    }

    /// Delete the entry only when it still matches `token`.
    pub fn delete_if_unchanged(&self, token: &AuthEntryToken) -> Result<(), AuthError> {
        delete_if_unchanged(&self.auth_file, token)
    }

    /// Read/refresh under the already-held lock.
    pub async fn authorize_account(&self) -> Result<AuthorizedAccount, AuthError> {
        authorize_under_lock(&self.auth_file, None, false).await
    }

    fn path(&self) -> &Path {
        &self.auth_file
    }
}

#[derive(Debug, Clone)]
pub(super) struct PlatformAuthenticator {
    auth_file: Option<PathBuf>,
    device_code_handler: DeviceCodeHandler,
    allow_device_flow: bool,
    auth_base_url: Option<String>,
}

#[derive(Clone, Deserialize, Serialize, Default)]
struct AuthRecord {
    access_token: Option<String>,
    refresh_token: Option<String>,
    id_token: Option<String>,
    expires_at: Option<i64>,
    account_id: Option<String>,
}

impl std::fmt::Debug for AuthRecord {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("AuthRecord")
            .field("account_id", &self.account_id)
            .field("expires_at", &self.expires_at)
            .finish_non_exhaustive()
    }
}

#[derive(Deserialize)]
struct DeviceCodeResponse {
    device_auth_id: String,
    #[serde(alias = "usercode")]
    user_code: String,
    #[serde(default, deserialize_with = "deserialize_optional_u64")]
    interval: Option<u64>,
}

impl std::fmt::Debug for DeviceCodeResponse {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("DeviceCodeResponse")
            .field("interval", &self.interval)
            .finish_non_exhaustive()
    }
}

#[derive(Deserialize)]
struct DeviceTokenResponse {
    authorization_code: String,
    code_verifier: String,
}

impl std::fmt::Debug for DeviceTokenResponse {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("DeviceTokenResponse(<redacted>)")
    }
}

#[derive(Deserialize)]
struct OAuthTokenResponse {
    access_token: String,
    refresh_token: Option<String>,
    id_token: Option<String>,
}

impl std::fmt::Debug for OAuthTokenResponse {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("OAuthTokenResponse(<redacted>)")
    }
}

#[derive(Debug, Deserialize)]
struct OAuthErrorResponse {
    error: Option<String>,
    error_description: Option<String>,
}

enum RefreshTokensError {
    Reauthenticate,
    Auth(AuthError),
}

impl PlatformAuthenticator {
    pub(super) fn new(
        auth_file: Option<PathBuf>,
        device_code_handler: DeviceCodeHandler,
        allow_device_flow: bool,
        auth_base_url: Option<String>,
    ) -> Self {
        Self {
            auth_file,
            device_code_handler,
            allow_device_flow,
            auth_base_url,
        }
    }
    pub(super) async fn auth_context_oauth(&self) -> Result<AuthContext, AuthError> {
        let Some(path) = &self.auth_file else {
            return Err(AuthError::DeviceFlowDisabled);
        };

        {
            let guard = AuthFileGuard::acquire(path)?;
            match authorize_under_lock(guard.path(), None, false).await {
                Ok(authorized) => {
                    let record = read_auth_record_present(guard.path())?;
                    let access_token =
                        record.access_token.ok_or(AuthError::DeviceFlowDisabled)?;
                    return Ok(AuthContext {
                        access_token,
                        account_id: authorized.account_id,
                    });
                }
                Err(AuthError::DeviceFlowDisabled) if self.allow_device_flow => {}
                Err(err) => return Err(err),
            }
        }

        self.login_device_flow(path).await
    }
    async fn login_device_flow(&self, path: &Path) -> Result<AuthContext, AuthError> {
        let record =
            poll_device_flow(&self.device_code_handler, self.auth_base_url.as_deref()).await?;
        let guard = AuthFileGuard::acquire(path)?;
        write_auth_record(guard.path(), &record, &ExpectedAuthEntry::Absent)?;
        let access_token = record.access_token.ok_or(AuthError::DeviceFlowDisabled)?;
        Ok(AuthContext {
            access_token,
            account_id: record.account_id,
        })
    }
}


async fn authorize_under_lock(
    path: &Path,
    expected_account: Option<&str>,
    allow_device_flow: bool,
) -> Result<AuthorizedAccount, AuthError> {
    let before = inspect_entry(path)?;
    if !before.exists {
        return Err(AuthError::DeviceFlowDisabled);
    }
    let entry = before.token.clone().ok_or(AuthError::UnsafeAuthFile {
        kind: UnsafeEntryKind::NonRegular,
        entry: None,
    })?;

    let mut record = match read_auth_record_present(path) {
        Ok(record) => record,
        Err(AuthError::Json(err)) => {
            return Err(repair_required(
                RepairKind::Corrupt,
                err.to_string(),
                entry,
            ));
        }
        Err(AuthError::Io(err)) => {
            return Err(repair_required(
                RepairKind::Unreadable,
                err.to_string(),
                entry,
            ));
        }
        Err(err) => return Err(err),
    };

    if let Some(access_token) = record.access_token.clone()
        && !token_expired(record.expires_at)
    {
        let account_id = record
            .account_id
            .clone()
            .or_else(|| extract_account_id(record.id_token.as_deref()))
            .or_else(|| extract_account_id(Some(&access_token)));
        if account_id != record.account_id {
            record.account_id = account_id.clone();
            write_auth_record(path, &record, &ExpectedAuthEntry::Present(entry.clone()))?;
        }
        let entry = inspect_required_entry(path)?;
        enforce_expected(expected_account, account_id.as_deref(), entry.clone())?;
        return Ok(AuthorizedAccount {
            account_id,
            refreshed: false,
            entry,
        });
    }

    if let Some(refresh_token) = record.refresh_token.clone() {
        match refresh_tokens(&refresh_token).await {
            Ok(refreshed) => {
                write_auth_record(
                    path,
                    &refreshed,
                    &ExpectedAuthEntry::Present(entry.clone()),
                )?;
                let entry = inspect_required_entry(path)?;
                let account_id = refreshed.account_id.clone();
                enforce_expected(expected_account, account_id.as_deref(), entry.clone())?;
                return Ok(AuthorizedAccount {
                    account_id,
                    refreshed: true,
                    entry,
                });
            }
            Err(RefreshTokensError::Reauthenticate) => {
                return Err(AuthError::DeviceFlowDisabled);
            }
            Err(RefreshTokensError::Auth(err)) => return Err(err),
        }
    }
    let _ = allow_device_flow;
    Err(AuthError::DeviceFlowDisabled)
}

fn enforce_expected(
    expected_account: Option<&str>,
    actual: Option<&str>,
    entry: AuthEntryToken,
) -> Result<(), AuthError> {
    let Some(expected) = expected_account else {
        return Ok(());
    };
    if actual == Some(expected) {
        return Ok(());
    }
    Err(AuthError::AccountMismatch {
        expected: Some(expected.to_owned()),
        actual: actual.map(ToOwned::to_owned),
        entry,
    })
}

fn repair_required(kind: RepairKind, detail: String, entry: AuthEntryToken) -> AuthError {
    AuthError::RepairRequired {
        kind,
        detail: truncate_detail(detail),
        entry,
    }
}

fn truncate_detail(detail: String) -> String {
    let mut detail: String = detail.chars().take(REPAIR_DETAIL_MAX).collect();
    if detail.len() > REPAIR_DETAIL_MAX {
        detail.truncate(REPAIR_DETAIL_MAX);
    }
    detail
}

fn lock_path(auth_file: &Path) -> PathBuf {
    let mut path = auth_file.as_os_str().to_os_string();
    path.push(".lock");
    PathBuf::from(path)
}

fn acquire_auth_lock(auth_file: &Path) -> Result<File, AuthError> {
    if let Some(parent) = auth_file.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let path = lock_path(auth_file);
    if path
        .symlink_metadata()
        .map(|meta| meta.file_type().is_symlink() || !meta.file_type().is_file())
        .unwrap_or(false)
        && path.exists()
    {
        let meta = path.symlink_metadata()?;
        if meta.file_type().is_symlink() || !meta.file_type().is_file() {
            return Err(AuthError::UnsafeLockFile);
        }
    }
    let file = open_nofollow_create(&path)?;
    let meta = file.metadata()?;
    if !meta.is_file() {
        return Err(AuthError::UnsafeLockFile);
    }
    let deadline = Instant::now() + LOCK_WAIT_BUDGET;
    loop {
        match file.try_lock_exclusive() {
            Ok(()) => return Ok(file),
            Err(err) if err.kind() == io::ErrorKind::WouldBlock => {
                if Instant::now() >= deadline {
                    return Err(AuthError::AuthBusy);
                }
                std::thread::sleep(LOCK_RETRY);
            }
            Err(err) => return Err(err.into()),
        }
    }
}

fn open_nofollow_create(path: &Path) -> Result<File, AuthError> {
    let mut options = OpenOptions::new();
    options.read(true).write(true).create(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.custom_flags(libc_o_nofollow());
        options.mode(0o600);
    }
    match options.open(path) {
        Ok(file) => Ok(file),
        Err(err) if is_symlink_loop(&err) => Err(AuthError::UnsafeLockFile),
        Err(err) => Err(err.into()),
    }
}

fn open_nofollow_read(path: &Path) -> Result<File, io::Error> {
    let mut options = OpenOptions::new();
    options.read(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.custom_flags(libc_o_nofollow());
    }
    options.open(path)
}

#[cfg(unix)]
fn libc_o_nofollow() -> i32 {
    libc::O_NOFOLLOW
}

#[cfg(not(unix))]
fn libc_o_nofollow() -> i32 {
    0
}

fn is_symlink_loop(err: &io::Error) -> bool {
    err.kind() == io::ErrorKind::Other
        || err.raw_os_error() == Some(libc_eloop())
}

#[cfg(unix)]
fn libc_eloop() -> i32 {
    libc::ELOOP
}

#[cfg(not(unix))]
fn libc_eloop() -> i32 {
    0
}

fn inspect_entry(path: &Path) -> Result<AuthFileStat, AuthError> {
    let meta = match path.symlink_metadata() {
        Ok(meta) => meta,
        Err(err) if err.kind() == io::ErrorKind::NotFound => {
            return Ok(AuthFileStat {
                exists: false,
                token: None,
            });
        }
        Err(err) => return Err(err.into()),
    };
    if meta.file_type().is_symlink() {
        let token = token_from_meta(path, &meta, AuthFileType::Symlink);
        return Err(AuthError::UnsafeAuthFile {
            kind: UnsafeEntryKind::Symlink,
            entry: Some(token),
        });
    }
    if !meta.file_type().is_file() {
        return Err(AuthError::UnsafeAuthFile {
            kind: UnsafeEntryKind::NonRegular,
            entry: None,
        });
    }
    tighten_mode(path, &meta)?;
    let meta = path.symlink_metadata()?;
    Ok(AuthFileStat {
        exists: true,
        token: Some(token_from_meta(path, &meta, AuthFileType::Regular)),
    })
}

fn inspect_required_entry(path: &Path) -> Result<AuthEntryToken, AuthError> {
    inspect_entry(path)?
        .token
        .ok_or(AuthError::AuthConflict)
}

fn token_from_meta(
    path: &Path,
    meta: &std::fs::Metadata,
    file_type: AuthFileType,
) -> AuthEntryToken {
    AuthEntryToken {
        resolved_path: resolved_physical(path),
        file_type,
        identity: file_identity(meta),
        mtime: meta.modified().ok(),
        ctime: file_ctime(meta),
        size: meta.len(),
    }
}

fn resolved_physical(path: &Path) -> PathBuf {
    path.parent()
        .and_then(|parent| parent.canonicalize().ok())
        .map(|parent| {
            path.file_name()
                .map(|name| parent.join(name))
                .unwrap_or(parent)
        })
        .unwrap_or_else(|| path.to_path_buf())
}

fn validate_auth_path(path: &Path) -> Result<PathBuf, AuthError> {
    let parent = path.parent().ok_or_else(|| {
        AuthError::Message("ChatGPT auth file path has no parent directory".into())
    })?;
    std::fs::create_dir_all(parent)?;
    let parent = parent.canonicalize()?;
    let file_name = path.file_name().ok_or_else(|| {
        AuthError::Message("ChatGPT auth file path has no file name".into())
    })?;
    let resolved = parent.join(file_name);
    match inspect_entry(&resolved) {
        Ok(_) | Err(AuthError::UnsafeAuthFile { .. }) => Ok(resolved),
        Err(err) => Err(err),
    }
}

fn file_identity(meta: &std::fs::Metadata) -> FileIdentity {
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        FileIdentity {
            device: meta.dev(),
            inode: meta.ino(),
        }
    }
    #[cfg(not(unix))]
    {
        FileIdentity {
            device: 0,
            inode: meta.len(),
        }
    }
}

fn file_ctime(meta: &std::fs::Metadata) -> Option<SystemTime> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        let secs = u64::try_from(meta.ctime()).ok()?;
        let nsecs = u32::try_from(meta.ctime_nsec()).ok()?;
        SystemTime::UNIX_EPOCH.checked_add(Duration::new(secs, nsecs))
    }
    #[cfg(not(unix))]
    {
        meta.modified().ok()
    }
}

fn tighten_mode(path: &Path, meta: &std::fs::Metadata) -> Result<(), AuthError> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::{MetadataExt, PermissionsExt};
        let mode = meta.mode() & 0o777;
        if mode != 0o600 {
            let file = open_nofollow_read(path)?;
            let mut perms = file.metadata()?.permissions();
            perms.set_mode(0o600);
            file.set_permissions(perms)?;
        }
    }
    let _ = path;
    let _ = meta;
    Ok(())
}

fn entries_match(left: &AuthEntryToken, right: &AuthEntryToken) -> bool {
    left.resolved_path == right.resolved_path
        && left.file_type == right.file_type
        && left.identity == right.identity
        && left.mtime == right.mtime
        && left.ctime == right.ctime
        && left.size == right.size
}

fn write_auth_record(
    path: &Path,
    record: &AuthRecord,
    expected: &ExpectedAuthEntry,
) -> Result<(), AuthError> {
    let current = inspect_entry(path)?;
    match expected {
        ExpectedAuthEntry::Absent if current.exists => return Err(AuthError::AuthConflict),
        ExpectedAuthEntry::Present(token) => {
            let Some(current_token) = &current.token else {
                return Err(AuthError::AuthConflict);
            };
            if !entries_match(current_token, token) {
                return Err(AuthError::AuthConflict);
            }
        }
        ExpectedAuthEntry::Absent => {}
    }

    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let tmp = path.with_extension("json.tmp");
    {
        let mut options = OpenOptions::new();
        options.write(true).create(true).truncate(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            options.mode(0o600);
        }
        let mut file = options.open(&tmp)?;
        file.write_all(&serde_json::to_vec_pretty(record)?)?;
        file.sync_all()?;
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mut perms = std::fs::metadata(&tmp)?.permissions();
        perms.set_mode(0o600);
        std::fs::set_permissions(&tmp, perms)?;
    }
    std::fs::rename(&tmp, path)?;
    if let Some(parent) = path.parent() {
        if let Ok(dir) = File::open(parent) {
            let _ = dir.sync_all();
        }
    }
    Ok(())
}

fn read_auth_record_present(path: &Path) -> Result<AuthRecord, AuthError> {
    let file = match open_nofollow_read(path) {
        Ok(file) => file,
        Err(err) if err.kind() == io::ErrorKind::NotFound => {
            return Err(AuthError::AuthConflict);
        }
        Err(err) => return Err(err.into()),
    };
    serde_json::from_reader(file).map_err(AuthError::from)
}

fn delete_if_unchanged(path: &Path, token: &AuthEntryToken) -> Result<(), AuthError> {
    let current = inspect_entry(path)?;
    let Some(current_token) = current.token else {
        return Err(AuthError::AuthConflict);
    };
    if !entries_match(&current_token, token) {
        return Err(AuthError::AuthConflict);
    }
    match std::fs::remove_file(path) {
        Ok(()) => Ok(()),
        Err(err) if err.kind() == io::ErrorKind::NotFound => Err(AuthError::AuthConflict),
        Err(err) => Err(err.into()),
    }
}

fn emit_device_code_prompt(handler: &DeviceCodeHandler, prompt: DeviceCodePrompt) {
    if let Some(callback) = &handler.0 {
        callback(prompt);
    } else {
        println!(
            "Sign in with ChatGPT:\n1) Visit {}\n2) Enter code: {}\nDo not share this device code.",
            prompt.verification_uri, prompt.user_code
        );
    }
}

fn build_auth_record(
    tokens: OAuthTokenResponse,
    previous_refresh_token: Option<String>,
) -> AuthRecord {
    let access_token = Some(tokens.access_token);
    let id_token = tokens.id_token;
    AuthRecord {
        expires_at: access_token
            .as_deref()
            .and_then(extract_expiration_timestamp),
        account_id: extract_account_id(id_token.as_deref()).or_else(|| {
            access_token
                .as_deref()
                .and_then(|token| extract_account_id(Some(token)))
        }),
        access_token,
        refresh_token: tokens.refresh_token.or(previous_refresh_token),
        id_token,
    }
}

fn extract_expiration_timestamp(token: &str) -> Option<i64> {
    decode_jwt_claims(token)
        .get("exp")
        .and_then(|value| value.as_i64().or_else(|| value.as_u64().map(|v| v as i64)))
}

fn extract_account_id(token: Option<&str>) -> Option<String> {
    let claims = decode_jwt_claims(token?);
    claims
        .get("https://api.openai.com/auth")
        .and_then(|value| value.as_object())
        .and_then(|map| map.get("chatgpt_account_id"))
        .and_then(|value| value.as_str())
        .map(ToOwned::to_owned)
}

fn decode_jwt_claims(token: &str) -> serde_json::Value {
    let payload = token.split('.').nth(1).unwrap_or_default();
    let decoded = BASE64_URL_SAFE_NO_PAD.decode(payload.as_bytes());
    decoded
        .ok()
        .and_then(|bytes| serde_json::from_slice::<serde_json::Value>(&bytes).ok())
        .unwrap_or(serde_json::Value::Null)
}

fn should_reauthenticate_after_refresh(
    status: reqwest::StatusCode,
    error_code: Option<&str>,
) -> bool {
    matches!(
        status,
        reqwest::StatusCode::BAD_REQUEST | reqwest::StatusCode::UNAUTHORIZED
    ) && matches!(error_code, Some("invalid_grant"))
}

fn format_refresh_error(
    status: reqwest::StatusCode,
    oauth_error: Option<&OAuthErrorResponse>,
    body: &str,
) -> String {
    let error_code = oauth_error.and_then(|error| error.error.as_deref());
    let description = oauth_error.and_then(|error| error.error_description.as_deref());

    if let Some(description) = description
        .map(str::trim)
        .filter(|description| !description.is_empty())
    {
        return format!(
            "ChatGPT token refresh failed: {status} {} ({description})",
            error_code.unwrap_or("unknown_error")
        );
    }

    if let Some(error_code) = error_code {
        return format!("ChatGPT token refresh failed: {status} {error_code}");
    }

    if !body.trim().is_empty() {
        return format!("ChatGPT token refresh failed: {status} {body}");
    }

    format!("ChatGPT token refresh failed: {status}")
}

fn token_expired(expires_at: Option<i64>) -> bool {
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|duration| duration.as_secs() as i64)
        .unwrap_or_default();

    match expires_at {
        Some(exp) => now >= exp - TOKEN_EXPIRY_SKEW_SECONDS,
        None => true,
    }
}

fn auth_base(base: Option<&str>) -> String {
    base.unwrap_or(CHATGPT_AUTH_BASE)
        .trim_end_matches('/')
        .to_string()
}

async fn poll_device_flow(
    handler: &DeviceCodeHandler,
    auth_base_url: Option<&str>,
) -> Result<AuthRecord, AuthError> {
    let base = auth_base(auth_base_url);
    let device_code_url = format!("{base}/api/accounts/deviceauth/usercode");
    let device_token_url = format!("{base}/api/accounts/deviceauth/token");
    let oauth_token_url = format!("{base}/oauth/token");
    let verify_url = format!("{base}/codex/device");
    let client = reqwest::Client::new();
    let device = client
        .post(device_code_url)
        .json(&serde_json::json!({ "client_id": CHATGPT_CLIENT_ID }))
        .send()
        .await?
        .error_for_status()?
        .json::<DeviceCodeResponse>()
        .await?;

    emit_device_code_prompt(
        handler,
        DeviceCodePrompt {
            verification_uri: verify_url,
            user_code: device.user_code.clone(),
        },
    );

    let interval = device.interval.unwrap_or(DEVICE_CODE_POLL_SLEEP_SECONDS);
    let start = Instant::now();
    let code = loop {
        if start.elapsed().as_secs() as i64 >= DEVICE_CODE_TIMEOUT_SECONDS {
            return Err(AuthError::Message(
                "Timed out waiting for ChatGPT device authorization".into(),
            ));
        }

        let response = client
            .post(&device_token_url)
            .json(&serde_json::json!({
                "device_auth_id": device.device_auth_id,
                "user_code": device.user_code,
            }))
            .send()
            .await?;

        if response.status().is_success() {
            break response.json::<DeviceTokenResponse>().await?;
        }

        let status = response.status();
        if status.as_u16() == 403 || status.as_u16() == 404 {
            tokio::time::sleep(Duration::from_secs(interval)).await;
            continue;
        }

        let text = response.text().await.unwrap_or_default();
        return Err(AuthError::Message(format!(
            "ChatGPT device authorization failed: {status} {text}"
        )));
    };

    let redirect_uri = format!("{base}/deviceauth/callback");
    let form = [
        ("grant_type", "authorization_code"),
        ("code", code.authorization_code.as_str()),
        ("redirect_uri", redirect_uri.as_str()),
        ("client_id", CHATGPT_CLIENT_ID),
        ("code_verifier", code.code_verifier.as_str()),
    ];
    let body = url::form_urlencoded::Serializer::new(String::new())
        .extend_pairs(form)
        .finish();

    let tokens = client
        .post(oauth_token_url)
        .header(
            reqwest::header::CONTENT_TYPE,
            "application/x-www-form-urlencoded",
        )
        .body(body)
        .send()
        .await?
        .error_for_status()?
        .json::<OAuthTokenResponse>()
        .await?;

    Ok(build_auth_record(tokens, None))
}


async fn refresh_tokens(refresh_token: &str) -> Result<AuthRecord, RefreshTokensError> {
    let client = reqwest::Client::new();
    let form = [
        ("client_id", CHATGPT_CLIENT_ID),
        ("grant_type", "refresh_token"),
        ("refresh_token", refresh_token),
        ("scope", "openid profile email"),
    ];

    let body = url::form_urlencoded::Serializer::new(String::new())
        .extend_pairs(form)
        .finish();

    let response = client
        .post(CHATGPT_OAUTH_TOKEN_URL)
        .header(
            reqwest::header::CONTENT_TYPE,
            "application/x-www-form-urlencoded",
        )
        .body(body)
        .send()
        .await
        .map_err(AuthError::from)
        .map_err(RefreshTokensError::Auth)?;

    let status = response.status();
    if status.is_success() {
        let tokens = response
            .json::<OAuthTokenResponse>()
            .await
            .map_err(AuthError::from)
            .map_err(RefreshTokensError::Auth)?;
        return Ok(build_auth_record(tokens, Some(refresh_token.to_owned())));
    }

    let body = response.text().await.unwrap_or_default();
    let oauth_error = serde_json::from_str::<OAuthErrorResponse>(&body).ok();
    if should_reauthenticate_after_refresh(
        status,
        oauth_error
            .as_ref()
            .and_then(|error| error.error.as_deref()),
    ) {
        return Err(RefreshTokensError::Reauthenticate);
    }

    Err(RefreshTokensError::Auth(AuthError::Message(
        format_refresh_error(status, oauth_error.as_ref(), &body),
    )))
}

fn deserialize_optional_u64<'de, D>(deserializer: D) -> Result<Option<u64>, D::Error>
where
    D: Deserializer<'de>,
{
    #[derive(Deserialize)]
    #[serde(untagged)]
    enum U64OrString {
        U64(u64),
        String(String),
    }

    let value = Option::<U64OrString>::deserialize(deserializer)?;
    match value {
        None => Ok(None),
        Some(U64OrString::U64(value)) => Ok(Some(value)),
        Some(U64OrString::String(value)) => {
            let value = value.trim();
            if value.is_empty() {
                Ok(None)
            } else {
                value
                    .parse::<u64>()
                    .map(Some)
                    .map_err(serde::de::Error::custom)
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{
        AuthFileGuard, DeviceCodeHandler, DeviceCodeResponse, ExpectedAuthEntry,
        OAuthErrorResponse, OAuthTokenResponse, PlatformAuthenticator, build_auth_record,
        format_refresh_error, inspect_entry, should_reauthenticate_after_refresh,
        write_auth_record, AuthRecord,
    };
    use crate::providers::internal::auth::AuthError;
    use reqwest::StatusCode;
    use std::fs;
    use std::os::unix::fs::PermissionsExt;

    #[test]
    fn device_code_response_accepts_numeric_interval() {
        let response: DeviceCodeResponse = serde_json::from_str(
            r#"{
                "device_auth_id": "deviceauth_123",
                "user_code": "ABCD-EFGH",
                "interval": 5
            }"#,
        )
        .expect("device code response");

        assert_eq!(response.interval, Some(5));
    }

    #[test]
    fn device_code_response_accepts_string_interval() {
        let response: DeviceCodeResponse = serde_json::from_str(
            r#"{
                "device_auth_id": "deviceauth_123",
                "user_code": "ABCD-EFGH",
                "interval": "5"
            }"#,
        )
        .expect("device code response");

        assert_eq!(response.interval, Some(5));
    }

    #[test]
    fn refresh_reauth_only_on_invalid_grant() {
        assert!(should_reauthenticate_after_refresh(
            StatusCode::BAD_REQUEST,
            Some("invalid_grant")
        ));
        assert!(should_reauthenticate_after_refresh(
            StatusCode::UNAUTHORIZED,
            Some("invalid_grant")
        ));
        assert!(!should_reauthenticate_after_refresh(
            StatusCode::BAD_GATEWAY,
            Some("invalid_grant")
        ));
        assert!(!should_reauthenticate_after_refresh(
            StatusCode::BAD_REQUEST,
            Some("invalid_request")
        ));
        assert!(!should_reauthenticate_after_refresh(
            StatusCode::UNAUTHORIZED,
            None
        ));
    }

    #[tokio::test]
    async fn noninteractive_oauth_requires_sign_in_instead_of_device_flow() {
        let dir = std::env::temp_dir().join(format!(
            "rig-chatgpt-auth-{}",
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .expect("time")
                .as_nanos()
        ));
        fs::create_dir_all(&dir).expect("temp dir");
        let path = dir.join("chatgpt-subscription.json");
        let auth = PlatformAuthenticator::new(
            Some(path),
            DeviceCodeHandler::default(),
            false,
            None,
        );
        let err = auth
            .auth_context_oauth()
            .await
            .expect_err("missing cached auth should not start device flow");
        assert!(matches!(err, AuthError::DeviceFlowDisabled), "{err:?}");
        let _ = fs::remove_dir_all(dir);
    }

    #[test]
    fn refresh_error_uses_oauth_description_when_present() {
        let oauth_error = OAuthErrorResponse {
            error: Some("temporarily_unavailable".into()),
            error_description: Some("please retry".into()),
        };

        assert_eq!(
            format_refresh_error(StatusCode::BAD_GATEWAY, Some(&oauth_error), ""),
            "ChatGPT token refresh failed: 502 Bad Gateway temporarily_unavailable (please retry)"
        );
    }

    #[test]
    fn build_auth_record_preserves_existing_refresh_token_when_refresh_omits_one() {
        let record = build_auth_record(
            OAuthTokenResponse {
                access_token: "access-token".into(),
                refresh_token: None,
                id_token: None,
            },
            Some("cached-refresh-token".into()),
        );

        assert_eq!(
            record.refresh_token.as_deref(),
            Some("cached-refresh-token")
        );
    }

    #[test]
    fn write_then_read_round_trip_is_0600() {
        let dir = std::env::temp_dir().join(format!(
            "rig-chatgpt-write-{}",
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .expect("time")
                .as_nanos()
        ));
        fs::create_dir_all(&dir).expect("temp dir");
        let path = dir.join("chatgpt-subscription.json");
        let record = AuthRecord {
            access_token: Some("access".into()),
            refresh_token: Some("refresh".into()),
            id_token: None,
            expires_at: Some(9_999_999_999),
            account_id: Some("acct_1".into()),
        };
        write_auth_record(&path, &record, &ExpectedAuthEntry::Absent).expect("write");
        let mode = fs::metadata(&path).expect("meta").permissions().mode() & 0o777;
        assert_eq!(mode, 0o600);
        let leftover: Vec<_> = fs::read_dir(&dir)
            .expect("read dir")
            .filter_map(|entry| entry.ok())
            .filter(|entry| {
                entry
                    .path()
                    .extension()
                    .is_some_and(|ext| ext == "tmp")
            })
            .collect();
        assert!(leftover.is_empty(), "{leftover:?}");
        let _guard = AuthFileGuard::acquire(&path).expect("lock");
        let stat = inspect_entry(&path).expect("stat");
        assert!(stat.exists);
        write_auth_record(
            &path,
            &record,
            &ExpectedAuthEntry::Present(stat.token.expect("token")),
        )
        .expect("rewrite");
        let mode = fs::metadata(&path).expect("meta").permissions().mode() & 0o777;
        assert_eq!(mode, 0o600);
        let _ = fs::remove_dir_all(dir);
    }

    #[test]
    fn stale_fence_is_auth_conflict() {
        let dir = std::env::temp_dir().join(format!(
            "rig-chatgpt-fence-{}",
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .expect("time")
                .as_nanos()
        ));
        fs::create_dir_all(&dir).expect("temp dir");
        let path = dir.join("chatgpt-subscription.json");
        let record = AuthRecord {
            access_token: Some("access".into()),
            refresh_token: None,
            id_token: None,
            expires_at: Some(9_999_999_999),
            account_id: Some("acct_1".into()),
        };
        write_auth_record(&path, &record, &ExpectedAuthEntry::Absent).expect("write");
        let first = inspect_entry(&path).expect("stat").token.expect("token");
        write_auth_record(&path, &record, &ExpectedAuthEntry::Present(first.clone()))
            .expect("rewrite");
        let err = write_auth_record(&path, &record, &ExpectedAuthEntry::Present(first))
            .expect_err("stale fence");
        assert!(matches!(err, AuthError::AuthConflict), "{err:?}");
        let _ = fs::remove_dir_all(dir);
    }

    #[test]
    fn authorized_account_has_no_token_fields() {
        let fields: Vec<_> = std::any::type_name::<super::AuthorizedAccount>()
            .split("::")
            .collect();
        assert!(fields.last().is_some_and(|name| *name == "AuthorizedAccount"));
    }

    #[test]
    fn contended_lock_is_auth_busy() {
        let dir = std::env::temp_dir().join(format!(
            "rig-chatgpt-busy-{}",
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .expect("time")
                .as_nanos()
        ));
        fs::create_dir_all(&dir).expect("temp dir");
        let path = dir.join("chatgpt-subscription.json");
        let _held = AuthFileGuard::acquire(&path).expect("first lock");
        let err = AuthFileGuard::acquire(&path).expect_err("second lock");
        assert!(matches!(err, AuthError::AuthBusy), "{err:?}");
        let _ = fs::remove_dir_all(dir);
    }

    #[test]
    fn symlink_lock_entry_is_unsafe_lock_file() {
        let dir = std::env::temp_dir().join(format!(
            "rig-chatgpt-lock-{}",
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .expect("time")
                .as_nanos()
        ));
        fs::create_dir_all(&dir).expect("temp dir");
        let path = dir.join("chatgpt-subscription.json");
        let target = dir.join("elsewhere");
        fs::write(&target, b"x").expect("target");
        std::os::unix::fs::symlink(&target, super::lock_path(&path)).expect("symlink lock");
        let err = AuthFileGuard::acquire(&path).expect_err("unsafe lock");
        assert!(matches!(err, AuthError::UnsafeLockFile), "{err:?}");
        let _ = fs::remove_dir_all(dir);
    }

    #[test]
    fn auth_entry_token_debug_omits_physical_path() {
        let dir = std::env::temp_dir().join(format!(
            "rig-chatgpt-debug-{}",
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .expect("time")
                .as_nanos()
        ));
        fs::create_dir_all(&dir).expect("temp dir");
        let path = dir.join("chatgpt-subscription.json");
        write_auth_record(
            &path,
            &AuthRecord {
                access_token: Some("secret-token".into()),
                refresh_token: None,
                id_token: None,
                expires_at: Some(9_999_999_999),
                account_id: Some("acct_1".into()),
            },
            &ExpectedAuthEntry::Absent,
        )
        .expect("write");
        let token = inspect_entry(&path).expect("stat").token.expect("token");
        let rendered = format!("{token:?}");
        assert!(!rendered.contains("secret-token"), "{rendered}");
        assert!(
            !rendered.contains(path.to_string_lossy().as_ref()),
            "{rendered}"
        );
        let prompt = super::DeviceCodePrompt {
            verification_uri: "https://example.test/device".into(),
            user_code: "ABCD-EFGH".into(),
        };
        let prompt_debug = format!("{prompt:?}");
        assert!(!prompt_debug.contains("ABCD-EFGH"), "{prompt_debug}");
        let _ = fs::remove_dir_all(dir);
    }

    #[test]
    fn directory_lock_entry_is_unsafe_lock_file() {
        let dir = std::env::temp_dir().join(format!(
            "rig-chatgpt-lockdir-{}",
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .expect("time")
                .as_nanos()
        ));
        fs::create_dir_all(&dir).expect("temp dir");
        let path = dir.join("chatgpt-subscription.json");
        fs::create_dir_all(super::lock_path(&path)).expect("lock dir");
        let err = AuthFileGuard::acquire(&path).expect_err("directory lock");
        assert!(matches!(err, AuthError::UnsafeLockFile), "{err:?}");
        let _ = fs::remove_dir_all(dir);
    }

    #[test]
    fn same_size_rewrite_with_restored_mtime_is_auth_conflict() {
        let dir = std::env::temp_dir().join(format!(
            "rig-chatgpt-ctime-{}",
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .expect("time")
                .as_nanos()
        ));
        fs::create_dir_all(&dir).expect("temp dir");
        let path = dir.join("chatgpt-subscription.json");
        let record = AuthRecord {
            access_token: Some("access-token-aaaa".into()),
            refresh_token: None,
            id_token: None,
            expires_at: Some(9_999_999_999),
            account_id: Some("acct_1".into()),
        };
        write_auth_record(&path, &record, &ExpectedAuthEntry::Absent).expect("write");
        let first = inspect_entry(&path).expect("stat").token.expect("token");
        let mtime = fs::metadata(&path).expect("meta").modified().expect("mtime");
        let original = fs::read(&path).expect("read");
        let mut rewritten = original.clone();
        if let Some(byte) = rewritten.last_mut() {
            *byte = byte.wrapping_add(1);
        }
        assert_eq!(rewritten.len(), original.len());
        fs::write(&path, rewritten).expect("in-place rewrite");
        let file = fs::File::open(&path).expect("open");
        file.set_modified(mtime).expect("restore mtime");
        drop(file);
        let err = write_auth_record(&path, &record, &ExpectedAuthEntry::Present(first))
            .expect_err("ctime fence");
        assert!(matches!(err, AuthError::AuthConflict), "{err:?}");
        let _ = fs::remove_dir_all(dir);
    }

    #[tokio::test]
    async fn injected_auth_base_rewrites_device_flow_endpoints() {
        use std::io::{Read, Write};
        use std::net::TcpListener;
        use std::sync::{Arc, Mutex};
        use std::thread;

        let listener = TcpListener::bind("127.0.0.1:0").expect("bind");
        let addr = listener.local_addr().expect("addr");
        let seen = Arc::new(Mutex::new(Vec::<String>::new()));
        let seen_for_server = Arc::clone(&seen);
        thread::spawn(move || {
            for incoming in listener.incoming().take(3) {
                let mut stream = incoming.expect("accept");
                let mut buf = [0_u8; 4096];
                let n = stream.read(&mut buf).unwrap_or(0);
                let request = String::from_utf8_lossy(&buf[..n]);
                let first = request.lines().next().unwrap_or_default().to_string();
                let body = request.split("\r\n\r\n").nth(1).unwrap_or_default().to_string();
                seen_for_server
                    .lock()
                    .expect("lock")
                    .push(format!("{first}\n{body}"));
                let (status, payload) = if first.contains("/api/accounts/deviceauth/usercode") {
                    (
                        "200 OK",
                        r#"{"device_auth_id":"dev1","user_code":"WXYZ-1234","interval":1}"#,
                    )
                } else if first.contains("/api/accounts/deviceauth/token") {
                    (
                        "200 OK",
                        r#"{"authorization_code":"authz","code_verifier":"verifier"}"#,
                    )
                } else if first.contains("/oauth/token") {
                    (
                        "200 OK",
                        r#"{"access_token":"access","refresh_token":"refresh","id_token":null}"#,
                    )
                } else {
                    ("404 Not Found", "{}")
                };
                let response = format!(
                    "HTTP/1.1 {status}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{payload}",
                    payload.len()
                );
                let _ = stream.write_all(response.as_bytes());
            }
        });

        let base = format!("http://{addr}");
        let verify = Arc::new(Mutex::new(None::<String>));
        let verify_for_handler = Arc::clone(&verify);
        let handler = DeviceCodeHandler::new(move |prompt| {
            *verify_for_handler.lock().expect("lock") = Some(prompt.verification_uri);
        });
        let record = super::poll_device_flow(&handler, Some(base.as_str()))
            .await
            .expect("device flow");
        assert_eq!(record.access_token.as_deref(), Some("access"));
        let expected_verify = format!("{base}/codex/device");
        assert_eq!(
            verify.lock().expect("lock").as_deref(),
            Some(expected_verify.as_str())
        );
        let seen = seen.lock().expect("lock");
        assert!(
            seen.iter()
                .any(|entry| entry.contains("/api/accounts/deviceauth/usercode")),
            "{seen:?}"
        );
        assert!(
            seen.iter()
                .any(|entry| entry.contains("/api/accounts/deviceauth/token")),
            "{seen:?}"
        );
        assert!(
            seen.iter().any(|entry| entry.contains("/oauth/token")),
            "{seen:?}"
        );
        let encoded_redirect = format!("{base}/deviceauth/callback").replace(':', "%3A").replace('/', "%2F");
        assert!(
            seen.iter().any(|entry| entry.contains(&encoded_redirect)),
            "{seen:?}"
        );
    }



}
