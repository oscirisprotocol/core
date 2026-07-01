import { useState } from "react";
import type {
  CreateJobInput,
  DaemonStatus,
  DesktopJob,
  JobKind,
  JobState,
  PrivacyMode,
  UnsignedTokenTransfer,
  WalletConfigInput,
  WalletStatus,
  WithdrawalInput,
  WorkspaceSnapshot,
} from "./lib/daemon";

const lifecycle: JobState[] = [
  "draft",
  "awaiting_funding",
  "queued",
  "matching",
  "running",
  "verifying",
  "completed",
];

const stateLabels: Record<JobState, string> = {
  draft: "Draft",
  awaiting_funding: "Funding review",
  queued: "Queued",
  matching: "Matching",
  running: "Running",
  verifying: "Verifying",
  completed: "Completed",
  failed: "Failed",
};

function money(micros: number) {
  return `$${(micros / 1_000_000).toLocaleString(undefined, {
    minimumFractionDigits: 2,
    maximumFractionDigits: 2,
  })}`;
}

function tokenAmount(value: string | null, decimals: number) {
  if (!value) return "—";
  const digits = value.replace(/^0+(?=\d)/, "");
  if (decimals === 0) return digits;
  const padded = digits.padStart(decimals + 1, "0");
  const integer = padded.slice(0, -decimals);
  const fraction = padded
    .slice(-decimals)
    .slice(0, Math.min(decimals, 4))
    .replace(/0+$/, "");
  return fraction ? `${integer}.${fraction}` : integer;
}

function short(value: string | null, length = 10) {
  if (!value) return "Pending";
  if (value.length <= length * 2) return value;
  return `${value.slice(0, length)}…${value.slice(-length)}`;
}

function titleCase(value: string) {
  return value
    .split("_")
    .map((part) => part.charAt(0).toUpperCase() + part.slice(1))
    .join(" ");
}

function StatusPill({ state }: { state: JobState }) {
  return (
    <span className={`status-pill state-${state}`}>{stateLabels[state]}</span>
  );
}

function EmptyPanel({
  title,
  body,
  action,
}: {
  title: string;
  body: string;
  action?: React.ReactNode;
}) {
  return (
    <div className="empty-panel">
      <div className="empty-symbol">
        <span />
        <span />
        <span />
      </div>
      <strong>{title}</strong>
      <p>{body}</p>
      {action}
    </div>
  );
}

