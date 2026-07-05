import { describe, it, expect, vi, beforeEach, afterEach } from "vitest";
import { render, screen, fireEvent, waitFor, cleanup } from "@testing-library/react";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import type {
  ProfileDto,
  ReadEnvelopeDto,
  SettingsSnapshotDto,
} from "@busytok/protocol-types";

vi.mock("../api/useBusytokData", () => ({
  useSettingsSnapshot: vi.fn(),
  useProfileMutations: vi.fn(),
}));
vi.mock("../logging/safeReporter", () => ({
  reportFrontendEventSafely: vi.fn(),
}));

import {
  useSettingsSnapshot,
  useProfileMutations,
} from "../api/useBusytokData";
import { reportFrontendEventSafely } from "../logging/safeReporter";
import { ProfilesSection } from "./ProfilesSection";

const mockSnapshot = vi.mocked(useSettingsSnapshot);
const mockMutations = vi.mocked(useProfileMutations);

function makeProfile(overrides: Partial<ProfileDto> = {}): ProfileDto {
  return {
    id: "pi/search-cheap",
    is_builtin: true,
    tools: ["read", "grep"],
    context_budget_tokens: 3000,
    timeout_seconds: 120,
    write_access: false,
    ...overrides,
  };
}

function renderWithProviders(ui: React.ReactElement) {
  const queryClient = new QueryClient({
    defaultOptions: { queries: { retry: false }, mutations: { retry: false } },
  });
  return render(
    <QueryClientProvider client={queryClient}>{ui}</QueryClientProvider>,
  );
}

/** Build a snapshot return value with the given profiles. */
function snapshotWith(profiles: ProfileDto[]) {
  return {
    data: {
      data: { subagent: { enabled: true, profiles } },
    } as unknown as ReadEnvelopeDto<SettingsSnapshotDto>,
    isLoading: false,
    isError: false,
    isFetching: false,
  } as never;
}

/** Build a mutations return value with controllable mutate fns. */
function mutationsWith(overrides: {
  create?: { mutate: ReturnType<typeof vi.fn>; isPending?: boolean };
  update?: { mutate: ReturnType<typeof vi.fn>; isPending?: boolean };
  delete?: { mutate: ReturnType<typeof vi.fn>; isPending?: boolean };
} = {}) {
  return {
    createProfile: { mutate: overrides.create?.mutate ?? vi.fn(), isPending: overrides.create?.isPending ?? false },
    updateProfile: { mutate: overrides.update?.mutate ?? vi.fn(), isPending: overrides.update?.isPending ?? false },
    deleteProfile: { mutate: overrides.delete?.mutate ?? vi.fn(), isPending: overrides.delete?.isPending ?? false },
  } as never;
}

beforeEach(() => {
  vi.clearAllMocks();
  // Default: no profiles, no pending mutations.
  mockSnapshot.mockReturnValue(snapshotWith([]));
  mockMutations.mockReturnValue(mutationsWith());
});

afterEach(() => cleanup());

