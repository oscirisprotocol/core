import { useEffect, useState } from "react";
import {
  configureWallet,
  createJob,
  type CreateJobInput,
  type DaemonStatus,
  getDaemonStatus,
  getWorkspace,
  launchDaemon,
  prepareWithdrawal,
  refreshWallet,
  setParticipation,
  submitJob,
  type UnsignedTokenTransfer,
  type WalletConfigInput,
  type WithdrawalInput,
  type WorkspaceSnapshot,
} from "./lib/daemon";
import {
  EvidenceView,
  JobDetailView,
  JobsView,
  NodeView,
  OverviewView,
  WalletView,
} from "./views";

type View = "overview" | "jobs" | "evidence" | "wallet" | "node";
type ConnectionState = "loading" | "connected" | "offline";

const navigation: Array<{ id: View; label: string; group: string }> = [
  { id: "overview", label: "Overview", group: "Workspace" },
  { id: "jobs", label: "Compute jobs", group: "Workspace" },
  { id: "evidence", label: "Evidence", group: "Workspace" },
  { id: "wallet", label: "Wallet", group: "Economics" },
  { id: "node", label: "Local node", group: "Operator" },
];

function LogoMark() {
  return (
    <svg viewBox="0 0 774 774" aria-hidden="true">
      <path
        d="M224.261 35.6535C273.608 12.7737 328.578 0 386.51 0C576.28 0 734.291 137.063 766.863 317.51C728.465 271.899 682.019 233.283 629.722 203.857C557.807 163.392 474.834 140.298 386.51 140.298C309.396 140.298 236.363 157.901 171.207 189.297C179.822 134.295 198.106 82.4834 224.261 35.6535Z"
        fill="currentColor"
      />
      <path
        d="M772.957 379.76C772.996 382.019 773.015 384.282 773.015 386.55C773.015 599.868 599.827 773.056 386.51 773.056C337.767 773.056 291.121 764.014 248.156 747.518C300.571 736.203 349.924 716.611 394.771 690.182C541.463 603.738 639.97 444.12 639.97 261.679C639.97 257.123 639.909 252.582 639.787 248.055C692.578 282.494 737.95 327.376 772.957 379.76Z"
        fill="currentColor"
      />
      <path
        d="M196.42 723.031C79.1855 656.622 0 530.729 0 386.487C0 251.588 69.2591 132.739 174.125 63.5859C153.406 111.178 139.922 162.633 135.066 216.559L135.065 216.577C133.728 231.412 133.045 246.434 133.045 261.615C133.045 434.005 220.998 586.018 354.448 675.174C306.066 699.989 252.769 716.561 196.42 723.031Z"
        fill="currentColor"
      />
    </svg>
  );
}

function viewTitle(view: View) {
  return {
    overview: "Overview",
    jobs: "Compute jobs",
    evidence: "Evidence",
    wallet: "Wallet",
    node: "Local node",
  }[view];
}

