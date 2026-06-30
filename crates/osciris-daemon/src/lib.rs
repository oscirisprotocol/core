use std::{
    env, fmt,
    io::Write,
    path::{Path, PathBuf},
    sync::{
        atomic::{AtomicU64, Ordering},
        Arc,
    },
    time::{Duration, Instant},
};

use anyhow::{anyhow, bail, Context, Result};
use atomic_write_file::AtomicWriteFile;
use base64::{engine::general_purpose::URL_SAFE_NO_PAD, Engine};
use futures::{SinkExt, StreamExt};
use rand::{rngs::OsRng, RngCore};
use serde::{Deserialize, Serialize};
use subtle::ConstantTimeEq;
use tokio::{
    io::{AsyncRead, AsyncWrite},
    sync::RwLock,
};
use tokio_util::codec::{Framed, LinesCodec};

pub const API_VERSION: u16 = 1;
pub const MAX_FRAME_BYTES: usize = 64 * 1024;

static NEXT_REQUEST_ID: AtomicU64 = AtomicU64::new(1);

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum LocalEndpoint {
    #[cfg(unix)]
    Unix(PathBuf),
    #[cfg(windows)]
    NamedPipe(String),
}

impl LocalEndpoint {
    pub fn default_for_user() -> Self {
        if let Ok(endpoint) = env::var("OSCIRIS_DAEMON_ENDPOINT") {
            return Self::from_override(endpoint);
        }

        #[cfg(unix)]
        {
            Self::Unix(default_state_dir().join("daemon.sock"))
        }

        #[cfg(windows)]
        {
            let user = env::var("USERNAME")
                .unwrap_or_else(|_| "user".to_string())
                .chars()
                .map(|character| {
                    if character.is_ascii_alphanumeric() {
                        character
                    } else {
                        '-'
                    }
                })
                .collect::<String>();
            Self::NamedPipe(format!(r"\\.\pipe\osciris-node-{user}"))
        }
    }

    pub fn from_override(value: String) -> Self {
        #[cfg(unix)]
        {
            Self::Unix(PathBuf::from(value))
        }

        #[cfg(windows)]
        {
            Self::NamedPipe(value)
        }
    }
}

impl fmt::Display for LocalEndpoint {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            #[cfg(unix)]
            Self::Unix(path) => write!(formatter, "{}", path.display()),
            #[cfg(windows)]
            Self::NamedPipe(name) => formatter.write_str(name),
        }
    }
}

pub fn default_state_dir() -> PathBuf {
    if let Some(path) = env::var_os("OSCIRIS_STATE_DIR") {
        return PathBuf::from(path);
    }

    #[cfg(target_os = "macos")]
    {
        home_dir()
            .join("Library")
            .join("Application Support")
            .join("OSCIRIS")
    }

    #[cfg(target_os = "windows")]
    {
        env::var_os("LOCALAPPDATA")
            .map(PathBuf::from)
            .unwrap_or_else(home_dir)
            .join("OSCIRIS")
    }

    #[cfg(all(unix, not(target_os = "macos")))]
    {
        env::var_os("XDG_STATE_HOME")
            .map(PathBuf::from)
            .unwrap_or_else(|| home_dir().join(".local").join("state"))
            .join("osciris")
    }
}

