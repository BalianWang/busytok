import type { ReactNode } from "react";

export function SettingsRow({
  label,
  description,
  control,
  error,
  dangerous,
  layout = "horizontal",
}: {
  label: string;
  description?: string;
  control: ReactNode;
  error?: string | null;
  dangerous?: boolean;
  layout?: "horizontal" | "vertical";
}) {
  return (
    <div className={`settings-row${dangerous ? " settings-row--dangerous" : ""}`}>
      <div>
        <h3>{label}</h3>
        {description ? <p>{description}</p> : null}
      </div>
      <div className={`settings-row__control settings-row__control--${layout}`}>
        {control}
        {error ? <span className="settings-row__error">{error}</span> : null}
      </div>
    </div>
  );
}