export default function App() {
  const [view, setView] = useState<View>("overview");
  const [selectedJobId, setSelectedJobId] = useState<string | null>(null);
  const [status, setStatus] = useState<DaemonStatus | null>(null);
  const [workspace, setWorkspace] = useState<WorkspaceSnapshot | null>(null);
  const [connection, setConnection] = useState<ConnectionState>("loading");
  const [error, setError] = useState<string | null>(null);
  const [busy, setBusy] = useState(false);
  const [withdrawal, setWithdrawal] =
    useState<UnsignedTokenTransfer | null>(null);

  async function refreshAll(showError = false) {
    try {
      const nextStatus = await getDaemonStatus();
      const nextWorkspace = await getWorkspace();
      setStatus(nextStatus);
      setWorkspace(nextWorkspace);
      setConnection("connected");
      setError(null);
    } catch (requestError) {
      setStatus(null);
      setWorkspace(null);
      setConnection("offline");
      if (showError) setError(String(requestError));
    }
  }

  useEffect(() => {
    void refreshAll();
    const timer = window.setInterval(() => void refreshAll(), 8_000);
    return () => window.clearInterval(timer);
  }, []);

  async function runAction<T>(action: () => Promise<T>, after?: (value: T) => void) {
    setBusy(true);
    setError(null);
    try {
      const value = await action();
      after?.(value);
      await refreshAll();
    } catch (actionError) {
      setError(String(actionError));
    } finally {
      setBusy(false);
    }
  }

  async function handleLaunch() {
    await runAction(launchDaemon, (next) => {
      setStatus(next);
      setConnection("connected");
    });
  }

  const daemonLive = connection === "connected" && status !== null;
  const selectedJob =
    workspace?.jobs.find((job) => job.job_id === selectedJobId) ?? null;

  return (
    <div className="app-shell">
      <aside className="sidebar">
        <button
          className="brand"
          onClick={() => {
            setView("overview");
            setSelectedJobId(null);
          }}
          type="button"
        >
          <span className="brand-mark">
            <LogoMark />
          </span>
          <span>
            OSCIRIS
            <small>Compute workspace</small>
          </span>
        </button>

        <nav aria-label="Primary">
          {["Workspace", "Economics", "Operator"].map((group) => (
            <div className="nav-group" key={group}>
              <p className="nav-label">{group}</p>
              {navigation
                .filter((item) => item.group === group)
                .map((item) => (
                  <button
                    className={
                      view === item.id && !selectedJob
                        ? "nav-item active"
                        : "nav-item"
                    }
                    key={item.id}
                    onClick={() => {
                      setView(item.id);
                      setSelectedJobId(null);
                    }}
                    type="button"
                  >
                    <span className={`nav-glyph glyph-${item.id}`} />
                    {item.label}
                    {item.id === "jobs" && workspace?.jobs.length ? (
                      <span className="nav-count">{workspace.jobs.length}</span>
                    ) : null}
                  </button>
                ))}
            </div>
          ))}
        </nav>

        <div className="sidebar-foot">
          <div className={daemonLive ? "identity-orb live" : "identity-orb"}>
            OS
          </div>
          <div>
            <strong>{daemonLive ? "Workspace connected" : "Daemon offline"}</strong>
            <span>
              {workspace?.wallet.address
                ? `${workspace.wallet.address.slice(0, 8)}…${workspace.wallet.address.slice(-4)}`
                : "No wallet configured"}
            </span>
          </div>
        </div>
      </aside>

      <main>
        <header className="topbar">
          <div>
            <span className="breadcrumb">
              {selectedJob ? "Compute jobs / Detail" : "OSCIRIS workspace"}
            </span>
            <h1>{selectedJob ? selectedJob.title : viewTitle(view)}</h1>
          </div>
          <div className="topbar-actions">
            <div className={daemonLive ? "connection-chip live" : "connection-chip"}>
              <span className="status-dot" />
              {daemonLive ? "Testnet workspace" : "Local daemon offline"}
            </div>
            {!daemonLive ? (
              <button
                className="compact-primary"
                disabled={busy}
                onClick={() => void handleLaunch()}
                type="button"
              >
                {busy ? "Starting…" : "Start daemon"}
              </button>
            ) : (
              <button
                aria-label="Refresh workspace"
                className="icon-button"
                onClick={() => void refreshAll(true)}
                type="button"
              >
                ↻
              </button>
            )}
          </div>
        </header>

        {error ? (
          <div className="error-banner" role="alert">
            <strong>Action needs attention</strong>
            <span>{error}</span>
            <button onClick={() => setError(null)} type="button">
              Dismiss
            </button>
          </div>
        ) : null}

        {!daemonLive && view !== "node" ? (
          <section className="offline-strip">
            <div>
              <strong>Start the local daemon to use the workspace</strong>
              <span>
                Job drafts and wallet configuration are stored locally, never in
                the webview.
              </span>
            </div>
            <button
              className="secondary-button"
              disabled={busy}
              onClick={() => void handleLaunch()}
              type="button"
            >
              Start daemon
            </button>
          </section>
        ) : null}

        <div className="view-stage">
          {selectedJob ? (
            <JobDetailView
              job={selectedJob}
              onBack={() => setSelectedJobId(null)}
              onSubmit={(jobId) =>
                void runAction(() => submitJob(jobId), () => setSelectedJobId(jobId))
              }
              busy={busy}
            />
          ) : view === "overview" ? (
            <OverviewView
              status={status}
              workspace={workspace}
              onCreateJob={() => setView("jobs")}
              onOpenJob={setSelectedJobId}
              onOpenWallet={() => setView("wallet")}
            />
          ) : view === "jobs" ? (
            <JobsView
              jobs={workspace?.jobs ?? []}
              daemonLive={daemonLive}
              busy={busy}
              onCreate={(input: CreateJobInput) =>
                void runAction(() => createJob(input))
              }
              onOpen={setSelectedJobId}
            />
          ) : view === "evidence" ? (
            <EvidenceView
              jobs={workspace?.jobs ?? []}
              onOpen={setSelectedJobId}
            />
          ) : view === "wallet" ? (
            <WalletView
              wallet={workspace?.wallet ?? null}
              jobs={workspace?.jobs ?? []}
              busy={busy}
              withdrawal={withdrawal}
              onConfigure={(input: WalletConfigInput) =>
                void runAction(() => configureWallet(input))
              }
              onRefresh={() => void runAction(refreshWallet)}
              onPrepare={(input: WithdrawalInput) =>
                void runAction(() => prepareWithdrawal(input), setWithdrawal)
              }
              onClearWithdrawal={() => setWithdrawal(null)}
            />
          ) : (
            <NodeView
              status={status}
              daemonLive={daemonLive}
              busy={busy}
              onLaunch={() => void handleLaunch()}
              onParticipation={(enabled) =>
                void runAction(() => setParticipation(enabled))
              }
            />
          )}
        </div>

        <footer className="app-footer">
          <span>OSCIRIS Node Desktop · Testnet workspace</span>
          <span>Private compute · Verified execution · External key custody</span>
        </footer>
      </main>
    </div>
  );
}
