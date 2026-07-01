use std::{
    env,
    path::{Path, PathBuf},
    process::{Command, Stdio},
    time::Duration,
};

use osciris_daemon::{
    default_state_dir, CreateJobInput, DaemonClient, DaemonStatus, DesktopJob,
    EvidenceIngestionInput, UnsignedTokenTransfer, WalletConfigInput, WalletStatus,
    WithdrawalInput, WorkspaceSnapshot,
};

#[tauri::command]
async fn daemon_status() -> Result<DaemonStatus, String> {
    DaemonClient::default_for_user()
        .status()
        .await
        .map_err(|error| error.to_string())
}

#[tauri::command]
async fn set_participation(enabled: bool) -> Result<DaemonStatus, String> {
    DaemonClient::default_for_user()
        .set_participation(enabled)
        .await
        .map_err(|error| error.to_string())
}

#[tauri::command]
async fn workspace_snapshot() -> Result<WorkspaceSnapshot, String> {
    DaemonClient::default_for_user()
        .workspace()
        .await
        .map_err(|error| error.to_string())
}

#[tauri::command]
async fn create_job(input: CreateJobInput) -> Result<DesktopJob, String> {
    DaemonClient::default_for_user()
        .create_job(input)
        .await
        .map_err(|error| error.to_string())
}

#[tauri::command]
async fn submit_job(job_id: String) -> Result<DesktopJob, String> {
    DaemonClient::default_for_user()
        .submit_job(job_id)
        .await
        .map_err(|error| error.to_string())
}

#[tauri::command]
async fn publish_job(job_id: String) -> Result<DesktopJob, String> {
    DaemonClient::default_for_user()
        .publish_job(job_id)
        .await
        .map_err(|error| error.to_string())
}

#[tauri::command]
async fn refresh_protocol_jobs() -> Result<WorkspaceSnapshot, String> {
    DaemonClient::default_for_user()
        .refresh_protocol_jobs()
        .await
        .map_err(|error| error.to_string())
}

#[tauri::command]
async fn ingest_evidence(input: EvidenceIngestionInput) -> Result<WorkspaceSnapshot, String> {
    DaemonClient::default_for_user()
        .ingest_evidence(input)
        .await
        .map_err(|error| error.to_string())
}

#[tauri::command]
async fn configure_wallet(input: WalletConfigInput) -> Result<WalletStatus, String> {
    DaemonClient::default_for_user()
        .configure_wallet(input)
        .await
        .map_err(|error| error.to_string())
}

#[tauri::command]
async fn refresh_wallet() -> Result<WalletStatus, String> {
    DaemonClient::default_for_user()
        .refresh_wallet()
        .await
        .map_err(|error| error.to_string())
}

#[tauri::command]
async fn prepare_withdrawal(input: WithdrawalInput) -> Result<UnsignedTokenTransfer, String> {
    DaemonClient::default_for_user()
        .prepare_withdrawal(input)
        .await
        .map_err(|error| error.to_string())
}

#[tauri::command]
async fn launch_daemon() -> Result<DaemonStatus, String> {
    if let Ok(status) = DaemonClient::default_for_user().status().await {
        return Ok(status);
    }

    let binary = resolve_daemon_binary()
        .ok_or_else(|| "osciris-daemon binary was not found beside the app".to_string())?;
    let mut command = Command::new(&binary);
    command
        .arg("--state-dir")
        .arg(default_state_dir())
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null());

    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        const CREATE_NO_WINDOW: u32 = 0x0800_0000;
        command.creation_flags(CREATE_NO_WINDOW);
    }

    command
        .spawn()
        .map_err(|error| format!("failed to launch {}: {error}", binary.display()))?;

    let client = DaemonClient::default_for_user().with_timeout(Duration::from_secs(1));
    let mut last_error = String::new();
    for _ in 0..30 {
        match client.status().await {
            Ok(status) => return Ok(status),
            Err(error) => last_error = error.to_string(),
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }

    Err(format!("osciris-daemon did not become ready: {last_error}"))
}

fn resolve_daemon_binary() -> Option<PathBuf> {
    let executable_name = if cfg!(windows) {
        "osciris-daemon.exe"
    } else {
        "osciris-daemon"
    };

    let mut candidates = Vec::new();
    if let Some(path) = env::var_os("OSCIRIS_DAEMON_BIN") {
        candidates.push(PathBuf::from(path));
    }
    if let Ok(current_executable) = env::current_exe() {
        if let Some(parent) = current_executable.parent() {
            candidates.push(parent.join(executable_name));
        }
    }
    if cfg!(debug_assertions) {
        candidates.push(
            Path::new(env!("CARGO_MANIFEST_DIR"))
                .join("../../../target/debug")
                .join(executable_name),
        );
    }

    candidates.into_iter().find(|candidate| candidate.is_file())
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    tauri::Builder::default()
        .plugin(tauri_plugin_dialog::init())
        .invoke_handler(tauri::generate_handler![
            configure_wallet,
            create_job,
            daemon_status,
            ingest_evidence,
            launch_daemon,
            prepare_withdrawal,
            publish_job,
            refresh_protocol_jobs,
            refresh_wallet,
            set_participation,
            submit_job,
            workspace_snapshot
        ])
        .run(tauri::generate_context!())
        .expect("error while running OSCIRIS desktop");
}
