//! statusAction — shared StatusActionDto → DesktopPage mapping. Unknown
//! actions return undefined so callers skip navigation rather than falling
//! back to a default page.
import type { StatusActionDto } from "@busytok/protocol-types";
import type { DesktopPage } from "../AppShell";

export function statusActionToPage(action: StatusActionDto): DesktopPage | undefined {
  switch (action) {
    case "open_activity": return "usage";
    case "open_settings": return "settings";
    default: return undefined;
  }
}