export function OverviewView({
  status,
  workspace,
  onCreateJob,
  onOpenJob,
  onOpenWallet,
}: {
  status: DaemonStatus | null;
  workspace: WorkspaceSnapshot | null;
  onCreateJob: () => void;
  onOpenJob: (jobId: string) => void;
  onOpenWallet: () => void;
}) {
  const jobs = workspace?.jobs ?? [];
  const running = jobs.filter((job) =>
    ["queued", "matching", "running", "verifying"].includes(job.state),
  ).length;
  const completed = jobs.filter((job) => job.state === "completed").length;
  const totalBudget = jobs.reduce(
    (sum, job) => sum + job.budget_usdc_micros,
    0,
  );

  return (
    <>
      <section className="business-hero">
        <div>
          <span className="eyebrow">SOVEREIGN AI OPERATIONS</span>
          <h2>Private compute, accountable by design.</h2>
          <p>
            Launch training and inference across independent GPU providers,
            control privacy policy and spend, then export verification evidence
            for every completed workload.
          </p>
          <div className="hero-actions">
            <button className="primary-button" onClick={onCreateJob} type="button">
              Create compute job
            </button>
            <button className="text-button" onClick={onOpenWallet} type="button">
              Fund workspace →
            </button>
          </div>
        </div>
        <div className="flow-visual" aria-hidden="true">
          <span className="flow-label label-a">POLICY</span>
          <span className="flow-label label-b">COMPUTE</span>
          <span className="flow-label label-c">PROOF</span>
          <div className="flow-core">OS</div>
          <span className="flow-line line-a" />
          <span className="flow-line line-b" />
          <span className="flow-line line-c" />
        </div>
      </section>

      <section className="kpi-grid">
        <article>
          <span>Workspace budget</span>
          <strong>{money(totalBudget)}</strong>
          <small>{jobs.length ? `${jobs.length} job records` : "No jobs yet"}</small>
        </article>
        <article>
          <span>In progress</span>
          <strong>{running}</strong>
          <small>Queued through verification</small>
        </article>
        <article>
          <span>Verified completions</span>
          <strong>{completed}</strong>
          <small>Receipt-backed outcomes</small>
        </article>
        <article>
          <span>Local node</span>
          <strong>{status?.participation_enabled ? "Available" : "Paused"}</strong>
          <small>{status ? `${status.platform.operating_system} · ${status.platform.architecture}` : "Daemon offline"}</small>
        </article>
      </section>

      <section className="dashboard-grid">
        <article className="panel recent-panel">
          <div className="panel-heading">
            <div>
              <span className="eyebrow">WORKLOAD CONTROL</span>
              <h3>Recent jobs</h3>
            </div>
            <button className="text-button" onClick={onCreateJob} type="button">
              View all
            </button>
          </div>
          {jobs.length ? (
            <div className="job-list compact">
              {jobs.slice(0, 4).map((job) => (
                <button
                  className="job-row"
                  key={job.job_id}
                  onClick={() => onOpenJob(job.job_id)}
                  type="button"
                >
                  <span className={`job-kind kind-${job.kind}`}>
                    {job.kind === "training" ? "TR" : "IN"}
                  </span>
                  <span className="job-main">
                    <strong>{job.title}</strong>
                    <small>{job.model_id}</small>
                  </span>
                  <StatusPill state={job.state} />
                  <span className="job-budget">{money(job.budget_usdc_micros)}</span>
                  <span className="row-arrow">→</span>
                </button>
              ))}
            </div>
          ) : (
            <EmptyPanel
              title="No compute jobs"
              body="Create a training or inference draft to define policy, hardware, verification, and budget."
            />
          )}
        </article>

        <article className="panel treasury-panel">
          <div className="panel-heading">
            <div>
              <span className="eyebrow">ECONOMICS</span>
              <h3>Workspace treasury</h3>
            </div>
            <span className="network-badge">HORIZEN TESTNET</span>
          </div>
          <div className="treasury-balance">
            <span>Settlement asset</span>
            <strong>
              {workspace?.wallet.settlement_token
                ? `${tokenAmount(
                    workspace.wallet.settlement_token.balance_atomic,
                    workspace.wallet.settlement_token.decimals,
                  )} ${workspace.wallet.settlement_token.symbol}`
                : "Not configured"}
            </strong>
          </div>
          <div className="treasury-lines">
            <div>
              <span>Committed</span>
              <strong>{money(workspace?.wallet.committed_usdc_micros ?? 0)}</strong>
            </div>
            <div>
              <span>Custody</span>
              <strong>External wallet</strong>
            </div>
          </div>
          <button className="secondary-button full" onClick={onOpenWallet} type="button">
            Manage wallet
          </button>
        </article>
      </section>
    </>
  );
}

const initialJob: CreateJobInput = {
  kind: "inference",
  title: "",
  model_id: "Qwen/Qwen3-4B",
  workload: "",
  privacy_mode: "dsp_prepared",
  hardware_profile: "gpu-24gb",
  required_verifier_count: 2,
  challenge_window_seconds: 3_600,
  budget_usdc_micros: 5_000_000,
};

