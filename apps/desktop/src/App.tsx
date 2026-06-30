import { useEffect, useState } from "react";
import {
  type DaemonStatus,
  getDaemonStatus,
  launchDaemon,
  setParticipation,
} from "./lib/daemon";

type ConnectionState = "loading" | "connected" | "offline";

const navigation = [
  { label: "Overview", active: true },
  { label: "Hardware", active: false },
  { label: "Models", active: false },
  { label: "Jobs", active: false },
  { label: "Receipts", active: false },
];

function LogoMark() {
  return (
    <svg viewBox="0 0 32 32" aria-hidden="true">
      <path
        d="M16 3.5 27 9.8v12.4L16 28.5 5 22.2V9.8L16 3.5Z"
        fill="none"
        stroke="currentColor"
        strokeWidth="2"
      />
      <path
        d="M10.2 17.6c2.9-5.1 8.4-7.2 12.7-4.4-2.7 5.5-8.1 7.7-12.7 4.4Z"
        fill="currentColor"
      />
    </svg>
  );
}

function StatusDot({ live }: { live: boolean }) {
  return <span className={live ? "status-dot live" : "status-dot"} />;
}

function formatDuration(totalSeconds: number) {
  if (totalSeconds < 60) return `${totalSeconds}s`;
  const hours = Math.floor(totalSeconds / 3600);
  const minutes = Math.floor((totalSeconds % 3600) / 60);
  if (hours === 0) return `${minutes}m`;
  return `${hours}h ${minutes}m`;
}

function titleCase(value: string) {
  return value
    .split("_")
    .map((part) => part.charAt(0).toUpperCase() + part.slice(1))
    .join(" ");
}

