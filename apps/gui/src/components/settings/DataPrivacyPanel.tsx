export function DataPrivacyPanel() {
  return (
    <section className="settings-panel page-surface" aria-label="Data and privacy">
      <div className="settings-panel__header">
        <h3>Data and privacy</h3>
        <p>Busytok is a local-first log audit tool. All usage data stays on this device and is derived solely from AI agent log files already present on disk.</p>
      </div>

      <div className="settings-panel__stack">
        <div className="settings-panel__info surface-inset">
          <strong>Stored on this device</strong>
          <p>Dashboard defaults, reminder preferences, and other consumer-facing settings stay in browser storage on this machine.</p>
        </div>
        <div className="settings-panel__info surface-inset">
          <strong>Usage history retention</strong>
          <p>Busytok reads and indexes log files that AI agents already write to disk. It does not intercept, proxy, or modify any network traffic. Retention follows the local database and log file lifecycle.</p>
        </div>
        <div className="settings-panel__info surface-inset">
          <strong>Privacy stance</strong>
          <p>No account sync, no cloud telemetry, no network interception. Busytok is an offline log reader that presents what your AI agents have already recorded locally.</p>
        </div>
      </div>
    </section>
  );
}