export function JobsView({
  jobs,
  daemonLive,
  busy,
  onCreate,
  onOpen,
}: {
  jobs: DesktopJob[];
  daemonLive: boolean;
  busy: boolean;
  onCreate: (input: CreateJobInput) => void;
  onOpen: (jobId: string) => void;
}) {
  const [kind, setKind] = useState<"all" | JobKind>("all");
  const [state, setState] = useState<"all" | JobState>("all");
  const [creating, setCreating] = useState(false);
  const [form, setForm] = useState<CreateJobInput>(initialJob);
  const filtered = jobs.filter(
    (job) =>
      (kind === "all" || job.kind === kind) &&
      (state === "all" || job.state === state),
  );

  function submit(event: React.FormEvent) {
    event.preventDefault();
    onCreate(form);
    setCreating(false);
    setForm(initialJob);
  }

  return (
    <section className="workspace-section">
      <div className="section-toolbar">
        <div className="segmented">
          {(["all", "training", "inference"] as const).map((value) => (
            <button
              className={kind === value ? "active" : ""}
              key={value}
              onClick={() => setKind(value)}
              type="button"
            >
              {titleCase(value)}
            </button>
          ))}
        </div>
        <button
          className="primary-button"
          disabled={!daemonLive}
          onClick={() => setCreating(true)}
          type="button"
        >
          New compute job
        </button>
      </div>

      <div className="status-filters">
        {(
          [
            ["all", "All"],
            ["draft", "Draft"],
            ["awaiting_funding", "Pending"],
            ["running", "Running"],
            ["completed", "Done"],
            ["failed", "Failed"],
          ] as Array<[typeof state, string]>
        ).map(([value, label]) => (
          <button
            className={state === value ? "active" : ""}
            key={value}
            onClick={() => setState(value)}
            type="button"
          >
            {label}
            <span>
              {value === "all"
                ? jobs.length
                : jobs.filter((job) => job.state === value).length}
            </span>
          </button>
        ))}
      </div>

      <article className="panel table-panel">
        <div className="table-header">
          <span>Job</span>
          <span>Type</span>
          <span>Status</span>
          <span>Privacy</span>
          <span>Budget</span>
          <span />
        </div>
        {filtered.length ? (
          filtered.map((job) => (
            <button
              className="table-row"
              key={job.job_id}
              onClick={() => onOpen(job.job_id)}
              type="button"
            >
              <span className="job-main">
                <strong>{job.title}</strong>
                <small>{job.model_id}</small>
              </span>
              <span>{titleCase(job.kind)}</span>
              <StatusPill state={job.state} />
              <span>{titleCase(job.privacy_mode)}</span>
              <strong>{money(job.budget_usdc_micros)}</strong>
              <span className="row-arrow">→</span>
            </button>
          ))
        ) : (
          <EmptyPanel
            title="No jobs in this view"
            body="Adjust the filters or create a compute job."
          />
        )}
      </article>

      {creating ? (
        <div className="modal-backdrop" role="presentation">
          <form className="job-composer" onSubmit={submit}>
            <div className="composer-heading">
              <div>
                <span className="eyebrow">NEW WORKLOAD</span>
                <h2>Create compute job</h2>
                <p>Define execution, privacy, verification, and economics.</p>
              </div>
              <button onClick={() => setCreating(false)} type="button">
                ×
              </button>
            </div>

            <div className="kind-picker">
              {(["training", "inference"] as const).map((value) => (
                <button
                  className={form.kind === value ? "active" : ""}
                  key={value}
                  onClick={() => setForm({ ...form, kind: value })}
                  type="button"
                >
                  <strong>{titleCase(value)}</strong>
                  <span>
                    {value === "training"
                      ? "Fine-tuning and perpetual learning"
                      : "Private provider-local model execution"}
                  </span>
                </button>
              ))}
            </div>

            <div className="form-grid">
              <label>
                Job title
                <input
                  maxLength={96}
                  onChange={(event) =>
                    setForm({ ...form, title: event.target.value })
                  }
                  placeholder="Private support-agent evaluation"
                  required
                  value={form.title}
                />
              </label>
              <label>
                Model
                <input
                  maxLength={160}
                  onChange={(event) =>
                    setForm({ ...form, model_id: event.target.value })
                  }
                  required
                  value={form.model_id}
                />
              </label>
              <label className="wide">
                Workload
                <textarea
                  maxLength={2_000}
                  onChange={(event) =>
                    setForm({ ...form, workload: event.target.value })
                  }
                  placeholder={
                    form.kind === "training"
                      ? "Describe the dataset artifact, adapter objective, and evaluation target."
                      : "Describe the prompt workload, output schema, and quality target."
                  }
                  required
                  rows={4}
                  value={form.workload}
                />
              </label>
              <label>
                Privacy mode
                <select
                  onChange={(event) =>
                    setForm({
                      ...form,
                      privacy_mode: event.target.value as PrivacyMode,
                    })
                  }
                  value={form.privacy_mode}
                >
                  <option value="dsp_prepared">DSP prepared</option>
                  <option value="dp_model_release">DP model release</option>
                  <option value="raw_baseline">Raw baseline</option>
                </select>
              </label>
              <label>
                Hardware profile
                <select
                  onChange={(event) =>
                    setForm({ ...form, hardware_profile: event.target.value })
                  }
                  value={form.hardware_profile}
                >
                  <option value="gpu-24gb">GPU · 24 GB+</option>
                  <option value="gpu-48gb">GPU · 48 GB+</option>
                  <option value="apple-unified-24gb">
                    Apple unified · 24 GB+
                  </option>
                  <option value="cpu-bounded">CPU · bounded</option>
                </select>
              </label>
              <label>
                Independent verifiers
                <input
                  max={10}
                  min={1}
                  onChange={(event) =>
                    setForm({
                      ...form,
                      required_verifier_count: Number(event.target.value),
                    })
                  }
                  type="number"
                  value={form.required_verifier_count}
                />
              </label>
              <label>
                Budget (test USDC)
                <input
                  min="0.01"
                  onChange={(event) =>
                    setForm({
                      ...form,
                      budget_usdc_micros: Math.round(
                        Number(event.target.value) * 1_000_000,
                      ),
                    })
                  }
                  step="0.01"
                  type="number"
                  value={form.budget_usdc_micros / 1_000_000}
                />
              </label>
            </div>

            <div className="composer-note">
              <strong>Draft first.</strong>
              <span>
                Saving does not broadcast a job or move funds. Funding and
                network submission remain explicit steps.
              </span>
            </div>
            <div className="composer-actions">
              <button
                className="text-button"
                onClick={() => setCreating(false)}
                type="button"
              >
                Cancel
              </button>
              <button className="primary-button" disabled={busy} type="submit">
                {busy ? "Saving…" : "Save job draft"}
              </button>
            </div>
          </form>
        </div>
      ) : null}
    </section>
  );
}