fn home_dir() -> PathBuf {
    env::var_os("HOME")
        .or_else(|| env::var_os("USERPROFILE"))
        .map(PathBuf::from)
        .unwrap_or_else(env::temp_dir)
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct DaemonRequest {
    pub api_version: u16,
    pub request_id: u64,
    pub auth_token: String,
    pub command: DaemonCommand,
}

impl DaemonRequest {
    pub fn new(auth_token: String, command: DaemonCommand) -> Self {
        Self {
            api_version: API_VERSION,
            request_id: NEXT_REQUEST_ID.fetch_add(1, Ordering::Relaxed),
            auth_token,
            command,
        }
    }
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum DaemonCommand {
    Ping,
    GetStatus,
    SetParticipation { enabled: bool },
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct DaemonResponse {
    pub api_version: u16,
    pub request_id: u64,
    pub result: Option<DaemonResult>,
    pub error: Option<DaemonError>,
}

impl DaemonResponse {
    fn success(request_id: u64, result: DaemonResult) -> Self {
        Self {
            api_version: API_VERSION,
            request_id,
            result: Some(result),
            error: None,
        }
    }

    fn error(request_id: u64, code: &str, message: impl Into<String>) -> Self {
        Self {
            api_version: API_VERSION,
            request_id,
            result: None,
            error: Some(DaemonError {
                code: code.to_string(),
                message: message.into(),
            }),
        }
    }
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(tag = "type", content = "data", rename_all = "snake_case")]
pub enum DaemonResult {
    Pong { daemon_version: String },
    Status(DaemonStatus),
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct DaemonError {
    pub code: String,
    pub message: String,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct DaemonStatus {
    pub api_version: u16,
    pub daemon_version: String,
    pub uptime_seconds: u64,
    pub participation_enabled: bool,
    pub network_state: NetworkState,
    pub active_jobs: u32,
    pub platform: PlatformSummary,
    pub readiness: Option<ReadinessSummary>,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum NetworkState {
    NotConfigured,
    Connecting,
    Online,
    Degraded,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct PlatformSummary {
    pub operating_system: String,
    pub architecture: String,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct ReadinessSummary {
    pub provider_target: u32,
    pub healthy_providers: u32,
    pub provider_gap: u32,
    pub slot_target: u32,
    pub available_slots: u32,
    pub slot_gap: u32,
    pub verifier_target: u32,
    pub online_verifiers: u32,
    pub verifier_gap: u32,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize, PartialEq, Eq)]
struct PersistedState {
    participation_enabled: bool,
}

#[derive(Clone)]
pub struct DaemonService {
    inner: Arc<DaemonServiceInner>,
}

struct DaemonServiceInner {
    started_at: Instant,
    state_dir: PathBuf,
    auth_token: String,
    state: RwLock<PersistedState>,
}

impl DaemonService {
    pub fn new(state_dir: PathBuf) -> Result<Self> {
        secure_state_dir(&state_dir)?;
        let state = load_state(&state_dir)?;
        let auth_token = ensure_auth_token(&state_dir)?;
        Ok(Self {
            inner: Arc::new(DaemonServiceInner {
                started_at: Instant::now(),
                state_dir,
                auth_token,
                state: RwLock::new(state),
            }),
        })
    }

    pub async fn handle(&self, request: DaemonRequest) -> DaemonResponse {
        if request
            .auth_token
            .as_bytes()
            .ct_eq(self.inner.auth_token.as_bytes())
            .unwrap_u8()
            != 1
        {
            return DaemonResponse::error(
                request.request_id,
                "authentication_failed",
                "invalid local daemon credential",
            );
        }
        if request.api_version != API_VERSION {
            return DaemonResponse::error(
                request.request_id,
                "unsupported_api_version",
                format!(
                    "daemon supports API version {API_VERSION}, request used {}",
                    request.api_version
                ),
            );
        }

        match request.command {
            DaemonCommand::Ping => DaemonResponse::success(
                request.request_id,
                DaemonResult::Pong {
                    daemon_version: env!("CARGO_PKG_VERSION").to_string(),
                },
            ),
            DaemonCommand::GetStatus => DaemonResponse::success(
                request.request_id,
                DaemonResult::Status(self.status().await),
            ),
            DaemonCommand::SetParticipation { enabled } => {
                let mut state = self.inner.state.write().await;
                state.participation_enabled = enabled;
                if let Err(error) = persist_state(&self.inner.state_dir, &state) {
                    return DaemonResponse::error(
                        request.request_id,
                        "state_persistence_failed",
                        error.to_string(),
                    );
                }
                drop(state);
                DaemonResponse::success(
                    request.request_id,
                    DaemonResult::Status(self.status().await),
                )
            }
        }
    }

    pub async fn status(&self) -> DaemonStatus {
        let state = self.inner.state.read().await;
        DaemonStatus {
            api_version: API_VERSION,
            daemon_version: env!("CARGO_PKG_VERSION").to_string(),
            uptime_seconds: self.inner.started_at.elapsed().as_secs(),
            participation_enabled: state.participation_enabled,
            network_state: NetworkState::NotConfigured,
            active_jobs: 0,
            platform: PlatformSummary {
                operating_system: env::consts::OS.to_string(),
                architecture: env::consts::ARCH.to_string(),
            },
            readiness: None,
        }
    }

    pub async fn serve(self, endpoint: LocalEndpoint) -> Result<()> {
        match endpoint {
            #[cfg(unix)]
            LocalEndpoint::Unix(path) => self.serve_unix(path).await,
            #[cfg(windows)]
            LocalEndpoint::NamedPipe(name) => self.serve_named_pipe(name).await,
        }
    }

    #[cfg(unix)]
    async fn serve_unix(self, path: PathBuf) -> Result<()> {
        use std::os::unix::fs::PermissionsExt;
        use tokio::net::{UnixListener, UnixStream};

        if let Some(parent) = path.parent() {
            secure_state_dir(parent)?;
        }
        if path.exists() {
            if UnixStream::connect(&path).await.is_ok() {
                bail!("OSCIRIS daemon is already listening at {}", path.display());
            }
            tokio::fs::remove_file(&path)
                .await
                .with_context(|| format!("remove stale socket {}", path.display()))?;
        }

        let listener = UnixListener::bind(&path)
            .with_context(|| format!("bind local socket {}", path.display()))?;
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600))
            .with_context(|| format!("secure local socket {}", path.display()))?;
        let _cleanup = UnixSocketCleanup(path);

        loop {
            let (stream, _) = listener.accept().await.context("accept local client")?;
            let service = self.clone();
            tokio::spawn(async move {
                if let Err(error) = handle_connection(stream, service).await {
                    tracing::warn!(%error, "local daemon client failed");
                }
            });
        }
    }

    #[cfg(windows)]
    async fn serve_named_pipe(self, name: String) -> Result<()> {
        use tokio::net::windows::named_pipe::ServerOptions;

        let mut first = true;
        loop {
            let server = ServerOptions::new()
                .first_pipe_instance(first)
                .reject_remote_clients(true)
                .create(&name)
                .with_context(|| format!("create local named pipe {name}"))?;
            first = false;
            server
                .connect()
                .await
                .with_context(|| format!("accept local named pipe client {name}"))?;
            let service = self.clone();
            tokio::spawn(async move {
                if let Err(error) = handle_connection(server, service).await {
                    tracing::warn!(%error, "local daemon client failed");
                }
            });
        }
    }
}

#[cfg(unix)]
struct UnixSocketCleanup(PathBuf);

#[cfg(unix)]
impl Drop for UnixSocketCleanup {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.0);
    }
}

async fn handle_connection<T>(stream: T, service: DaemonService) -> Result<()>
where
    T: AsyncRead + AsyncWrite + Unpin,
{
    let mut framed = Framed::new(stream, LinesCodec::new_with_max_length(MAX_FRAME_BYTES));
    while let Some(frame) = framed.next().await {
        let line = frame.context("read local daemon frame")?;
        let response = match serde_json::from_str::<DaemonRequest>(&line) {
            Ok(request) => service.handle(request).await,
            Err(error) => DaemonResponse::error(0, "invalid_request", error.to_string()),
        };
        framed
            .send(serde_json::to_string(&response)?)
            .await
            .context("write local daemon response")?;
    }
    Ok(())
}

#[derive(Clone, Debug)]
pub struct DaemonClient {
    endpoint: LocalEndpoint,
    state_dir: PathBuf,
    timeout: Duration,
}

impl DaemonClient {
    pub fn new(endpoint: LocalEndpoint, state_dir: PathBuf) -> Self {
        Self {
            endpoint,
            state_dir,
            timeout: Duration::from_secs(3),
        }
    }

    pub fn default_for_user() -> Self {
        Self::new(LocalEndpoint::default_for_user(), default_state_dir())
    }

    pub fn with_timeout(mut self, timeout: Duration) -> Self {
        self.timeout = timeout;
        self
    }

    pub async fn status(&self) -> Result<DaemonStatus> {
        match self.send(DaemonCommand::GetStatus).await? {
            DaemonResult::Status(status) => Ok(status),
            result => bail!("unexpected daemon result: {result:?}"),
        }
    }

    pub async fn set_participation(&self, enabled: bool) -> Result<DaemonStatus> {
        match self
            .send(DaemonCommand::SetParticipation { enabled })
            .await?
        {
            DaemonResult::Status(status) => Ok(status),
            result => bail!("unexpected daemon result: {result:?}"),
        }
    }

    pub async fn send(&self, command: DaemonCommand) -> Result<DaemonResult> {
        let auth_token = read_auth_token(&self.state_dir)?;
        let request = DaemonRequest::new(auth_token, command);
        let request_id = request.request_id;
        let response = tokio::time::timeout(self.timeout, self.send_request(request))
            .await
            .map_err(|_| anyhow!("daemon request timed out after {:?}", self.timeout))??;
        if response.request_id != request_id {
            bail!(
                "daemon response ID {} did not match request ID {request_id}",
                response.request_id
            );
        }
        if let Some(error) = response.error {
            bail!("daemon error {}: {}", error.code, error.message);
        }
        response
            .result
            .ok_or_else(|| anyhow!("daemon response did not contain a result"))
    }

    async fn send_request(&self, request: DaemonRequest) -> Result<DaemonResponse> {
        match &self.endpoint {
            #[cfg(unix)]
            LocalEndpoint::Unix(path) => {
                let stream = tokio::net::UnixStream::connect(path)
                    .await
                    .with_context(|| format!("connect to daemon at {}", path.display()))?;
                exchange(stream, request).await
            }
            #[cfg(windows)]
            LocalEndpoint::NamedPipe(name) => {
                let stream = tokio::net::windows::named_pipe::ClientOptions::new()
                    .open(name)
                    .with_context(|| format!("connect to daemon at {name}"))?;
                exchange(stream, request).await
            }
        }
    }
}

async fn exchange<T>(stream: T, request: DaemonRequest) -> Result<DaemonResponse>
where
    T: AsyncRead + AsyncWrite + Unpin,
{
    let mut framed = Framed::new(stream, LinesCodec::new_with_max_length(MAX_FRAME_BYTES));
    framed
        .send(serde_json::to_string(&request)?)
        .await
        .context("send daemon request")?;
    let line = framed
        .next()
        .await
        .ok_or_else(|| anyhow!("daemon closed the local connection"))?
        .context("read daemon response")?;
    serde_json::from_str(&line).context("decode daemon response")
}

fn secure_state_dir(path: &Path) -> Result<()> {
    std::fs::create_dir_all(path)
        .with_context(|| format!("create daemon state directory {}", path.display()))?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o700))
            .with_context(|| format!("secure daemon state directory {}", path.display()))?;
    }
    Ok(())
}

