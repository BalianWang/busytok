import { cleanup, render, screen, waitFor, act } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { afterEach, beforeEach, describe, expect, it, vi, type MockInstance } from "vitest";
import type {
  PromptCreateRequestDto,
  PromptEntryDto,
  PromptListResponseDto,
  ReadEnvelopeDto,
} from "@busytok/protocol-types";
import { busytokClient } from "../api/busytokClient";
import { PageToolbarProvider, usePageToolbar } from "../components/desktop/PageToolbarContext";
import { PromptPalettePage } from "./PromptPalettePage";

const { mockHideWindow, mockInvoke, mockSetFocusWindow, mockShowWindow } = vi.hoisted(() => ({
  mockHideWindow: vi.fn(() => Promise.resolve()),
  mockInvoke: vi.fn(),
  mockSetFocusWindow: vi.fn(() => Promise.resolve()),
  mockShowWindow: vi.fn(() => Promise.resolve()),
}));

vi.mock("../logging/reporter", () => ({
  reportFrontendEvent: vi.fn(),
}));

vi.mock("@tauri-apps/api/window", () => ({
  getCurrentWindow: vi.fn(() => ({
    hide: mockHideWindow,
    setFocus: mockSetFocusWindow,
    show: mockShowWindow,
  })),
}));

vi.mock("@tauri-apps/api/core", () => ({
  invoke: mockInvoke,
}));

let clipboardWriteMock: MockInstance<(data: string) => Promise<void>>;

function envelope<T>(data: T): ReadEnvelopeDto<T> {
  return {
    data,
    generated_at_ms: 1716000000000,
    generation_id: "gen-1",
    readiness: "ready_exact",
    is_exact: true,
    is_stale: false,
    watermark_ms: null,
    progress: null,
    degraded_reason: null,
  };
}

function makePrompt(overrides: Partial<PromptEntryDto> = {}): PromptEntryDto {
  return {
    id: "prompt-1",
    content: "Create focused tests before changing the implementation.",
    alias: "tests",
    tags: ["testing", "quality"],
    is_pinned: true,
    usage_count: 7,
    last_used_at_ms: Date.now() - 5 * 60_000,
    created_at_ms: 1715900000000,
    updated_at_ms: 1716000000000,
    ...overrides,
  };
}

function renderPage() {
  const queryClient = new QueryClient({
    defaultOptions: { queries: { retry: false }, mutations: { retry: false } },
  });

  function ToolbarProbe() {
    const context = usePageToolbar();
    return <>{context?.toolbar ?? null}</>;
  }

  return render(
    <QueryClientProvider client={queryClient}>
      <PageToolbarProvider>
        <PromptPalettePage />
        <ToolbarProbe />
      </PageToolbarProvider>
    </QueryClientProvider>,
  );
}

function deferred<T>() {
  let resolve!: (value: T) => void;
  let reject!: (error: unknown) => void;
  const promise = new Promise<T>((res, rej) => {
    resolve = res;
    reject = rej;
  });
  return { promise, reject, resolve };
}