export function JobDetailView({
  job,
  busy,
  onBack,
  onSubmit,
  onPublish,
  onIngestEvidence,
}: {
  job: DesktopJob;
  busy: boolean;
  onBack: () => void;
  onSubmit: (jobId: string) => void;
  onPublish: (jobId: string) => void;
  onIngestEvidence: (jobId: string) => void;
}) {
  const current = lifecycle.indexOf(job.state);
  const canIngestEvidence = ["queued", "matching", "running", "verifying"].includes(
    job.state,
  );
  return (
    <section className="workspace-section">
      <div className="detail-toolbar">
        <button className="back-button" onClick={onBack} type="button">
          ← All jobs
        </button>
        <div>
          <StatusPill state={job.state} />
          {job.state === "draft" ? (
            <button
              className="primary-button"
              disabled={busy}
              onClick={() => onSubmit(job.job_id)}
              type="button"
            >
              Send to funding review
            </button>
          ) : null}
          {job.state === "awaiting_funding" ? (
            <button
              className="primary-button"
              disabled={busy}
              onClick={() => onPublish(job.job_id)}
              type="button"
            >
              Publish protocol job
            </button>
          ) : null}
          {canIngestEvidence ? (
            <button
              className="secondary-button"
              disabled={busy}
              onClick={() => onIngestEvidence(job.job_id)}
              type="button"
            >
              Import evidence
            </button>
          ) : null}
        </div>
      </div>

      <section className="detail-summary">
        <div>
          <span className="eyebrow">{job.kind.toUpperCase()} JOB</span>
          <h2>{job.model_id}</h2>
          <p>{job.workload}</p>
        </div>
        <div className="detail-economics">
          <span>Maximum budget</span>
          <strong>{money(job.budget_usdc_micros)}</strong>
          <small>Stable-value test settlement</small>
        </div>
      </section>

      <article className="panel lifecycle-panel">
        <div className="panel-heading">
          <div>
            <span className="eyebrow">EXECUTION CONTROL</span>
            <h3>Job lifecycle</h3>
          </div>
          <span className="mono-id">{short(job.job_id, 7)}</span>
        </div>
        <div className="lifecycle-track">
          {lifecycle.map((state, index) => (
            <div
              className={
                index < current
                  ? "lifecycle-step done"
                  : index === current
                    ? "lifecycle-step active"
                    : "lifecycle-step"
              }
              key={state}
            >
              <span>{index < current ? "✓" : index + 1}</span>
              <strong>{stateLabels[state]}</strong>
            </div>
          ))}
        </div>
        {job.state === "awaiting_funding" ? (
          <div className="boundary-callout warning">
            <strong>Ready to publish protocol announcement</strong>
            <span>
              Publishing records a signed local job announcement for provider
              matching. External wallet funding and network execution remain
              separate steps.
            </span>
          </div>
        ) : null}
        {canIngestEvidence ? (
          <div className="boundary-callout">
            <strong>Provider evidence can be imported manually</strong>
            <span>
              Select the provider evidence folder containing job_spec,
              execution_receipt, and receipt_bundle JSON files. The daemon will
              verify signatures and hashes before updating this job.
            </span>
          </div>
        ) : null}
      </article>

      <section className="detail-grid">
        <article className="panel">
          <div className="panel-heading">
            <div>
              <span className="eyebrow">POLICY</span>
              <h3>Execution terms</h3>
            </div>
          </div>
          <dl className="definition-list">
            <div>
              <dt>Privacy mode</dt>
              <dd>{titleCase(job.privacy_mode)}</dd>
            </div>
            <div>
              <dt>Hardware</dt>
              <dd>{titleCase(job.hardware_profile)}</dd>
            </div>
            <div>
              <dt>Verifier quorum</dt>
              <dd>{job.required_verifier_count} independent receipts</dd>
            </div>
            <div>
              <dt>Challenge window</dt>
              <dd>{job.challenge_window_seconds / 60} minutes</dd>
            </div>
            <div>
              <dt>Assigned provider</dt>
              <dd>{short(job.provider_node_id)}</dd>
            </div>
          </dl>
        </article>

        <article className="panel">
          <div className="panel-heading">
            <div>
              <span className="eyebrow">PROOF</span>
              <h3>Evidence receipt</h3>
            </div>
            <span className="pending-badge">
              {job.evidence.verification_status ?? "Pending"}
            </span>
          </div>
          <dl className="definition-list mono-values">
            <div>
              <dt>Execution receipt</dt>
              <dd>{short(job.evidence.execution_receipt_sha256)}</dd>
            </div>
            <div>
              <dt>Evidence bundle</dt>
              <dd>{short(job.evidence.bundle_sha256)}</dd>
            </div>
            <div>
              <dt>Verifier receipts</dt>
              <dd>
                {job.evidence.verifier_count} / {job.required_verifier_count}
              </dd>
            </div>
            <div>
              <dt>Horizen anchor</dt>
              <dd>{short(job.evidence.chain_tx_hash)}</dd>
            </div>
          </dl>
          <p className="panel-footnote">
            Hashes appear only after provider execution and independent
            verification.
          </p>
        </article>
      </section>
    </section>
  );
}

