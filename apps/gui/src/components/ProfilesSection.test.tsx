import { describe, it, expect, vi, beforeEach, afterEach } from "vitest";
import { render, screen, fireEvent, waitFor, cleanup } from "@testing-library/react";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import type {
  ProviderDto,
  ProviderListResponseDto,
  ProfileDto,
  ReadEnvelopeDto,
  SettingsSnapshotDto,
} from "@busytok/protocol-types";

vi.mock("../api/useBusytokData", () => ({
  useSettingsSnapshot: vi.fn(),
  useProviders: vi.fn(),
  useProfileMutations: vi.fn(),
}));
vi.mock("../logging/safeReporter", () => ({
  reportFrontendEventSafely: vi.fn(),
}));

import { useSettingsSnapshot, useProviders, useProfileMutations } from "../api/useBusytokData";
import { reportFrontendEventSafely } from "../logging/safeReporter";
import { ProfilesSection } from "./ProfilesSection";

const mockSnapshot = vi.mocked(useSettingsSnapshot);
const mockProviders = vi.mocked(useProviders);
const mockMutations = vi.mocked(useProfileMutations);

function makeProfile(overrides: Partial<ProfileDto> = {}): ProfileDto {
  return {
    id: "pi/search-cheap",
    is_builtin: true,
    provider_id: null,
    model: "deepseek-chat",
    tools: ["read", "grep"],
    context_budget_tokens: 3000,
    timeout_seconds: 120,
    write_access: false,
    ...overrides,
  };
}