export default function App() {
  const [status, setStatus] = useState<DaemonStatus | null>(null);
  const [connection, setConnection] = useState<ConnectionState>("loading");
  const [error, setError] = useState<string | null>(null);
  const [busy, setBusy] = useState(false);

  async function refreshStatus(showError = false) {
    try {
      const next = await getDaemonStatus();
      setStatus(next);
      setConnection("connected");
      setError(null);
    } catch (requestError) {
      setStatus(null);
      setConnection("offline");
      if (showError) {
        setError(String(requestError));
      }
    }
  }

  useEffect(() => {
    void refreshStatus();
    const timer = window.setInterval(() => void refreshStatus(), 4_000);
    return () => window.clearInterval(timer);
  }, []);

  async function handleLaunch() {
    setBusy(true);
    setError(null);
    try {
      const next = await launchDaemon();
      setStatus(next);
      setConnection("connected");
    } catch (launchError) {
      setError(String(launchError));
      setConnection("offline");
    } finally {
      setBusy(false);
    }
  }

  async function handleParticipation(enabled: boolean) {
    setBusy(true);
    setError(null);
    try {
      setStatus(await setParticipation(enabled));
    } catch (controlError) {
      setError(String(controlError));
    } finally {
      setBusy(false);
    }
  }

  const daemonLive = connection === "connected" && status !== null;
  const participating = status?.participation_enabled ?? false;
  const stateLabel = !daemonLive
    ? "Daemon offline"
    : participating
      ? "Participation enabled"
      : "Participation paused";

  return (
    <div className="app-shell">
      <aside className="sidebar">
        <div className="brand">
          <span className="brand-mark">
            <LogoMark />
          </span>
          <span>OSCIRIS</span>
        </div>

        <nav aria-label="Primary">
          <p className="nav-label">Node</p>
          {navigation.map((item) => (
            <button
              className={item.active ? "nav-item active" : "nav-item"}
              disabled={!item.active}
              key={item.label}
              type="button"
            >
              <span className="nav-glyph" />
              {item.label}
              {!item.active && <span className="soon">Soon</span>}
            </button>
          ))}
        </nav>

        <div className="sidebar-foot">
          <div className="identity-orb">OS</div>
          <div>
            <strong>Local participant</strong>
            <span>Identity not configured</span>
          </div>
        </div>
      </aside>

      <main>
        <header className="topbar">
          <div>
            <span className="breadcrumb">Participant node</span>
            <h1>Overview</h1>
          </div>
          <div className="topbar-actions">
            <div className="connection-chip">
              <StatusDot live={daemonLive} />
              {daemonLive ? "Daemon connected" : "Daemon offline"}
            </div>
            <button
              className="icon-button"
              type="button"
              aria-label="Refresh daemon status"
              onClick={() => void refreshStatus(true)}
            >
              ↻
            </button>
          </div>
        </header>

        <section className="hero-panel">
          <div className="hero-copy">
            <span className="eyebrow">LOCAL NODE CONTROL</span>
            <h2>{stateLabel}</h2>
            <p>
              Models and workloads stay on participant machines. This desktop
              controls your local daemon and reports only measured node state.
            </p>
            <div className="hero-actions">
              {!daemonLive ? (
                <button
                  className="primary-button"
                  disabled={busy}
                  onClick={() => void handleLaunch()}
                  type="button"
                >
                  {busy ? "Starting…" : "Start local daemon"}
                </button>
              ) : (
                <button
                  className={participating ? "secondary-button" : "primary-button"}
                  disabled={busy}
                  onClick={() => void handleParticipation(!participating)}
                  type="button"
                >
                  {busy
                    ? "Updating…"
                    : participating
                      ? "Pause participation"
                      : "Enable participation"}
                </button>
              )}
              <span className="endpoint-note">
                Per-user authenticated IPC · API v{status?.api_version ?? 1}
              </span>
            </div>
          </div>
          <div className="signal-visual" aria-hidden="true">
            <div className="signal-ring ring-one" />
            <div className="signal-ring ring-two" />
            <div className={daemonLive ? "signal-core active" : "signal-core"}>
              <LogoMark />
            </div>
            <span className="signal-node node-a" />
            <span className="signal-node node-b" />
            <span className="signal-node node-c" />
          </div>
        </section>

        {error && (
          <div className="error-banner" role="alert">
            <strong>Local control failed</strong>
            <span>{error}</span>
          </div>
        )}

        <section className="metric-grid" aria-label="Node metrics">
          <article className="metric-card">
            <div className="metric-head">
              <span>Daemon</span>
              <StatusDot live={daemonLive} />
            </div>
            <strong>{status ? `v${status.daemon_version}` : "Not running"}</strong>
            <p>
              {status
                ? `${formatDuration(status.uptime_seconds)} uptime`
                : "Start the local process to expose node controls."}
            </p>
          </article>
          <article className="metric-card">
            <div className="metric-head">
              <span>Network</span>
              <span className="metric-tag">Pending</span>
            </div>
            <strong>
              {status ? titleCase(status.network_state) : "Not configured"}
            </strong>
            <p>Peer bootstrap and live readiness arrive in the next daemon API.</p>
          </article>
          <article className="metric-card">
            <div className="metric-head">
              <span>Platform</span>
              <span className="metric-tag neutral">Local</span>
            </div>
            <strong>
              {status
                ? `${titleCase(status.platform.operating_system)} · ${status.platform.architecture}`
                : "Awaiting daemon"}
            </strong>
            <p>Accelerator detection and signed capability are not yet published.</p>
          </article>
          <article className="metric-card">
            <div className="metric-head">
              <span>Active jobs</span>
              <span className="metric-tag neutral">Measured</span>
            </div>
            <strong>{status?.active_jobs ?? "—"}</strong>
            <p>No synthetic workload counts are shown.</p>
          </article>
        </section>

        <section className="content-grid">
          <article className="readiness-card">
            <div className="section-heading">
              <div>
                <span className="eyebrow">NETWORK READINESS</span>
                <h3>Capacity gaps</h3>
              </div>
              <span className="pending-badge">Snapshot pending</span>
            </div>
            <div className="gap-table">
              {[
                ["Compatible providers", "4", "Provider API pending"],
                ["Inference slots", "3", "Profile API pending"],
                ["Independent verifiers", "2", "Peer API pending"],
              ].map(([label, target, note]) => (
                <div className="gap-row" key={label}>
                  <div>
                    <strong>{label}</strong>
                    <span>{note}</span>
                  </div>
                  <div className="gap-value">
                    <span>—</span>
                    <small>/ {target}</small>
                  </div>
                </div>
              ))}
            </div>
            <p className="card-footnote">
              Gaps activate only after the daemon receives signed peer and
              profile snapshots.
            </p>
          </article>

          <article className="activity-card">
            <div className="section-heading">
              <div>
                <span className="eyebrow">LOCAL ACTIVITY</span>
                <h3>Receipts and jobs</h3>
              </div>
            </div>
            <div className="empty-state">
              <div className="empty-icon">
                <span />
                <span />
                <span />
              </div>
              <strong>No local activity yet</strong>
              <p>
                Verified jobs, receipts, and testnet anchors will appear here
                after their daemon endpoints are connected.
              </p>
            </div>
          </article>
        </section>

        <footer className="app-footer">
          <span>OSCIRIS Node Desktop · Foundation build</span>
          <span>Provider-local compute · No central inference server</span>
        </footer>
      </main>
    </div>
  );
}