export function EvidenceView({
  jobs,
  onOpen,
}: {
  jobs: DesktopJob[];
  onOpen: (jobId: string) => void;
}) {
  const evidenced = jobs.filter(
    (job) =>
      job.evidence.execution_receipt_sha256 ||
      job.evidence.bundle_sha256 ||
      job.evidence.chain_tx_hash,
  );
  return (
    <section className="workspace-section">
      <div className="section-intro">
        <div>
          <span className="eyebrow">VERIFIABLE DELIVERY</span>
          <h2>Proof for every completed workload.</h2>
          <p>
            Execution receipts bind job terms, provider output, artifact roots,
            verifier decisions, and Horizen anchor transactions.
          </p>
        </div>
        <div className="evidence-stat">
          <strong>{evidenced.length}</strong>
          <span>Evidence bundles</span>
        </div>
      </div>
      <article className="panel evidence-panel">
        {evidenced.length ? (
          evidenced.map((job) => (
            <button
              className="evidence-row"
              key={job.job_id}
              onClick={() => onOpen(job.job_id)}
              type="button"
            >
              <span className="proof-mark">✓</span>
              <span className="job-main">
                <strong>{job.title}</strong>
                <small>{short(job.evidence.bundle_sha256)}</small>
              </span>
              <span>
                {job.evidence.verifier_count}/{job.required_verifier_count}{" "}
                verifiers
              </span>
              <span>{job.evidence.chain_tx_hash ? "Anchored" : "Not anchored"}</span>
              <span className="row-arrow">→</span>
            </button>
          ))
        ) : (
          <EmptyPanel
            title="No evidence receipts yet"
            body="Draft and funding states do not create proof. Receipts appear only after real provider execution and verifier review."
          />
        )}
      </article>
    </section>
  );
}

