import { reportFrontendEvent } from "./reporter";

export type FrontendEventEntry = Parameters<typeof reportFrontendEvent>[0];

export function reportFrontendEventSafely(entry: FrontendEventEntry): void {
  try {
    reportFrontendEvent(entry);
  } catch {
    // Observability must never break the user interaction path.
  }
}