fn state_path(state_dir: &Path) -> PathBuf {
    state_dir.join("daemon-state.json")
}

fn auth_token_path(state_dir: &Path) -> PathBuf {
    state_dir.join("daemon-auth-token")
}

fn ensure_auth_token(state_dir: &Path) -> Result<String> {
    let path = auth_token_path(state_dir);
    if path.exists() {
        return read_auth_token(state_dir);
    }

    let mut bytes = [0_u8; 32];
    OsRng.fill_bytes(&mut bytes);
    let token = URL_SAFE_NO_PAD.encode(bytes);

    let mut options = std::fs::OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    let mut file = match options.open(&path) {
        Ok(file) => file,
        Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {
            return read_auth_token(state_dir);
        }
        Err(error) => {
            return Err(error)
                .with_context(|| format!("create daemon credential {}", path.display()));
        }
    };
    file.write_all(token.as_bytes())
        .with_context(|| format!("write daemon credential {}", path.display()))?;
    file.sync_all()
        .with_context(|| format!("sync daemon credential {}", path.display()))?;
    secure_private_file(&path)?;
    Ok(token)
}

fn read_auth_token(state_dir: &Path) -> Result<String> {
    let path = auth_token_path(state_dir);
    secure_private_file(&path)?;
    let token = std::fs::read_to_string(&path)
        .with_context(|| format!("read daemon credential {}", path.display()))?;
    let token = token.trim().to_string();
    if token.len() < 32 {
        bail!("daemon credential at {} is invalid", path.display());
    }
    Ok(token)
}