export function WalletView({
  wallet,
  jobs,
  busy,
  withdrawal,
  onConfigure,
  onRefresh,
  onPrepare,
  onClearWithdrawal,
}: {
  wallet: WalletStatus | null;
  jobs: DesktopJob[];
  busy: boolean;
  withdrawal: UnsignedTokenTransfer | null;
  onConfigure: (input: WalletConfigInput) => void;
  onRefresh: () => void;
  onPrepare: (input: WithdrawalInput) => void;
  onClearWithdrawal: () => void;
}) {
  const [configuring, setConfiguring] = useState(false);
  const [withdrawing, setWithdrawing] = useState(false);
  const [config, setConfig] = useState<WalletConfigInput>({
    address: wallet?.address ?? "",
    settlement_token_address:
      wallet?.settlement_token?.contract_address ?? null,
    settlement_token_symbol: wallet?.settlement_token?.symbol ?? "USDC_TEST",
    settlement_token_decimals: wallet?.settlement_token?.decimals ?? 6,
  });
  const [withdraw, setWithdraw] = useState<WithdrawalInput>({
    recipient: "",
    amount_atomic: "",
  });

  const committed = jobs
    .filter((job) => job.state !== "draft")
    .reduce((sum, job) => sum + job.budget_usdc_micros, 0);

  function saveConfig(event: React.FormEvent) {
    event.preventDefault();
    onConfigure({
      ...config,
      settlement_token_address:
        config.settlement_token_address?.trim() || null,
    });
    setConfiguring(false);
  }

  function prepare(event: React.FormEvent) {
    event.preventDefault();
    onPrepare(withdraw);
    setWithdrawing(false);
  }

  return (
    <section className="workspace-section">
      <div className="wallet-hero">
        <div>
          <span className="eyebrow">WORKSPACE TREASURY</span>
          <h2>Stable compute economics, external key custody.</h2>
          <p>
            OSCIRIS reads Horizen testnet balances and prepares transactions.
            Signing remains in your EVM wallet; private keys never enter this app.
          </p>
        </div>
        <div className="wallet-network">
          <span className="network-dot" />
          <div>
            <strong>{wallet?.network_name ?? "Horizen Testnet"}</strong>
            <small>Chain ID {wallet?.chain_id ?? 2_651_420}</small>
          </div>
        </div>
      </div>

      {wallet?.configured ? (
        <>
          <section className="balance-grid">
            <article className="primary-balance">
              <span>Settlement balance</span>
              <strong>
                {wallet.settlement_token
                  ? tokenAmount(
                      wallet.settlement_token.balance_atomic,
                      wallet.settlement_token.decimals,
                    )
                  : "—"}
                <small>
                  {wallet.settlement_token?.symbol ?? "TOKEN NOT CONFIGURED"}
                </small>
              </strong>
              <p>
                {wallet.sync_error
                  ? `Sync issue: ${wallet.sync_error}`
                  : wallet.last_synced_at
                    ? `Synced ${new Date(wallet.last_synced_at).toLocaleTimeString()}`
                    : "Balance not synced"}
              </p>
            </article>
            <article>
              <span>Native gas balance</span>
              <strong>
                {tokenAmount(wallet.native_balance_wei, 18)}
                <small>ETH</small>
              </strong>
              <p>Required for Horizen testnet transactions.</p>
            </article>
            <article>
              <span>Committed budget</span>
              <strong>
                {money(committed)}
                <small>TEST USD</small>
              </strong>
              <p>Jobs beyond local draft state.</p>
            </article>
          </section>

          <section className="wallet-grid">
            <article className="panel deposit-panel">
              <div className="panel-heading">
                <div>
                  <span className="eyebrow">DEPOSIT</span>
                  <h3>Fund this workspace</h3>
                </div>
              </div>
              <p>
                Send only Horizen testnet assets to this watch-only address.
              </p>
              <div className="address-box">
                <span>{wallet.address}</span>
                <button
                  onClick={() =>
                    void navigator.clipboard.writeText(wallet.address ?? "")
                  }
                  type="button"
                >
                  Copy
                </button>
              </div>
              <div className="asset-warning">
                <strong>Testnet asset boundary</strong>
                <span>
                  Horizen does not publish an official testnet USDC address.
                  Configure only the OSCIRIS test-token contract used by the
                  current deployment.
                </span>
              </div>
            </article>

            <article className="panel wallet-actions-panel">
              <div className="panel-heading">
                <div>
                  <span className="eyebrow">CONTROL</span>
                  <h3>Wallet actions</h3>
                </div>
              </div>
              <button
                className="wallet-action"
                disabled={busy}
                onClick={onRefresh}
                type="button"
              >
                <span>↻</span>
                <div>
                  <strong>Refresh balances</strong>
                  <small>Read from official Horizen RPC</small>
                </div>
              </button>
              <button
                className="wallet-action"
                disabled={!wallet.settlement_token}
                onClick={() => setWithdrawing(true)}
                type="button"
              >
                <span>↗</span>
                <div>
                  <strong>Prepare withdrawal</strong>
                  <small>
                    {wallet.settlement_token
                      ? "Generate an external-wallet payload"
                      : "Configure a test-token contract first"}
                  </small>
                </div>
              </button>
              <button
                className="wallet-action"
                onClick={() => setConfiguring(true)}
                type="button"
              >
                <span>⚙</span>
                <div>
                  <strong>Wallet configuration</strong>
                  <small>Address and test-token contract</small>
                </div>
              </button>
            </article>
          </section>
        </>
      ) : (
        <article className="panel wallet-empty">
          <EmptyPanel
            title="Connect a watch-only treasury"
            body="Add a public EVM address to view Horizen testnet balances. OSCIRIS never requests your seed phrase or private key."
            action={
              <button
                className="primary-button"
                onClick={() => setConfiguring(true)}
                type="button"
              >
                Configure wallet
              </button>
            }
          />
        </article>
      )}

      {configuring ? (
        <div className="modal-backdrop">
          <form className="wallet-dialog" onSubmit={saveConfig}>
            <div className="composer-heading">
              <div>
                <span className="eyebrow">WATCH-ONLY WALLET</span>
                <h2>Configure treasury</h2>
                <p>No secret keys. Balance reads and unsigned payloads only.</p>
              </div>
              <button onClick={() => setConfiguring(false)} type="button">
                ×
              </button>
            </div>
            <label>
              Horizen testnet EVM address
              <input
                onChange={(event) =>
                  setConfig({ ...config, address: event.target.value })
                }
                pattern="0x[0-9a-fA-F]{40}"
                placeholder="0x…"
                required
                value={config.address}
              />
            </label>
            <label>
              OSCIRIS test-token contract
              <input
                onChange={(event) =>
                  setConfig({
                    ...config,
                    settlement_token_address: event.target.value,
                  })
                }
                pattern="0x[0-9a-fA-F]{40}"
                placeholder="Optional until deployed"
                value={config.settlement_token_address ?? ""}
              />
            </label>
            <div className="form-grid">
              <label>
                Symbol
                <input
                  maxLength={12}
                  onChange={(event) =>
                    setConfig({
                      ...config,
                      settlement_token_symbol: event.target.value,
                    })
                  }
                  required
                  value={config.settlement_token_symbol}
                />
              </label>
              <label>
                Decimals
                <input
                  max={36}
                  min={0}
                  onChange={(event) =>
                    setConfig({
                      ...config,
                      settlement_token_decimals: Number(event.target.value),
                    })
                  }
                  type="number"
                  value={config.settlement_token_decimals}
                />
              </label>
            </div>
            <div className="composer-actions">
              <button
                className="text-button"
                onClick={() => setConfiguring(false)}
                type="button"
              >
                Cancel
              </button>
              <button className="primary-button" disabled={busy} type="submit">
                Save and sync
              </button>
            </div>
          </form>
        </div>
      ) : null}

      {withdrawing ? (
        <div className="modal-backdrop">
          <form className="wallet-dialog" onSubmit={prepare}>
            <div className="composer-heading">
              <div>
                <span className="eyebrow">EXTERNAL SIGNING</span>
                <h2>Prepare withdrawal</h2>
                <p>Generate ERC-20 calldata for review in your wallet.</p>
              </div>
              <button onClick={() => setWithdrawing(false)} type="button">
                ×
              </button>
            </div>
            <label>
              Recipient
              <input
                onChange={(event) =>
                  setWithdraw({ ...withdraw, recipient: event.target.value })
                }
                pattern="0x[0-9a-fA-F]{40}"
                placeholder="0x…"
                required
                value={withdraw.recipient}
              />
            </label>
            <label>
              Amount in atomic units
              <input
                min="1"
                onChange={(event) =>
                  setWithdraw({ ...withdraw, amount_atomic: event.target.value })
                }
                required
                type="number"
                value={withdraw.amount_atomic}
              />
            </label>
            <div className="composer-note">
              <strong>This does not move funds.</strong>
              <span>
                OSCIRIS generates a payload. You must verify and sign it in an
                external wallet connected to chain {wallet?.chain_id}.
              </span>
            </div>
            <div className="composer-actions">
              <button
                className="text-button"
                onClick={() => setWithdrawing(false)}
                type="button"
              >
                Cancel
              </button>
              <button className="primary-button" disabled={busy} type="submit">
                Prepare payload
              </button>
            </div>
          </form>
        </div>
      ) : null}

      {withdrawal ? (
        <div className="modal-backdrop">
          <div className="wallet-dialog transaction-dialog">
            <div className="composer-heading">
              <div>
                <span className="eyebrow">UNSIGNED TRANSACTION</span>
                <h2>Review in external wallet</h2>
                <p>{withdrawal.signing_instruction}</p>
              </div>
              <button onClick={onClearWithdrawal} type="button">
                ×
              </button>
            </div>
            <dl className="definition-list mono-values">
              <div>
                <dt>Chain</dt>
                <dd>{withdrawal.chain_id}</dd>
              </div>
              <div>
                <dt>From</dt>
                <dd>{withdrawal.from}</dd>
              </div>
              <div>
                <dt>Token contract</dt>
                <dd>{withdrawal.to}</dd>
              </div>
              <div>
                <dt>Amount</dt>
                <dd>
                  {withdrawal.amount_atomic} {withdrawal.symbol}
                </dd>
              </div>
              <div>
                <dt>Calldata</dt>
                <dd>{withdrawal.data}</dd>
              </div>
            </dl>
            <button
              className="primary-button full"
              onClick={() =>
                void navigator.clipboard.writeText(
                  JSON.stringify(withdrawal, null, 2),
                )
              }
              type="button"
            >
              Copy transaction JSON
            </button>
          </div>
        </div>
      ) : null}
    </section>
  );
}

