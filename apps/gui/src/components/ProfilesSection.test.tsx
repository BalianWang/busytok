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

beforeEach(() => {
  vi.clearAllMocks();
  // Default: no profiles.
  mockSnapshot.mockReturnValue({
    data: {
      data: { subagent: { enabled: true, profiles: [] } },
    } as unknown as ReadEnvelopeDto<SettingsSnapshotDto>,
    isLoading: false,
    isError: false,
    isFetching: false,
  } as never);
  mockMutations.mockReturnValue({
    createProfile: { mutate: vi.fn(), isPending: false },
    updateProfile: { mutate: vi.fn(), isPending: false },
    deleteProfile: { mutate: vi.fn(), isPending: false },
  } as never);
});

afterEach(() => cleanup());

describe("ProfilesSection", () => {
  it("renders built-in profiles from settings snapshot", () => {
    const profiles = [
      makeProfile({ id: "pi/search-cheap", is_builtin: true }),
      makeProfile({ id: "pi/review-cheap", is_builtin: true }),
    ];
    mockSnapshot.mockReturnValue({
      data: {
        data: { subagent: { enabled: true, profiles } },
      } as unknown as ReadEnvelopeDto<SettingsSnapshotDto>,
      isLoading: false,
      isError: false,
      isFetching: false,
    } as never);
    renderWithProviders(<ProfilesSection />);
    expect(screen.getByText("pi/search-cheap")).toBeTruthy();
    expect(screen.getByText("pi/review-cheap")).toBeTruthy();
  });

  it("renders advanced read-only fields (tools, budget, timeout, write_access)", () => {
    mockSnapshot.mockReturnValue({
      data: {
        data: {
          subagent: {
            enabled: true,
            profiles: [
              makeProfile({
                id: "my-profile",
                is_builtin: false,
                tools: ["read", "grep", "glob"],
                context_budget_tokens: 8000,
                timeout_seconds: 60,
                write_access: true,
              }),
            ],
          },
        },
      } as unknown as ReadEnvelopeDto<SettingsSnapshotDto>,
      isLoading: false,
      isError: false,
      isFetching: false,
    } as never);
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
    mockSnapshot.mockReturnValue({
      data: {
        data: { subagent: { enabled: true, profiles } },
      } as unknown as ReadEnvelopeDto<SettingsSnapshotDto>,
      isLoading: false,
      isError: false,
      isFetching: false,
    } as never);

    renderWithProviders(<ProfilesSection />);
    const builtinRow = screen.getByText("pi/search-cheap").closest(".settings-panel");
    expect(builtinRow?.querySelector('button[class*="btn--danger"]')).toBeNull();
    const userRow = screen.getByText("my-custom").closest(".settings-panel");
    expect(userRow?.querySelector('button[class*="btn--danger"]')).not.toBeNull();
  });

  it("calls deleteProfile.mutate when Delete is clicked on a user profile", () => {
    const deleteMutate = vi.fn();
    mockSnapshot.mockReturnValue({
      data: {
        data: {
          subagent: {
            enabled: true,
            profiles: [makeProfile({ id: "my-custom", is_builtin: false })],
          },
        },
      } as unknown as ReadEnvelopeDto<SettingsSnapshotDto>,
      isLoading: false,
      isError: false,
      isFetching: false,
    } as never);
    mockMutations.mockReturnValue({
      createProfile: { mutate: vi.fn(), isPending: false },
      updateProfile: { mutate: vi.fn(), isPending: false },
      deleteProfile: { mutate: deleteMutate, isPending: false },
    } as never);

    renderWithProviders(<ProfilesSection />);
    fireEvent.click(screen.getByRole("button", { name: /delete/i }));
    expect(deleteMutate).toHaveBeenCalledWith("my-custom", expect.anything());
  });

  it("shows Deleting... and disables the Delete button while delete is pending", () => {
    mockSnapshot.mockReturnValue({
      data: {
        data: {
          subagent: {
            enabled: true,
            profiles: [makeProfile({ id: "my-custom", is_builtin: false })],
          },
        },
      } as unknown as ReadEnvelopeDto<SettingsSnapshotDto>,
      isLoading: false,
      isError: false,
      isFetching: false,
    } as never);
    mockMutations.mockReturnValue({
      createProfile: { mutate: vi.fn(), isPending: false },
      updateProfile: { mutate: vi.fn(), isPending: false },
      deleteProfile: { mutate: vi.fn(), isPending: true },
    } as never);

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
    mockSnapshot.mockReturnValue({
      data: {
        data: {
          subagent: {
            enabled: true,
            profiles: [makeProfile({ id: "my-profile", is_builtin: false })],
          },
        },
      } as unknown as ReadEnvelopeDto<SettingsSnapshotDto>,
      isLoading: false,
      isError: false,
      isFetching: false,
    } as never);
    mockMutations.mockReturnValue({
      createProfile: { mutate: vi.fn(), isPending: false },
      updateProfile: { mutate: vi.fn(), isPending: false },
      deleteProfile: { mutate: deleteMutate, isPending: false },
    } as never);

    renderWithProviders(<ProfilesSection />);
    fireEvent.click(screen.getByRole("button", { name: /delete/i }));
    await waitFor(() => {
      expect(screen.getByText(/cannot delete built-in profile/i)).toBeTruthy();
    });
    expect(vi.mocked(reportFrontendEventSafely)).toHaveBeenCalledWith(
      expect.objectContaining({ event_code: "profile.delete.failed" }),
    );
  });
});