function makeProvider(overrides: Partial<ProviderDto> = {}): ProviderDto {
  return {
    id: "deepseek",
    name: "DeepSeek",
    base_url: "https://api.deepseek.com/v1",
    api_key_env_name: "DEEPSEEK_API_KEY",
    base_url_env_name: null,
    models: ["deepseek-chat"],
    enabled: true,
    has_api_key: true,
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

describe("ProfilesSection", () => {
  beforeEach(() => {
    vi.clearAllMocks();
  });

  afterEach(() => cleanup());

  it("renders built-in profiles from settings snapshot", () => {
    const profiles = [
      makeProfile({ id: "pi/search-cheap", is_builtin: true }),
      makeProfile({ id: "pi/review-cheap", is_builtin: true, model: "qwen-coder" }),
    ];
    mockSnapshot.mockReturnValue({
      data: { data: { subagent: { enabled: true, profiles } } } as ReadEnvelopeDto<SettingsSnapshotDto>,
      isLoading: false,
      isError: false,
      isFetching: false,
    } as never);
    mockProviders.mockReturnValue({
      data: { providers: [] } as ProviderListResponseDto,
      isLoading: false,
    } as never);
    mockMutations.mockReturnValue({
      createProfile: { mutate: vi.fn(), isPending: false },
      updateProfile: { mutate: vi.fn(), isPending: false },
      deleteProfile: { mutate: vi.fn(), isPending: false },
    } as never);

    renderWithProviders(<ProfilesSection />);
    expect(screen.getByText("pi/search-cheap")).toBeTruthy();
    expect(screen.getByText("pi/review-cheap")).toBeTruthy();
  });

  it("shows ⚠ warning when profile is bound to a disabled provider", () => {
    mockSnapshot.mockReturnValue({
      data: {
        data: {
          subagent: {
            enabled: true,
            profiles: [makeProfile({ provider_id: "disabled-p" })],
          },
        } as ReadEnvelopeDto<SettingsSnapshotDto>,
      },
      isLoading: false,
      isError: false,
      isFetching: false,
    } as never);
    mockProviders.mockReturnValue({
      data: {
        providers: [makeProvider({ id: "disabled-p", enabled: false })],
      } as ProviderListResponseDto,
      isLoading: false,
    } as never);
    mockMutations.mockReturnValue({
      createProfile: { mutate: vi.fn(), isPending: false },
      updateProfile: { mutate: vi.fn(), isPending: false },
      deleteProfile: { mutate: vi.fn(), isPending: false },
    } as never);

    renderWithProviders(<ProfilesSection />);
    expect(screen.getByText(/disabled provider/i)).toBeTruthy();
  });

  it("shows stale model warning when model not in provider whitelist", () => {
    mockSnapshot.mockReturnValue({
      data: {
        data: {
          subagent: {
            enabled: true,
            profiles: [makeProfile({ provider_id: "deepseek", model: "stale-model" })],
          },
        } as ReadEnvelopeDto<SettingsSnapshotDto>,
      },
      isLoading: false,
      isError: false,
      isFetching: false,
    } as never);
    mockProviders.mockReturnValue({
      data: {
        providers: [makeProvider({ id: "deepseek", models: ["deepseek-chat"] })],
      } as ProviderListResponseDto,
      isLoading: false,
    } as never);
    mockMutations.mockReturnValue({
      createProfile: { mutate: vi.fn(), isPending: false },
      updateProfile: { mutate: vi.fn(), isPending: false },
      deleteProfile: { mutate: vi.fn(), isPending: false },
    } as never);

    renderWithProviders(<ProfilesSection />);
    expect(screen.getByText(/stale|invalid model/i)).toBeTruthy();
  });

  it("calls profileUpdate when binding provider + model", async () => {
    const updateMutate = vi.fn();
    mockSnapshot.mockReturnValue({
      data: {
        data: {
          subagent: {
            enabled: true,
            profiles: [makeProfile({ provider_id: null, model: "deepseek-chat" })],
          },
        } as ReadEnvelopeDto<SettingsSnapshotDto>,
      },
      isLoading: false,
      isError: false,
      isFetching: false,
    } as never);
    mockProviders.mockReturnValue({
      data: {
        providers: [makeProvider({ id: "deepseek", models: ["deepseek-chat"] })],
      } as ProviderListResponseDto,
      isLoading: false,
    } as never);
    mockMutations.mockReturnValue({
      createProfile: { mutate: vi.fn(), isPending: false },
      updateProfile: { mutate: updateMutate, isPending: false },
      deleteProfile: { mutate: vi.fn(), isPending: false },
    } as never);

    renderWithProviders(<ProfilesSection />);

    // Click "Edit" on the profile row.
    fireEvent.click(screen.getByRole("button", { name: /edit/i }));

    // Select provider from dropdown.
    const select = screen.getByLabelText(/provider/i);
    fireEvent.change(select, { target: { value: "deepseek" } });

    // Click Save.
    fireEvent.click(screen.getByRole("button", { name: /save/i }));

    await waitFor(() => {
      expect(updateMutate).toHaveBeenCalledWith(
        expect.objectContaining({
          id: "pi/search-cheap",
          provider_id: "deepseek",
          model: "deepseek-chat",
        }),
        expect.anything(),
      );
    });
  });

  it("hides the Delete button for built-in profiles", () => {
    const profiles = [
      makeProfile({ id: "pi/search-cheap", is_builtin: true }),
      makeProfile({ id: "my-custom", is_builtin: false }),
    ];
    mockSnapshot.mockReturnValue({
      data: { data: { subagent: { enabled: true, profiles } } } as ReadEnvelopeDto<SettingsSnapshotDto>,
      isLoading: false,
      isError: false,
      isFetching: false,
    } as never);
    mockProviders.mockReturnValue({
      data: { providers: [] } as ProviderListResponseDto,
      isLoading: false,
    } as never);
    mockMutations.mockReturnValue({
      createProfile: { mutate: vi.fn(), isPending: false },
      updateProfile: { mutate: vi.fn(), isPending: false },
      deleteProfile: { mutate: vi.fn(), isPending: false },
    } as never);

    renderWithProviders(<ProfilesSection />);
    // Built-in profile row: no Delete button.
    const builtinRow = screen.getByText("pi/search-cheap").closest(".settings-panel");
    expect(builtinRow?.querySelector('button[class*="btn--danger"]')).toBeNull();
    // User profile row: Delete button present.
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
        } as ReadEnvelopeDto<SettingsSnapshotDto>,
      },
      isLoading: false,
      isError: false,
      isFetching: false,
    } as never);
    mockProviders.mockReturnValue({
      data: { providers: [] } as ProviderListResponseDto,
      isLoading: false,
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

  it("cascade-filters the model dropdown when provider changes", () => {
    mockSnapshot.mockReturnValue({
      data: {
        data: {
          subagent: {
            enabled: true,
            profiles: [makeProfile({ provider_id: null, model: "" })],
          },
        } as ReadEnvelopeDto<SettingsSnapshotDto>,
      },
      isLoading: false,
      isError: false,
      isFetching: false,
    } as never);
    mockProviders.mockReturnValue({
      data: {
        providers: [
          makeProvider({ id: "deepseek", models: ["deepseek-chat", "deepseek-reasoner"] }),
          makeProvider({ id: "openai", name: "OpenAI", models: ["gpt-4", "gpt-3.5-turbo"] }),
        ],
      } as ProviderListResponseDto,
      isLoading: false,
    } as never);
    mockMutations.mockReturnValue({
      createProfile: { mutate: vi.fn(), isPending: false },
      updateProfile: { mutate: vi.fn(), isPending: false },
      deleteProfile: { mutate: vi.fn(), isPending: false },
    } as never);

    renderWithProviders(<ProfilesSection />);
    fireEvent.click(screen.getByRole("button", { name: /edit/i }));

    // Select deepseek → model dropdown shows deepseek models.
    fireEvent.change(screen.getByLabelText(/provider/i), { target: { value: "deepseek" } });
    let modelSelect = screen.getByLabelText(/model/i) as HTMLSelectElement;
    expect(modelSelect.innerHTML).toContain("deepseek-chat");
    expect(modelSelect.innerHTML).toContain("deepseek-reasoner");
    expect(modelSelect.innerHTML).not.toContain("gpt-4");

    // Switch to openai → model dropdown now shows openai models only.
    fireEvent.change(screen.getByLabelText(/provider/i), { target: { value: "openai" } });
    modelSelect = screen.getByLabelText(/model/i) as HTMLSelectElement;
    expect(modelSelect.innerHTML).toContain("gpt-4");
    expect(modelSelect.innerHTML).not.toContain("deepseek-chat");
  });

  it("Cancel button exits edit mode without calling mutate", () => {
    const updateMutate = vi.fn();
    mockSnapshot.mockReturnValue({
      data: {
        data: {
          subagent: {
            enabled: true,
            profiles: [makeProfile({ provider_id: null, model: "deepseek-chat" })],
          },
        } as ReadEnvelopeDto<SettingsSnapshotDto>,
      },
      isLoading: false,
      isError: false,
      isFetching: false,
    } as never);
    mockProviders.mockReturnValue({
      data: { providers: [] } as ProviderListResponseDto,
      isLoading: false,
    } as never);
    mockMutations.mockReturnValue({
      createProfile: { mutate: vi.fn(), isPending: false },
      updateProfile: { mutate: updateMutate, isPending: false },
      deleteProfile: { mutate: vi.fn(), isPending: false },
    } as never);

    renderWithProviders(<ProfilesSection />);
    fireEvent.click(screen.getByRole("button", { name: /edit/i }));
    fireEvent.click(screen.getByRole("button", { name: /cancel/i }));
    // No Save button after cancel (back to view mode).
    expect(screen.queryByRole("button", { name: /save/i })).toBeNull();
    // updateProfile was NOT called.
    expect(updateMutate).not.toHaveBeenCalled();
  });

  it("disables Save button when model is not in selected provider's whitelist", () => {
    mockSnapshot.mockReturnValue({
      data: {
        data: {
          subagent: {
            enabled: true,
            profiles: [makeProfile({ provider_id: null, model: "deepseek-chat" })],
          },
        } as ReadEnvelopeDto<SettingsSnapshotDto>,
      },
      isLoading: false,
      isError: false,
      isFetching: false,
    } as never);
    mockProviders.mockReturnValue({
      data: {
        providers: [makeProvider({ id: "deepseek", models: ["deepseek-chat"] })],
      } as ProviderListResponseDto,
      isLoading: false,
    } as never);
    mockMutations.mockReturnValue({
      createProfile: { mutate: vi.fn(), isPending: false },
      updateProfile: { mutate: vi.fn(), isPending: false },
      deleteProfile: { mutate: vi.fn(), isPending: false },
    } as never);

    renderWithProviders(<ProfilesSection />);
    fireEvent.click(screen.getByRole("button", { name: /edit/i }));
    // Select provider — model auto-resets to first available ("deepseek-chat"),
    // so Save is enabled.
    fireEvent.change(screen.getByLabelText(/provider/i), { target: { value: "deepseek" } });
    let saveBtn = screen.getByRole("button", { name: /save/i }) as HTMLButtonElement;
    expect(saveBtn.disabled).toBe(false);

    // Manually clear the model selection (set to empty option) → Save disabled.
    fireEvent.change(screen.getByLabelText(/model/i), { target: { value: "" } });
    saveBtn = screen.getByRole("button", { name: /save/i }) as HTMLButtonElement;
    expect(saveBtn.disabled).toBe(true);
  });

  it("shows loading state when snapshot is loading", () => {
    mockSnapshot.mockReturnValue({
      data: undefined,
      isLoading: true,
      isError: false,
      isFetching: false,
    } as never);
    mockProviders.mockReturnValue({
      data: { providers: [] } as ProviderListResponseDto,
      isLoading: false,
    } as never);
    mockMutations.mockReturnValue({
      createProfile: { mutate: vi.fn(), isPending: false },
      updateProfile: { mutate: vi.fn(), isPending: false },
      deleteProfile: { mutate: vi.fn(), isPending: false },
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
    mockProviders.mockReturnValue({
      data: { providers: [] } as ProviderListResponseDto,
      isLoading: false,
    } as never);
    mockMutations.mockReturnValue({
      createProfile: { mutate: vi.fn(), isPending: false },
      updateProfile: { mutate: vi.fn(), isPending: false },
      deleteProfile: { mutate: vi.fn(), isPending: false },
    } as never);

    renderWithProviders(<ProfilesSection />);
    expect(screen.getByText(/failed to load profiles/i)).toBeTruthy();
  });

  it("renders empty state when no profiles configured", () => {
    mockSnapshot.mockReturnValue({
      data: {
        data: {
          subagent: {
            enabled: true,
            profiles: [],
          },
        } as ReadEnvelopeDto<SettingsSnapshotDto>,
      },
      isLoading: false,
      isError: false,
      isFetching: false,
    } as never);
    mockProviders.mockReturnValue({
      data: { providers: [] } as ProviderListResponseDto,
      isLoading: false,
    } as never);
    mockMutations.mockReturnValue({
      createProfile: { mutate: vi.fn(), isPending: false },
      updateProfile: { mutate: vi.fn(), isPending: false },
      deleteProfile: { mutate: vi.fn(), isPending: false },
    } as never);

    renderWithProviders(<ProfilesSection />);
    expect(screen.getByText(/no profiles configured/i)).toBeTruthy();
  });

  it("shows mutation error when updateProfile fails", async () => {
    const updateMutate = vi.fn(
      (_payload: unknown, opts?: { onError?: (err: Error) => void }) => {
        opts?.onError?.(new Error("model not in provider whitelist"));
      },
    );
    mockSnapshot.mockReturnValue({
      data: {
        data: {
          subagent: {
            enabled: true,
            profiles: [makeProfile({ provider_id: null, model: "deepseek-chat" })],
          },
        } as ReadEnvelopeDto<SettingsSnapshotDto>,
      },
      isLoading: false,
      isError: false,
      isFetching: false,
    } as never);
    mockProviders.mockReturnValue({
      data: { providers: [] } as ProviderListResponseDto,
      isLoading: false,
    } as never);
    mockMutations.mockReturnValue({
      createProfile: { mutate: vi.fn(), isPending: false },
      updateProfile: { mutate: updateMutate, isPending: false },
      deleteProfile: { mutate: vi.fn(), isPending: false },
    } as never);

    renderWithProviders(<ProfilesSection />);
    fireEvent.click(screen.getByRole("button", { name: /edit/i }));
    fireEvent.click(screen.getByRole("button", { name: /save/i }));

    await waitFor(() => {
      expect(screen.getByText(/model not in provider whitelist/i)).toBeTruthy();
    });
    expect(vi.mocked(reportFrontendEventSafely)).toHaveBeenCalledWith(
      expect.objectContaining({ event_code: "profile.update.failed" }),
    );
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
        } as ReadEnvelopeDto<SettingsSnapshotDto>,
      },
      isLoading: false,
      isError: false,
      isFetching: false,
    } as never);
    mockProviders.mockReturnValue({
      data: { providers: [] } as ProviderListResponseDto,
      isLoading: false,
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

  it("clears mutation error when starting a new edit", () => {
    const updateMutate = vi.fn(
      (_payload: unknown, opts?: { onError?: (err: Error) => void }) => {
        opts?.onError?.(new Error("first error"));
      },
    );
    mockSnapshot.mockReturnValue({
      data: {
        data: {
          subagent: {
            enabled: true,
            profiles: [makeProfile({ provider_id: null, model: "deepseek-chat" })],
          },
        } as ReadEnvelopeDto<SettingsSnapshotDto>,
      },
      isLoading: false,
      isError: false,
      isFetching: false,
    } as never);
    mockProviders.mockReturnValue({
      data: { providers: [] } as ProviderListResponseDto,
      isLoading: false,
    } as never);
    mockMutations.mockReturnValue({
      createProfile: { mutate: vi.fn(), isPending: false },
      updateProfile: { mutate: updateMutate, isPending: false },
      deleteProfile: { mutate: vi.fn(), isPending: false },
    } as never);

    renderWithProviders(<ProfilesSection />);
    // Trigger first error.
    fireEvent.click(screen.getByRole("button", { name: /edit/i }));
    fireEvent.click(screen.getByRole("button", { name: /save/i }));
    expect(screen.getByText(/first error/i)).toBeTruthy();

    // Click Edit again — error should clear.
    fireEvent.click(screen.getByRole("button", { name: /edit/i }));
    expect(screen.queryByText(/first error/i)).toBeNull();
  });
});