describe("ProfilesSection", () => {
  it("renders built-in profiles from settings snapshot", () => {
    const profiles = [
      makeProfile({ id: "pi/search-cheap", is_builtin: true }),
      makeProfile({ id: "pi/review-cheap", is_builtin: true }),
    ];
    mockSnapshot.mockReturnValue(snapshotWith(profiles));
    renderWithProviders(<ProfilesSection />);
    expect(screen.getByText("pi/search-cheap")).toBeTruthy();
    expect(screen.getByText("pi/review-cheap")).toBeTruthy();
  });

  it("renders advanced read-only fields (tools, budget, timeout, write_access)", () => {
    mockSnapshot.mockReturnValue(
      snapshotWith([
        makeProfile({
          id: "my-profile",
          is_builtin: false,
          tools: ["read", "grep", "glob"],
          context_budget_tokens: 8000,
          timeout_seconds: 60,
          write_access: true,
        }),
      ]),
    );
    renderWithProviders(<ProfilesSection />);
    expect(screen.getByText(/Tools: read, grep, glob/i)).toBeTruthy();
    expect(screen.getByText(/Budget: 8000 tokens/i)).toBeTruthy();
    expect(screen.getByText(/Timeout: 60s/i)).toBeTruthy();
    expect(screen.getByText(/Write access: yes/i)).toBeTruthy();
  });

  it("hides the Delete button for built-in profiles", () => {
    const profiles = [
      makeProfile({ id: "pi/search-cheap", is_builtin: true }),
      makeProfile({ id: "my-custom", is_builtin: false }),
    ];
    mockSnapshot.mockReturnValue(snapshotWith(profiles));

    renderWithProviders(<ProfilesSection />);
    const builtinRow = screen.getByText("pi/search-cheap").closest(".settings-panel");
    expect(builtinRow?.querySelector('button[class*="btn--danger"]')).toBeNull();
    const userRow = screen.getByText("my-custom").closest(".settings-panel");
    expect(userRow?.querySelector('button[class*="btn--danger"]')).not.toBeNull();
  });

  it("hides the Edit button for built-in profiles", () => {
    const profiles = [
      makeProfile({ id: "pi/search-cheap", is_builtin: true }),
      makeProfile({ id: "my-custom", is_builtin: false }),
    ];
    mockSnapshot.mockReturnValue(snapshotWith(profiles));

    renderWithProviders(<ProfilesSection />);
    // Builtin row: no Edit button
    const builtinRow = screen.getByText("pi/search-cheap").closest(".settings-panel");
    expect(builtinRow?.querySelector("button")).toBeNull();
    // User row: has an Edit button
    const userRow = screen.getByText("my-custom").closest(".settings-panel");
    expect(userRow?.querySelector('button[class*="btn--secondary"]')).not.toBeNull();
  });

  it("shows the Edit button on non-builtin profiles", () => {
    mockSnapshot.mockReturnValue(
      snapshotWith([makeProfile({ id: "my-custom", is_builtin: false })]),
    );
    renderWithProviders(<ProfilesSection />);
    expect(screen.getByRole("button", { name: /^edit$/i })).toBeTruthy();
  });

  it("clicking Edit shows the inline edit form with current values", () => {
    mockSnapshot.mockReturnValue(
      snapshotWith([
        makeProfile({
          id: "my-custom",
          is_builtin: false,
          tools: ["read", "grep"],
          context_budget_tokens: 5000,
          timeout_seconds: 90,
          write_access: true,
        }),
      ]),
    );
    renderWithProviders(<ProfilesSection />);
    fireEvent.click(screen.getByRole("button", { name: /^edit$/i }));
    // Edit form inputs are present with the current values pre-filled.
    const toolsInput = screen.getByLabelText(/tools for my-custom/i) as HTMLInputElement;
    expect(toolsInput.value).toBe("read, grep");
    const budgetInput = screen.getByLabelText(/context budget for my-custom/i) as HTMLInputElement;
    expect(budgetInput.value).toBe("5000");
    const timeoutInput = screen.getByLabelText(/timeout for my-custom/i) as HTMLInputElement;
    expect(timeoutInput.value).toBe("90");
    // Save + Cancel buttons appear in edit mode.
    expect(screen.getByRole("button", { name: /^save$/i })).toBeTruthy();
    expect(screen.getByRole("button", { name: /^cancel$/i })).toBeTruthy();
  });

  it("Save calls updateProfile.mutate with the correct payload", () => {
    const updateMutate = vi.fn();
    mockSnapshot.mockReturnValue(
      snapshotWith([
        makeProfile({
          id: "my-custom",
          is_builtin: false,
          tools: ["read"],
          context_budget_tokens: 3000,
          timeout_seconds: 120,
          write_access: false,
        }),
      ]),
    );
    mockMutations.mockReturnValue(mutationsWith({ update: { mutate: updateMutate } }));

    renderWithProviders(<ProfilesSection />);
    fireEvent.click(screen.getByRole("button", { name: /^edit$/i }));
    // Change the tools field.
    const toolsInput = screen.getByLabelText(/tools for my-custom/i);
    fireEvent.change(toolsInput, { target: { value: "read, write, bash" } });
    fireEvent.click(screen.getByRole("button", { name: /^save$/i }));

    expect(updateMutate).toHaveBeenCalledTimes(1);
    const [payload] = updateMutate.mock.calls[0]!;
    expect(payload).toEqual({
      id: "my-custom",
      tools: ["read", "write", "bash"],
      context_budget_tokens: 3000,
      timeout_seconds: 120,
      write_access: false,
    });
  });

  it("Cancel exits edit mode and returns to view", () => {
    mockSnapshot.mockReturnValue(
      snapshotWith([makeProfile({ id: "my-custom", is_builtin: false })]),
    );
    renderWithProviders(<ProfilesSection />);
    fireEvent.click(screen.getByRole("button", { name: /^edit$/i }));
    expect(screen.getByRole("button", { name: /^save$/i })).toBeTruthy();
    fireEvent.click(screen.getByRole("button", { name: /^cancel$/i }));
    // Back to view mode: Edit button visible, Save gone.
    expect(screen.getByRole("button", { name: /^edit$/i })).toBeTruthy();
    expect(screen.queryByRole("button", { name: /^save$/i })).toBeNull();
  });

  it("updateProfile onSuccess reports event and exits edit mode", async () => {
    const updateMutate = vi.fn(
      (_req: unknown, opts?: { onSuccess?: () => void }) => {
        opts?.onSuccess?.();
      },
    );
    mockSnapshot.mockReturnValue(
      snapshotWith([makeProfile({ id: "my-custom", is_builtin: false })]),
    );
    mockMutations.mockReturnValue(mutationsWith({ update: { mutate: updateMutate } }));

    renderWithProviders(<ProfilesSection />);
    fireEvent.click(screen.getByRole("button", { name: /^edit$/i }));
    fireEvent.click(screen.getByRole("button", { name: /^save$/i }));

    await waitFor(() => {
      expect(vi.mocked(reportFrontendEventSafely)).toHaveBeenCalledWith(
        expect.objectContaining({ event_code: "profile.updated" }),
      );
    });
    // Edit mode exited after successful save.
    expect(screen.queryByRole("button", { name: /^save$/i })).toBeNull();
  });

  it("updateProfile onError shows error + reports profile.update.failed", async () => {
    const updateMutate = vi.fn(
      (_req: unknown, opts?: { onError?: (err: Error) => void }) => {
        opts?.onError?.(new Error("update rejected"));
      },
    );
    mockSnapshot.mockReturnValue(
      snapshotWith([makeProfile({ id: "my-custom", is_builtin: false })]),
    );
    mockMutations.mockReturnValue(mutationsWith({ update: { mutate: updateMutate } }));

    renderWithProviders(<ProfilesSection />);
    fireEvent.click(screen.getByRole("button", { name: /^edit$/i }));
    fireEvent.click(screen.getByRole("button", { name: /^save$/i }));

    await waitFor(() => {
      expect(screen.getByText(/update rejected/i)).toBeTruthy();
    });
    expect(vi.mocked(reportFrontendEventSafely)).toHaveBeenCalledWith(
      expect.objectContaining({ event_code: "profile.update.failed" }),
    );
  });

  it("calls deleteProfile.mutate when Delete is clicked on a user profile", () => {
    const deleteMutate = vi.fn();
    mockSnapshot.mockReturnValue(
      snapshotWith([makeProfile({ id: "my-custom", is_builtin: false })]),
    );
    mockMutations.mockReturnValue(mutationsWith({ delete: { mutate: deleteMutate } }));

    renderWithProviders(<ProfilesSection />);
    fireEvent.click(screen.getByRole("button", { name: /^delete$/i }));
    expect(deleteMutate).toHaveBeenCalledWith("my-custom", expect.anything());
  });

  it("shows Deleting... and disables the Delete button while delete is pending", () => {
    mockSnapshot.mockReturnValue(
      snapshotWith([makeProfile({ id: "my-custom", is_builtin: false })]),
    );
    mockMutations.mockReturnValue(
      mutationsWith({ delete: { mutate: vi.fn(), isPending: true } }),
    );

    renderWithProviders(<ProfilesSection />);
    const deleteBtn = screen.getByRole("button", { name: /deleting/i });
    expect((deleteBtn as HTMLButtonElement).disabled).toBe(true);
  });

  it("shows loading state when snapshot is loading", () => {
    mockSnapshot.mockReturnValue({
      data: undefined,
      isLoading: true,
      isError: false,
      isFetching: false,
    } as never);
    renderWithProviders(<ProfilesSection />);
    expect(screen.getByText(/loading profiles/i)).toBeTruthy();
  });

  it("shows error state when snapshot fetch fails", () => {
    mockSnapshot.mockReturnValue({
      data: undefined,
      isLoading: false,
      isError: true,
      isFetching: false,
    } as never);
    renderWithProviders(<ProfilesSection />);
    expect(screen.getByText(/failed to load profiles/i)).toBeTruthy();
  });

  it("renders empty state when no profiles configured", () => {
    renderWithProviders(<ProfilesSection />);
    expect(screen.getByText(/no profiles configured/i)).toBeTruthy();
  });

  it("shows mutation error when deleteProfile fails", async () => {
    const deleteMutate = vi.fn(
      (_id: string, opts?: { onError?: (err: Error) => void }) => {
        opts?.onError?.(new Error("cannot delete built-in profile"));
      },
    );
    mockSnapshot.mockReturnValue(
      snapshotWith([makeProfile({ id: "my-profile", is_builtin: false })]),
    );
    mockMutations.mockReturnValue(mutationsWith({ delete: { mutate: deleteMutate } }));

    renderWithProviders(<ProfilesSection />);
    fireEvent.click(screen.getByRole("button", { name: /^delete$/i }));
    await waitFor(() => {
      expect(screen.getByText(/cannot delete built-in profile/i)).toBeTruthy();
    });
    expect(vi.mocked(reportFrontendEventSafely)).toHaveBeenCalledWith(
      expect.objectContaining({ event_code: "profile.delete.failed" }),
    );
  });

  // ── Create form ─────────────────────────────────────────────────────

  it("renders the Create Profile form", () => {
    renderWithProviders(<ProfilesSection />);
    expect(screen.getByLabelText(/profile id/i)).toBeTruthy();
    expect(screen.getByRole("button", { name: /create profile/i })).toBeTruthy();
  });

  it("submitting create with valid data calls createProfile.mutate", () => {
    const createMutate = vi.fn();
    mockMutations.mockReturnValue(mutationsWith({ create: { mutate: createMutate } }));

    renderWithProviders(<ProfilesSection />);
    fireEvent.change(screen.getByLabelText(/profile id/i), { target: { value: "my-new-profile" } });
    fireEvent.change(screen.getByLabelText(/tools for new profile/i), { target: { value: "read, write" } });
    fireEvent.change(screen.getByLabelText(/context budget \(tokens\)/i), { target: { value: "6000" } });
    fireEvent.change(screen.getByLabelText(/timeout \(seconds\)/i), { target: { value: "60" } });
    fireEvent.click(screen.getByRole("button", { name: /create profile/i }));

    expect(createMutate).toHaveBeenCalledTimes(1);
    const [payload] = createMutate.mock.calls[0]!;
    expect(payload).toEqual({
      id: "my-new-profile",
      tools: ["read", "write"],
      context_budget_tokens: 6000,
      timeout_seconds: 60,
      write_access: false,
    });
  });

  it("create with empty id shows validation error and does not call mutate", () => {
    const createMutate = vi.fn();
    mockMutations.mockReturnValue(mutationsWith({ create: { mutate: createMutate } }));

    renderWithProviders(<ProfilesSection />);
    fireEvent.click(screen.getByRole("button", { name: /create profile/i }));

    expect(createMutate).not.toHaveBeenCalled();
    expect(screen.getByText(/profile id cannot be empty/i)).toBeTruthy();
  });

  it("createProfile onSuccess reports event and resets the form", async () => {
    const createMutate = vi.fn(
      (_req: unknown, opts?: { onSuccess?: () => void }) => {
        opts?.onSuccess?.();
      },
    );
    mockMutations.mockReturnValue(mutationsWith({ create: { mutate: createMutate } }));

    renderWithProviders(<ProfilesSection />);
    const idInput = screen.getByLabelText(/profile id/i) as HTMLInputElement;
    fireEvent.change(idInput, { target: { value: "temp-profile" } });
    fireEvent.click(screen.getByRole("button", { name: /create profile/i }));

    await waitFor(() => {
      expect(vi.mocked(reportFrontendEventSafely)).toHaveBeenCalledWith(
        expect.objectContaining({ event_code: "profile.created" }),
      );
    });
    // Form reset: id input is empty again.
    expect(idInput.value).toBe("");
  });

  it("createProfile onError shows error + reports profile.create.failed", async () => {
    const createMutate = vi.fn(
      (_req: unknown, opts?: { onError?: (err: Error) => void }) => {
        opts?.onError?.(new Error("id already exists"));
      },
    );
    mockMutations.mockReturnValue(mutationsWith({ create: { mutate: createMutate } }));

    renderWithProviders(<ProfilesSection />);
    fireEvent.change(screen.getByLabelText(/profile id/i), { target: { value: "dup" } });
    fireEvent.click(screen.getByRole("button", { name: /create profile/i }));

    await waitFor(() => {
      expect(screen.getByText(/id already exists/i)).toBeTruthy();
    });
    expect(vi.mocked(reportFrontendEventSafely)).toHaveBeenCalledWith(
      expect.objectContaining({ event_code: "profile.create.failed" }),
    );
  });
});
