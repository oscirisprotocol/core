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