fn load_state(state_dir: &Path) -> Result<PersistedState> {
    let path = state_path(state_dir);
    if !path.exists() {
        return Ok(PersistedState::default());
    }
    let bytes =
        std::fs::read(&path).with_context(|| format!("read daemon state {}", path.display()))?;
    serde_json::from_slice(&bytes)
        .with_context(|| format!("decode daemon state {}", path.display()))
}

fn persist_state(state_dir: &Path, state: &PersistedState) -> Result<()> {
    let path = state_path(state_dir);
    let mut file = AtomicWriteFile::open(&path)
        .with_context(|| format!("open atomic daemon state {}", path.display()))?;
    file.write_all(&serde_json::to_vec_pretty(state)?)
        .with_context(|| format!("write daemon state {}", path.display()))?;
    file.commit()
        .with_context(|| format!("commit daemon state {}", path.display()))
}

fn secure_private_file(path: &Path) -> Result<()> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600))
            .with_context(|| format!("secure private file {}", path.display()))?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn request_serialization_is_versioned_and_tagged() {
        let request = DaemonRequest {
            api_version: API_VERSION,
            request_id: 9,
            auth_token: "test-token-with-sufficient-length".to_string(),
            command: DaemonCommand::SetParticipation { enabled: true },
        };
        let value = serde_json::to_value(&request).unwrap();
        assert_eq!(value["api_version"], API_VERSION);
        assert_eq!(
            value["auth_token"],
            "test-token-with-sufficient-length".to_string()
        );
        assert_eq!(value["command"]["type"], "set_participation");
        assert_eq!(value["command"]["enabled"], true);
        assert_eq!(
            serde_json::from_value::<DaemonRequest>(value).unwrap(),
            request
        );
    }

    #[tokio::test]
    async fn participation_state_is_persisted() {
        let directory = tempfile::tempdir().unwrap();
        let service = DaemonService::new(directory.path().to_path_buf()).unwrap();
        let response = service
            .handle(DaemonRequest {
                api_version: API_VERSION,
                request_id: 3,
                auth_token: service.inner.auth_token.clone(),
                command: DaemonCommand::SetParticipation { enabled: true },
            })
            .await;
        assert!(response.error.is_none());
        drop(service);

        let restarted = DaemonService::new(directory.path().to_path_buf()).unwrap();
        assert!(restarted.status().await.participation_enabled);
    }

    #[tokio::test]
    async fn unsupported_api_version_is_rejected() {
        let directory = tempfile::tempdir().unwrap();
        let service = DaemonService::new(directory.path().to_path_buf()).unwrap();
        let response = service
            .handle(DaemonRequest {
                api_version: API_VERSION + 1,
                request_id: 4,
                auth_token: service.inner.auth_token.clone(),
                command: DaemonCommand::Ping,
            })
            .await;
        assert_eq!(
            response.error.unwrap().code,
            "unsupported_api_version".to_string()
        );
    }

    #[tokio::test]
    async fn invalid_local_credential_is_rejected() {
        let directory = tempfile::tempdir().unwrap();
        let service = DaemonService::new(directory.path().to_path_buf()).unwrap();
        let response = service
            .handle(DaemonRequest {
                api_version: API_VERSION,
                request_id: 5,
                auth_token: "not-the-daemon-token".to_string(),
                command: DaemonCommand::GetStatus,
            })
            .await;
        assert_eq!(
            response.error.unwrap().code,
            "authentication_failed".to_string()
        );
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn unix_client_round_trips_status_and_control() {
        let directory = tempfile::tempdir().unwrap();
        let endpoint = LocalEndpoint::Unix(directory.path().join("daemon.sock"));
        let state_dir = directory.path().join("state");
        let service = DaemonService::new(state_dir.clone()).unwrap();
        let server = tokio::spawn(service.serve(endpoint.clone()));

        for _ in 0..50 {
            let LocalEndpoint::Unix(path) = &endpoint;
            if path.exists() {
                break;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }

        let client = DaemonClient::new(endpoint, state_dir);
        let initial = client.status().await.unwrap();
        assert!(!initial.participation_enabled);
        let updated = client.set_participation(true).await.unwrap();
        assert!(updated.participation_enabled);
        server.abort();
    }
}
