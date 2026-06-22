import { useState } from "react";
import { checkAndApplyUpdate, type UpdaterResult } from "../lib/updaterClient";

export type UpdaterStatus =
  | { state: "idle" }
  | { state: "checking" }
  | { state: "done"; result: UpdaterResult };

export interface UseUpdaterApi {
  status: UpdaterStatus;
  checkNow: () => Promise<void>;
}

export function useUpdater(): UseUpdaterApi {
  const [status, setStatus] = useState<UpdaterStatus>({ state: "idle" });
  const checkNow = async () => {
    setStatus({ state: "checking" });
    const result = await checkAndApplyUpdate();
    setStatus({ state: "done", result });
  };
  return { status, checkNow };
}
