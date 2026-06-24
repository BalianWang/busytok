import type {
  PromptActionDto,
  PromptCreateRequestDto,
  PromptEntryDto,
} from "@busytok/protocol-types";
import { formatRelativeTime, formatShortDate } from "./formatters";

function compactWhitespace(text: string): string {
  return text.replace(/\s+/g, " ").trim();
}

export function parsePromptTags(value: string): string[] {
  const seen = new Set<string>();
  const tags: string[] = [];
  for (const raw of value.split(",")) {
    const tag = raw.trim();
    const key = tag.toLowerCase();
    if (tag && !seen.has(key)) {
      seen.add(key);
      tags.push(tag);
    }
  }
  return tags;
}

export function promptUseCountLabel(usageCount: number): string {
  return `${usageCount} ${usageCount === 1 ? "use" : "uses"}`;
}

export function promptLastUsedLabel(lastUsedAtMs: number | null): string {
  return lastUsedAtMs == null ? "Not used yet" : `Last used ${formatRelativeTime(lastUsedAtMs)}`;
}

export function promptUpdatedLabel(updatedAtMs: number): string {
  return `Updated ${formatShortDate(updatedAtMs)}`;
}

export function promptActionLabel(action: PromptActionDto): string {
  switch (action) {
    case "OnlyCopy": return "Copy";
    case "OnlyPaste": return "Paste";
    case "CopyAndPaste": return "Paste";
  }
}

export function promptDisplayLabel(alias: string | null, content: string): string {
  const compact = compactWhitespace(content);
  return alias ? `${alias}: ${compact}` : compact;
}

export function promptDisplayTitle(alias: string | null, content: string, max = 80): string {
  if (alias) return alias;
  const compact = compactWhitespace(content);
  return compact.length > max ? `${compact.slice(0, max - 1)}…` : compact;
}

export function promptDisplayHeadline(alias: string | null, content: string, max = 120): string {
  const compact = compactWhitespace(content);
  if (!alias) {
    return compact.length > max ? `${compact.slice(0, max)}…` : compact;
  }
  const label = `${alias}: ${compact}`;
  return label.length > max ? `${label.slice(0, max)}…` : label;
}
