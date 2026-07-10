/**
 * Shared helpers for provider creation/edit forms.
 * Used by ProvidersPage (GUI) — the CLI has a Rust mirror in
 * apps/cli/src/commands/provider.rs.
 */

/**
 * Safely extract a human-readable message from an unknown catch value.
 * Returns the fallback when the value is not an Error or has no message.
 */
export function errorMessage(error: unknown, fallback: string): string {
  if (error instanceof Error && error.message) return error.message;
  if (typeof error === "string" && error) return error;
  return fallback;
}

/** Split a comma-separated tag string into a clean string[]. */
export function parseTags(input: string): string[] {
  return input
    .split(",")
    .map((s) => s.trim())
    .filter((s) => s.length > 0);
}

/**
 * Derive a provider name from Base URL + Kind.
 * Format: `{domain}_{kindShort}` where domain is the second-to-last
 * hostname segment (e.g. "deepseek" from "api.deepseek.com") and
 * kindShort strips the `_compatible` suffix.
 */
export function deriveProviderName(url: string, kind: string): string {
  const host = new URL(url).hostname;
  const parts = host.split(".");
  const domain = parts[parts.length - 2] || host;
  const kindShort = kind.replace("_compatible", "");
  return `${domain}_${kindShort}`;
}

/**
 * Derive a provider name, appending `_2`, `_3`, ... on collision with
 * `existingNames` until unique.
 */
export function deriveUniqueProviderName(
  url: string,
  kind: string,
  existingNames: Set<string>,
): string {
  const base = deriveProviderName(url, kind);
  if (!existingNames.has(base)) return base;
  let i = 2;
  while (existingNames.has(`${base}_${i}`)) i++;
  return `${base}_${i}`;
}

/**
 * Validate a Base URL. Returns an error message string, or null if valid.
 * Checks: non-empty, starts with http:// or https://, parses as URL.
 */
export function validateBaseUrl(input: string): string | null {
  const trimmed = input.trim();
  if (!trimmed) return "Base URL 不能为空";
  if (!/^https?:\/\//.test(trimmed)) {
    return "请输入完整的 URL（以 http:// 或 https:// 开头）";
  }
  try {
    new URL(trimmed);
  } catch {
    return "URL 格式不正确";
  }
  return null;
}
