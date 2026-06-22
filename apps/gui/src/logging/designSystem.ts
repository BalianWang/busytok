// apps/gui/src/logging/designSystem.ts

import { safeReportEvent } from "./reporter";

/**
 * Token-layer version tag for the Geist-inspired refactor. Emitted once per
 * bootstrap so that field-reported visual behavior can be correlated to the
 * active design-system contract. Bump this when a Phase lands.
 */
export const DESIGN_SYSTEM_VERSION = "geist-refactor-phase-1";

/**
 * Fire-and-forget marker that the design-system token layer is active.
 * Safe to call from the bootstrap path — never throws into app startup.
 */
export function reportDesignSystemApplied(): void {
  try {
    safeReportEvent(
      "gui.design_system.applied",
      "Design system token layer applied",
      { version: DESIGN_SYSTEM_VERSION },
    );
  } catch {
    // Observability must not break bootstrap.
  }
}
