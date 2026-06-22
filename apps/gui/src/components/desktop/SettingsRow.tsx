import type { ReactNode } from "react";

export function SettingsRow({
  label,
  description,
  control,
  error,
  dangerous,
}: {
  label: string;
  description?: string;
  control: ReactNode;
  error?: string | null;
  dangerous?: boolean;
}) {
  return (
    <div className={`settings-row${dangerous ? " settings-row--dangerous" : ""}`}>
      <div>
        <h3>{label}</h3>
        {description ? <p>{description}</p> : null}
      </div>
      <div className="settings-row__control">
        {control}
        {error ? <span className="settings-row__error">{error}</span> : null}
      </div>
    </div>
  );
}