describe("PromptPalettePage", () => {
  beforeEach(() => {
    mockHideWindow.mockClear();
    mockSetFocusWindow.mockClear();
    mockShowWindow.mockClear();
    mockInvoke.mockReset();
    mockInvoke.mockImplementation((command: string) => {
      if (command === "prompt_palette_accessibility_status") {
        return Promise.resolve({ ok: true, failure_reason: null });
      }
      if (command === "prompt_palette_paste_active_app") {
        return Promise.resolve({ ok: true, failure_reason: null });
      }
      return Promise.reject(new Error(`unexpected command ${command}`));
    });
    if (!navigator.clipboard) {
      Object.defineProperty(navigator, "clipboard", {
        value: { writeText: async () => undefined },
        configurable: true,
      });
    }
    clipboardWriteMock = vi.spyOn(navigator.clipboard, "writeText").mockResolvedValue(undefined);
    vi.spyOn(busytokClient, "promptsList").mockResolvedValue(
      envelope<PromptListResponseDto>({
        entries: [
          makePrompt(),
          makePrompt({
            id: "prompt-2",
            content: "Condense notes into a clear timeline and action list.",
            alias: null,
            tags: ["ops"],
            is_pinned: false,
            usage_count: 2,
          }),
        ],
        total_count: 2,
      }),
    );
    vi.spyOn(busytokClient, "promptsUse").mockResolvedValue({
      usage_count: 8,
      last_used_at_ms: 1716000000000,
    });
    vi.spyOn(busytokClient, "promptsCreate").mockImplementation(
      async (request: PromptCreateRequestDto) =>
        envelope<PromptEntryDto>({
          ...makePrompt({
            id: "created-prompt",
            content: request.content,
            alias: request.alias,
            tags: request.tags,
            is_pinned: false,
          }),
        }),
    );
    vi.spyOn(busytokClient, "promptsUpdate").mockImplementation(async (request) =>
      envelope<PromptEntryDto>({
        ...makePrompt({
          id: request.id,
          content: request.content ?? "Updated content",
          alias: request.alias ?? null,
          tags: request.tags ?? [],
          is_pinned: request.is_pinned ?? false,
        }),
      }),
    );
    vi.spyOn(busytokClient, "promptsDelete").mockResolvedValue({ deleted: true });
    vi.spyOn(busytokClient, "promptsSuggestTags").mockResolvedValue({
      tags: ["ops", "testing"],
    });
  });

  afterEach(() => {
    cleanup();
    vi.restoreAllMocks();
  });

  it("renders prompt entries with alias: content headline and meta", async () => {
    renderPage();

    // Entry with alias "tests" should show "tests: Create focused tests..."
    expect(await screen.findByText(/tests: Create focused tests/i)).toBeDefined();
    // Entry without alias shows content directly
    expect(screen.getByText("Condense notes into a clear timeline and action list.")).toBeDefined();
    expect(screen.getByText("7 uses")).toBeDefined();
    expect(screen.getAllByText(/Last used/i).length).toBeGreaterThan(0);
    expect(screen.getAllByText(/Updated/i).length).toBeGreaterThan(0);
  });

  it("keeps the prompt title readable while row actions remain independently accessible", async () => {
    renderPage();

    // Title is rendered as a heading so it is the scan-primary signal
    expect(await screen.findByRole("heading", { name: /tests: Create focused tests/i })).toBeDefined();

    // Row actions stay accessible as independently labeled buttons
    expect(screen.getAllByRole("button", { name: "Copy" }).length).toBeGreaterThan(0);

    // Usage metadata stays subordinate to the title but still present
    expect(screen.getByText(/7 uses/i)).toBeDefined();
  });

  it("renders tag chips as neutral material rather than accent categorization", async () => {
    renderPage();

    await screen.findByText(/tests: Create focused tests/i);

    const tags = document.querySelectorAll(".prompt-tag");
    expect(tags.length).toBeGreaterThan(0);

    // Tag chips render as the neutral .prompt-tag base. Accent
    // categorization on tags would re-introduce a second palette.
    for (const tag of tags) {
      expect(tag.classList.contains("prompt-tag")).toBe(true);
      expect(tag.classList.contains("prompt-tag--neutral")).toBe(false);
    }
  });

  it("keeps row actions recessed by default and lifts them on hover/focus", async () => {
    renderPage();

    await screen.findByText(/tests: Create focused tests/i);

    const actions = document.querySelectorAll(".prompt-row__actions");
    expect(actions.length).toBeGreaterThan(0);

    // The recessed treatment is observable via the modifier class on the
    // actions container — full opacity only on hover/focus-within.
    for (const action of actions) {
      expect(action.classList.contains("prompt-row__actions--recessed")).toBe(true);
    }
  });

  it("renders inline tag chips for entries with tags", async () => {
    renderPage();

    await screen.findByText(/tests: Create focused tests/i);

    // First entry has "testing" and "quality" tags
    expect(screen.getByText("testing")).toBeDefined();
    expect(screen.getByText("quality")).toBeDefined();

    // Second entry has "ops" tag
    expect(screen.getByText("ops")).toBeDefined();
  });

  it("shows +N overflow when entry has more than 3 tags", async () => {
    vi.mocked(busytokClient.promptsList).mockResolvedValueOnce(
      envelope<PromptListResponseDto>({
        entries: [
          makePrompt({
            id: "prompt-many-tags",
            tags: ["alpha", "beta", "gamma", "delta", "epsilon"],
          }),
        ],
        total_count: 1,
      }),
    );
    renderPage();

    await screen.findByText("alpha");
    expect(screen.getByText("beta")).toBeDefined();
    expect(screen.getByText("gamma")).toBeDefined();
    expect(screen.getByText("+2")).toBeDefined();
    // 4th and 5th tags should NOT be rendered as individual chips
    expect(screen.queryByText("delta")).toBeNull();
    expect(screen.queryByText("epsilon")).toBeNull();
  });

  it("does not render old-style preview body, Use/Paste buttons, or Pinned badge", async () => {
    renderPage();

    await screen.findByText(/tests: Create focused tests/i);

    // No "Use" / "Paste" buttons
    expect(screen.queryByRole("button", { name: "Use" })).toBeNull();
    expect(screen.queryByRole("button", { name: "Paste" })).toBeNull();

    // No "Copy default" / "Paste default" labels
    expect(screen.queryByText(/Copy default/)).toBeNull();
    expect(screen.queryByText(/Paste default/)).toBeNull();

    // No "Pinned" badge text
    expect(screen.queryByText("Pinned")).toBeNull();
  });

  it("degrades gracefully for entries without alias or tags", async () => {
    vi.mocked(busytokClient.promptsList).mockResolvedValueOnce(
      envelope<PromptListResponseDto>({
        entries: [
          makePrompt({
            id: "prompt-bare",
            alias: null,
            tags: [],
            content: "Just a plain prompt body.",
          }),
        ],
        total_count: 1,
      }),
    );
    renderPage();

    // No alias → shows content directly as headline
    expect(await screen.findByText("Just a plain prompt body.")).toBeDefined();
    // No tag chips rendered at all (tags now live inside .prompt-row__meta as .prompt-tag spans)
    const tagChips = document.querySelectorAll(".prompt-tag");
    expect(tagChips.length).toBe(0);
  });

  it("renders tags inside the meta row alongside usage and updated labels", async () => {
    vi.mocked(busytokClient.promptsList).mockResolvedValueOnce(
      envelope<PromptListResponseDto>({
        entries: [
          makePrompt({
            id: "with-tags",
            alias: "tagged",
            content: "body",
            tags: ["alpha", "beta", "gamma", "delta"],
          }),
        ],
        total_count: 1,
      }),
    );
    renderPage();

    const meta = await screen.findByText("7 uses").then((el) => el.closest(".prompt-row__meta"));
    expect(meta).not.toBeNull();

    // Tags are now siblings of the metadata spans inside the same .prompt-row__meta container
    const tagsInMeta = meta?.querySelectorAll(".prompt-tag");
    expect(tagsInMeta?.length).toBe(3); // capped at 3
    expect(meta?.textContent).toContain("+1"); // 4 - 3 overflow
    expect(meta?.textContent).not.toContain("delta");

    // No separate tags row exists anymore
    expect(document.querySelectorAll(".prompt-row__tags").length).toBe(0);
  });

  it("shows loading, empty, and retryable error states", async () => {
    const pending = deferred<ReadEnvelopeDto<PromptListResponseDto>>();
    vi.mocked(busytokClient.promptsList).mockReturnValueOnce(pending.promise);
    const { unmount } = renderPage();

    expect(await screen.findByText("Loading prompts")).toBeDefined();

    unmount();
    vi.mocked(busytokClient.promptsList).mockResolvedValueOnce(
      envelope<PromptListResponseDto>({ entries: [], total_count: 0 }),
    );
    renderPage();
    expect(await screen.findByText("No prompts")).toBeDefined();

    cleanup();
    vi.mocked(busytokClient.promptsList).mockRejectedValueOnce(new Error("offline"));
    renderPage();
    expect(await screen.findByText("Prompt Palette unavailable")).toBeDefined();
    const callsBeforeRetry = vi.mocked(busytokClient.promptsList).mock.calls.length;
    await userEvent.click(screen.getByRole("button", { name: "Retry" }));
    await waitFor(() =>
      expect(vi.mocked(busytokClient.promptsList).mock.calls.length).toBeGreaterThan(
        callsBeforeRetry,
      ),
    );
  });

  it("searchbox triggers list calls with the entered query", async () => {
    const user = userEvent.setup();
    renderPage();

    await screen.findByText(/tests: Create focused tests/i);
    await user.type(screen.getByRole("searchbox", { name: "Search prompts" }), "tests");

    await waitFor(() => {
      expect(busytokClient.promptsList).toHaveBeenCalledWith({
        query: "tests",
        tag: null,
        sort: "pinned_first",
        limit: 100,
      });
    });
  });

  it("disables native text assistance on the management searchbox", async () => {
    renderPage();

    const input = await screen.findByRole("searchbox", { name: "Search prompts" }) as HTMLInputElement;
    expect(input.getAttribute("autocomplete")).toBe("off");
    expect(input.getAttribute("autocorrect")).toBe("off");
    expect(input.getAttribute("autocapitalize")).toBe("off");
    expect(input.getAttribute("data-form-type")).toBe("other");
    expect(input.getAttribute("spellcheck")).toBe("false");
  });

  it("defaults management list calls to pinned first sorting", async () => {
    renderPage();

    await waitFor(() => {
      expect(busytokClient.promptsList).toHaveBeenCalledWith({
        query: null,
        tag: null,
        sort: "pinned_first",
        limit: 100,
      });
    });
  });

  it("creates a prompt from the new prompt dialog", async () => {
    const user = userEvent.setup();
    renderPage();

    await screen.findByText(/tests: Create focused tests/i);
    await user.click(screen.getByRole("button", { name: "New Prompt" }));
    await user.type(screen.getByLabelText("Content"), "Summarize the release plan.");
    await user.type(screen.getByLabelText("Alias"), "draft-launch");
    await user.type(screen.getByLabelText("Tags"), "release, launch");
    await user.click(screen.getByRole("button", { name: "Save" }));

    await waitFor(() => {
      expect(busytokClient.promptsCreate).toHaveBeenCalledWith({
        content: "Summarize the release plan.",
        alias: "draft-launch",
        tags: ["release", "launch"],
      });
    });
    expect(screen.queryByRole("dialog", { name: "New Prompt" })).toBeNull();
  });

  it("edits and pins prompt entries via icon buttons", async () => {
    const user = userEvent.setup();
    renderPage();

    await screen.findByText(/tests: Create focused tests/i);

    // Edit
    await user.click(screen.getAllByRole("button", { name: "Edit" })[0]);
    await user.clear(screen.getByLabelText("Content"));
    await user.type(screen.getByLabelText("Content"), "Review release checklist");
    await user.click(screen.getByRole("button", { name: "Save" }));

    await waitFor(() => {
      expect(busytokClient.promptsUpdate).toHaveBeenCalledWith(
        expect.objectContaining({
          id: "prompt-1",
          content: "Review release checklist",
        }),
      );
    });

    // Unpin (pinned entry shows "Unpin")
    await user.click(screen.getAllByRole("button", { name: "Unpin" })[0]);
    await waitFor(() => {
      expect(busytokClient.promptsUpdate).toHaveBeenCalledWith(
        expect.objectContaining({
          id: "prompt-1",
          is_pinned: false,
        }),
      );
    });
  });

  it("does not delete immediately when clicking the delete icon", async () => {
    const user = userEvent.setup();
    renderPage();

    await screen.findByText(/tests: Create focused tests/i);
    await user.click(screen.getAllByRole("button", { name: "Delete" })[0]);

    // Mutation must NOT have been called yet
    expect(busytokClient.promptsDelete).not.toHaveBeenCalled();
  });

  it("opens a confirmation dialog when clicking delete", async () => {
    const user = userEvent.setup();
    renderPage();

    await screen.findByText(/tests: Create focused tests/i);
    await user.click(screen.getAllByRole("button", { name: "Delete" })[0]);

    expect(screen.getByText("Delete Prompt?")).toBeDefined();
    expect(screen.getByText(/permanently remove/)).toBeDefined();
  });

  it("cancels delete without calling mutation", async () => {
    const { reportFrontendEvent } = await import("../logging/reporter");
    const user = userEvent.setup();
    renderPage();

    await screen.findByText(/tests: Create focused tests/i);
    await user.click(screen.getAllByRole("button", { name: "Delete" })[0]);
    await user.click(screen.getByRole("button", { name: "Cancel" }));

    expect(busytokClient.promptsDelete).not.toHaveBeenCalled();
    expect(screen.queryByText("Delete Prompt?")).toBeNull();

    // Regression guard: onCancel must fire exactly once (not twice via
    // both Dialog.Close onOpenChange and button onClick).
    const cancelCalls = vi.mocked(reportFrontendEvent).mock.calls.filter(
      (c) => c[0].event_code === "gui.prompt.delete_confirm_cancelled",
    );
    expect(cancelCalls).toHaveLength(1);
  });

  it("deletes only after confirming in the dialog", async () => {
    const user = userEvent.setup();
    renderPage();

    await screen.findByText(/tests: Create focused tests/i);
    await user.click(screen.getAllByRole("button", { name: "Delete" })[0]);
    await user.click(screen.getByRole("button", { name: "Delete Prompt" }));

    await waitFor(() => {
      expect(busytokClient.promptsDelete).toHaveBeenCalledWith({ id: "prompt-1" });
    });
    // Dialog closes on success
    expect(screen.queryByText("Delete Prompt?")).toBeNull();
  });

  it("keeps dialog open and shows error when delete fails", async () => {
    const user = userEvent.setup();
    vi.mocked(busytokClient.promptsDelete).mockRejectedValueOnce(new Error("server error"));
    renderPage();

    await screen.findByText(/tests: Create focused tests/i);
    await user.click(screen.getAllByRole("button", { name: "Delete" })[0]);
    await user.click(screen.getByRole("button", { name: "Delete Prompt" }));

    expect(await screen.findByText("server error")).toBeDefined();
    expect(screen.getByText("Delete Prompt?")).toBeDefined();
  });

  it("disables buttons during delete and prevents double-submit", async () => {
    const user = userEvent.setup();
    const pending = deferred<{ deleted: boolean }>();
    vi.mocked(busytokClient.promptsDelete).mockReturnValueOnce(pending.promise);
    renderPage();

    await screen.findByText(/tests: Create focused tests/i);
    await user.click(screen.getAllByRole("button", { name: "Delete" })[0]);
    await user.click(screen.getByRole("button", { name: "Delete Prompt" }));

    const deleteBtn = await screen.findByRole("button", { name: "Deleting…" });
    const cancelBtn = screen.getByRole("button", { name: "Cancel" });
    expect(deleteBtn.hasAttribute("disabled")).toBe(true);
    expect(cancelBtn.hasAttribute("disabled")).toBe(true);

    expect(busytokClient.promptsDelete).toHaveBeenCalledTimes(1);

    pending.resolve({ deleted: true });
    await waitFor(() => {
      expect(screen.queryByText("Delete Prompt?")).toBeNull();
    });
  });

  it("reports frontend events for the delete confirmation flow", async () => {
    const { reportFrontendEvent } = await import("../logging/reporter");
    const user = userEvent.setup();
    renderPage();

    await screen.findByText(/tests: Create focused tests/i);

    // Open dialog
    await user.click(screen.getAllByRole("button", { name: "Delete" })[0]);
    expect(reportFrontendEvent).toHaveBeenCalledWith(
      expect.objectContaining({ event_code: "gui.prompt.delete_confirm_opened" }),
    );

    vi.mocked(reportFrontendEvent).mockClear();

    // Cancel
    await user.click(screen.getByRole("button", { name: "Cancel" }));
    expect(reportFrontendEvent).toHaveBeenCalledWith(
      expect.objectContaining({ event_code: "gui.prompt.delete_confirm_cancelled" }),
    );

    vi.mocked(reportFrontendEvent).mockClear();

    // Reopen and confirm
    await user.click(screen.getAllByRole("button", { name: "Delete" })[0]);
    await user.click(screen.getByRole("button", { name: "Delete Prompt" }));
    expect(reportFrontendEvent).toHaveBeenCalledWith(
      expect.objectContaining({ event_code: "gui.prompt.delete_confirmed" }),
    );
  });

  it("reports delete_failed event on mutation error", async () => {
    const { reportFrontendEvent } = await import("../logging/reporter");
    const user = userEvent.setup();
    vi.mocked(busytokClient.promptsDelete).mockRejectedValueOnce(new Error("network"));
    renderPage();

    await screen.findByText(/tests: Create focused tests/i);
    await user.click(screen.getAllByRole("button", { name: "Delete" })[0]);
    await user.click(screen.getByRole("button", { name: "Delete Prompt" }));

    await waitFor(() => {
      expect(reportFrontendEvent).toHaveBeenCalledWith(
        expect.objectContaining({
          event_code: "gui.prompt.delete_failed",
          level: "ERROR",
        }),
      );
    });
  });

  it("copies prompt content via icon button and records use event", async () => {
    const user = userEvent.setup();
    vi.mocked(busytokClient.promptsUse).mockResolvedValue({ usage_count: 7, last_used_at_ms: 1716000000000 });
    renderPage();

    await screen.findByText(/tests: Create focused tests/i);
    await user.click(screen.getAllByRole("button", { name: "Copy" })[0]);

    await waitFor(() => {
      expect(busytokClient.promptsUse).toHaveBeenCalledWith({
        id: "prompt-1",
        action: "copy",
        surface: "page",
        outcome: "copy",
        failure_reason: null,
      });
    });
    expect(clipboardWriteMock).toHaveBeenCalledWith(
      "Create focused tests before changing the implementation.",
    );
  });

  it("shows local Copied feedback after successful copy", async () => {
    const user = userEvent.setup();
    vi.mocked(busytokClient.promptsUse).mockResolvedValue({ usage_count: 7, last_used_at_ms: 1716000000000 });
    renderPage();

    await screen.findByText(/tests: Create focused tests/i);
    await user.click(screen.getAllByRole("button", { name: "Copy" })[0]);

    // Copy button should get the success styling (checkmark icon via --success class)
    await waitFor(() => {
      const copyBtn = screen.getAllByRole("button", { name: "Copy" })[0];
      expect(copyBtn.classList.contains("prompt-row__icon-btn--success")).toBe(true);
    });
  });

  it("copy does not disable other action icons on the same row", async () => {
    const user = userEvent.setup();
    // Make promptsUse hang so we can check mid-flight state
    const pending = deferred<{ usage_count: number; last_used_at_ms: number }>();
    vi.mocked(busytokClient.promptsUse).mockReturnValueOnce(pending.promise);
    renderPage();

    await screen.findByText(/tests: Create focused tests/i);
    await user.click(screen.getAllByRole("button", { name: "Copy" })[0]);

    // While copy is in-flight, Edit/Pin/Delete buttons should NOT be disabled
    const editBtns = screen.getAllByRole("button", { name: "Edit" });
    const deleteBtns = screen.getAllByRole("button", { name: "Delete" });
    for (const btn of [...editBtns, ...deleteBtns]) {
      expect(btn.hasAttribute("disabled")).toBe(false);
    }

    // Clean up the pending promise
    pending.resolve({ usage_count: 8, last_used_at_ms: 1716000000000 });
  });

  it("copy does not disable action icons on other rows", async () => {
    const user = userEvent.setup();
    const pending = deferred<{ usage_count: number; last_used_at_ms: number }>();
    vi.mocked(busytokClient.promptsUse).mockReturnValueOnce(pending.promise);
    renderPage();

    await screen.findByText(/tests: Create focused tests/i);

    // Copy the second entry
    const copyBtns = screen.getAllByRole("button", { name: "Copy" });
    await user.click(copyBtns[1]);

    // First row's buttons should all be enabled
    const firstRowEdit = screen.getAllByRole("button", { name: "Edit" })[0];
    const firstRowDelete = screen.getAllByRole("button", { name: "Delete" })[0];
    expect(firstRowEdit.hasAttribute("disabled")).toBe(false);
    expect(firstRowDelete.hasAttribute("disabled")).toBe(false);

    pending.resolve({ usage_count: 8, last_used_at_ms: 1716000000000 });
  });

  it("pin does not disable action icons across the list while update is in flight", async () => {
    const user = userEvent.setup();
    const pending = deferred<ReadEnvelopeDto<PromptEntryDto>>();
    vi.mocked(busytokClient.promptsUpdate).mockReturnValueOnce(pending.promise);
    renderPage();

    await screen.findByText(/tests: Create focused tests/i);
    await user.click(screen.getAllByRole("button", { name: "Unpin" })[0]);

    for (const button of screen.getAllByRole("button")) {
      const label = button.getAttribute("aria-label");
      if (label === "Copy" || label === "Edit" || label === "Delete" || label === "Unpin" || label === "Pin") {
        expect(button.hasAttribute("disabled")).toBe(false);
      }
    }

    pending.resolve(
      envelope<PromptEntryDto>(
        makePrompt({
          id: "prompt-1",
          is_pinned: false,
        }),
      ),
    );
  });

  it("blocks same-row edit while pin is in flight but keeps other rows editable", async () => {
    const user = userEvent.setup();
    const pending = deferred<ReadEnvelopeDto<PromptEntryDto>>();
    vi.mocked(busytokClient.promptsUpdate).mockReturnValueOnce(pending.promise);
    renderPage();

    await screen.findByText(/tests: Create focused tests/i);
    await user.click(screen.getAllByRole("button", { name: "Unpin" })[0]);
    await user.click(screen.getAllByRole("button", { name: "Edit" })[0]);

    expect(screen.queryByRole("dialog", { name: "Edit Prompt" })).toBeNull();

    await user.click(screen.getAllByRole("button", { name: "Edit" })[1]);
    expect(await screen.findByRole("dialog", { name: "Edit Prompt" })).toBeDefined();

    pending.resolve(
      envelope<PromptEntryDto>(
        makePrompt({
          id: "prompt-1",
          is_pinned: false,
        }),
      ),
    );
  });

  it("shows sanitized action feedback when prompt use tracking fails", async () => {
    const user = userEvent.setup();
    const secret = "database error with private prompt text";
    vi.mocked(busytokClient.promptsUse).mockRejectedValue(new Error(secret));
    renderPage();

    await screen.findByText(/tests: Create focused tests/i);
    await user.click(screen.getAllByRole("button", { name: "Copy" })[0]);

    expect(await screen.findByText("Could not run prompt action. Try again.")).toBeDefined();
    expect(screen.queryByText(secret)).toBeNull();
  });

  it("icon buttons have aria-labels for accessibility", async () => {
    renderPage();

    await screen.findByText(/tests: Create focused tests/i);

    const prompt1Buttons = screen.getAllByRole("button").filter(
      (btn) =>
        btn.getAttribute("aria-label") === "Copy" ||
        btn.getAttribute("aria-label") === "Edit" ||
        btn.getAttribute("aria-label") === "Unpin" ||
        btn.getAttribute("aria-label") === "Delete",
    );
    // First entry is pinned → shows "Unpin"
    expect(prompt1Buttons.length).toBeGreaterThanOrEqual(4);

    // Second entry is not pinned → shows "Pin"
    expect(screen.getAllByRole("button", { name: "Pin" }).length).toBeGreaterThan(0);
  });

  it("copy tooltip shows on hover in idle state", async () => {
    const user = userEvent.setup();
    renderPage();

    await screen.findByText(/tests: Create focused tests/i);
    const copyBtn = screen.getAllByRole("button", { name: "Copy" })[0];

    // Before hover: no tooltip portals visible
    expect(document.querySelectorAll(".prompt-row__tooltip").length).toBe(0);

    // Hover over the copy button — tooltip portal should appear with "Copy"
    await user.hover(copyBtn);
    await waitFor(() => {
      expect(document.querySelectorAll(".prompt-row__tooltip").length).toBeGreaterThan(0);
    });
    const tooltip = document.querySelector(".prompt-row__tooltip");
    expect(tooltip?.textContent).toContain("Copy");
  });

  it("action buttons are enabled once the list loads regardless of subscription status", async () => {
    renderPage();

    await screen.findByText(/tests: Create focused tests/i);

    const actionLabels = ["Copy", "Edit", "Unpin", "Delete"];
    for (const label of actionLabels) {
      const btn = screen.getAllByRole("button", { name: label })[0];
      expect(btn).toBeDefined();
      expect(btn.hasAttribute("disabled")).toBe(false);
    }
  });

  it("sorts and refreshes via toolbar controls", async () => {
    const user = userEvent.setup();
    renderPage();

    await screen.findByText(/tests: Create focused tests/i);

    await user.click(screen.getByRole("combobox", { name: "Sort prompts" }));
    await user.click(await screen.findByRole("option", { name: "Most used" }));
    await waitFor(() => {
      expect(busytokClient.promptsList).toHaveBeenCalledWith({
        query: null,
        tag: null,
        sort: "most_used",
        limit: 100,
      });
    });

    await user.click(screen.getByRole("button", { name: "Refresh data" }));
    expect(busytokClient.promptsList).toHaveBeenCalled();
  });

  it("applies tag filter when a tag is selected from the combobox", async () => {
    vi.useFakeTimers({ shouldAdvanceTime: true });
    const user = userEvent.setup({ advanceTimers: vi.advanceTimersByTime });
    renderPage();

    await screen.findByText(/tests: Create focused tests/i);

    const tagInput = screen.getByRole("combobox", { name: "Filter by tag" });
    await user.type(tagInput, "te");
    act(() => { vi.advanceTimersByTime(200); });

    await waitFor(() => {
      expect(busytokClient.promptsSuggestTags).toHaveBeenCalledWith(
        expect.objectContaining({ query: "te" }),
      );
    });

    await user.click(await screen.findByRole("option", { name: "testing" }));

    await waitFor(() => {
      expect(busytokClient.promptsList).toHaveBeenCalledWith(
        expect.objectContaining({ tag: "testing" }),
      );
    });
    vi.useRealTimers();
  });

  it("typing in tag combobox does not trigger prompt list filtering until a tag is selected", async () => {
    vi.useFakeTimers({ shouldAdvanceTime: true });
    const user = userEvent.setup({ advanceTimers: vi.advanceTimersByTime });
    renderPage();

    await screen.findByText(/tests: Create focused tests/i);

    const callsBeforeTyping = vi.mocked(busytokClient.promptsList).mock.calls.length;

    const tagInput = screen.getByRole("combobox", { name: "Filter by tag" });
    await user.type(tagInput, "te");
    act(() => { vi.advanceTimersByTime(200); });

    await waitFor(() => {
      expect(busytokClient.promptsSuggestTags).toHaveBeenCalledWith(
        expect.objectContaining({ query: "te" }),
      );
    });

    const listCallsAfterTyping = vi.mocked(busytokClient.promptsList).mock.calls;
    expect(listCallsAfterTyping.length).toBe(callsBeforeTyping);
    expect(listCallsAfterTyping.every((call) => call[0].tag === null)).toBe(true);

    await user.click(await screen.findByRole("option", { name: "testing" }));

    await waitFor(() => {
      expect(busytokClient.promptsList).toHaveBeenCalledWith(
        expect.objectContaining({ tag: "testing" }),
      );
    });

    vi.useRealTimers();
  });

  // ── Tooltip controlled/uncontrolled regression ────────────────────

  it("does not warn about controlled/uncontrolled tooltip switch on copy success", async () => {
    const user = userEvent.setup();
    const errorSpy = vi.spyOn(console, "error").mockImplementation(() => {});
    vi.mocked(busytokClient.promptsUse).mockResolvedValue({ usage_count: 7, last_used_at_ms: 1716000000000 });
    renderPage();

    await screen.findByText(/tests: Create focused tests/i);
    await user.click(screen.getAllByRole("button", { name: "Copy" })[0]);

    await waitFor(() => {
      expect(clipboardWriteMock).toHaveBeenCalled();
    });

    const radixWarnings = errorSpy.mock.calls.filter(
      (call) => typeof call[0] === "string" && call[0].includes("uncontrolled to controlled"),
    );
    expect(radixWarnings).toHaveLength(0);
    errorSpy.mockRestore();
  });

  it("does not warn about controlled/uncontrolled tooltip switch on copy failure", async () => {
    const user = userEvent.setup();
    const errorSpy = vi.spyOn(console, "error").mockImplementation(() => {});
    vi.mocked(busytokClient.promptsUse).mockRejectedValue(new Error("fail"));
    renderPage();

    await screen.findByText(/tests: Create focused tests/i);
    await user.click(screen.getAllByRole("button", { name: "Copy" })[0]);

    expect(await screen.findByText("Could not run prompt action. Try again.")).toBeDefined();

    const radixWarnings = errorSpy.mock.calls.filter(
      (call) => typeof call[0] === "string" && call[0].includes("uncontrolled to controlled"),
    );
    expect(radixWarnings).toHaveLength(0);
    errorSpy.mockRestore();
  });
});