export function NodeView({
  status,
  daemonLive,
  busy,
  onLaunch,
  onParticipation,
}: {
  status: DaemonStatus | null;
  daemonLive: boolean;
  busy: boolean;
  onLaunch: () => void;
  onParticipation: (enabled: boolean) => void;
}) {
  const participating = status?.participation_enabled ?? false;
  return (
    <section className="workspace-section">
      <div className="node-hero">
        <div>
          <span className="eyebrow">PROVIDER-LOCAL EXECUTION</span>
          <h2>
            {daemonLive
              ? participating
                ? "Your node is available."
                : "Participation is paused."
              : "Start the OSCIRIS daemon."}
          </h2>
          <p>
            Models and workloads execute on participant machines. The daemon
            owns identity, capability, jobs, and receipts outside the webview.
          </p>
          {!daemonLive ? (
            <button
              className="primary-button"
              disabled={busy}
              onClick={onLaunch}
              type="button"
            >
              Start local daemon
            </button>
          ) : (
            <button
              className={participating ? "secondary-button" : "primary-button"}
              disabled={busy}
              onClick={() => onParticipation(!participating)}
              type="button"
            >
              {participating ? "Pause participation" : "Enable participation"}
            </button>
          )}
        </div>
        <div className={daemonLive ? "node-orbit live" : "node-orbit"}>
          <span />
          <span />
          <strong>OS</strong>
        </div>
      </div>
      <section className="kpi-grid node-kpis">
        <article>
          <span>Daemon</span>
          <strong>{status ? `v${status.daemon_version}` : "Offline"}</strong>
          <small>Authenticated local IPC</small>
        </article>
        <article>
          <span>Platform</span>
          <strong>
            {status
              ? `${titleCase(status.platform.operating_system)}`
              : "Unknown"}
          </strong>
          <small>{status?.platform.architecture ?? "Awaiting daemon"}</small>
        </article>
        <article>
          <span>Active jobs</span>
          <strong>{status?.active_jobs ?? "—"}</strong>
          <small>Measured daemon state</small>
        </article>
        <article>
          <span>Network</span>
          <strong>
            {status ? titleCase(status.network_state) : "Not connected"}
          </strong>
          <small>Peer integration boundary</small>
        </article>
      </section>
      <div className="boundary-callout">
        <strong>Next operator integration</strong>
        <span>
          Signed identity, accelerator discovery, model profiles, peer bootstrap,
          and provider job execution will attach to this daemon surface.
        </span>
      </div>
    </section>
  );
}
