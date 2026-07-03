import { describe, it, expect, vi, beforeEach, afterEach } from "vitest";
import { render, screen, fireEvent, waitFor, cleanup } from "@testing-library/react";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import type {
  ProviderDto,
  ProviderListResponseDto,
  ProfileDto,
  ModelCatalogEntryDto,
  ModelListResponseDto,
  ReadEnvelopeDto,
  SettingsSnapshotDto,
} from "@busytok/protocol-types";

vi.mock("../api/useBusytokData", () => ({
  useSettingsSnapshot: vi.fn(),
  useProviders: vi.fn(),
  useProfileMutations: vi.fn(),
  useModels: vi.fn(),
}));
vi.mock("../logging/safeReporter", () => ({
  reportFrontendEventSafely: vi.fn(),
}));

import {
  useSettingsSnapshot,
  useProviders,
  useProfileMutations,
  useModels,
} from "../api/useBusytokData";
import { reportFrontendEventSafely } from "../logging/safeReporter";
import { ProfilesSection } from "./ProfilesSection";

const mockSnapshot = vi.mocked(useSettingsSnapshot);
const mockProviders = vi.mocked(useProviders);
const mockMutations = vi.mocked(useProfileMutations);
const mockUseModels = vi.mocked(useModels);

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
    provider_kind: "openai_compatible",
    base_url: "https://api.deepseek.com/v1",
    enabled: true,
    has_api_key: true,
    created_at_ms: 0,
    updated_at_ms: 0,
    ...overrides,
  };
}

function makeModel(overrides: Partial<ModelCatalogEntryDto> = {}): ModelCatalogEntryDto {
  return {
    provider_id: "deepseek",
    provider_name: "DeepSeek",
    provider_kind: "openai_compatible",
    provider_enabled: true,
    model_db_id: "m-0001",
    model_id: "deepseek-chat",
    model_enabled: true,
    tags: ["chat"],
    ...overrides,
  };
}

type ModelsQueryResult = ReturnType<typeof useModels>;

function emptyModelsQuery(
  extras: Partial<ModelsQueryResult> = {},
): ModelsQueryResult {
  return {
    data: { models: [] } as ModelListResponseDto,
    isLoading: false,
    isError: false,
    isFetching: false,
    ...extras,
  } as never;
}

/**
 * Mock `useModels` to return a per-provider model list. Unrecognised
 * providerIds (including undefined/"" for unbound profiles) get an empty
 * list so display-state and edit-state hooks both behave correctly.
 */
function mockModelsPerProvider(
  map: Record<string, ModelCatalogEntryDto[]>,
  extras: Partial<ModelsQueryResult> = {},
) {
  mockUseModels.mockImplementation((filter = {}) => {
    const pid = filter.providerId;
    if (pid && map[pid]) {
      return {
        data: { models: map[pid] } as ModelListResponseDto,
        isLoading: false,
        isError: false,
        isFetching: false,
        ...extras,
      } as never;
    }
    return emptyModelsQuery(extras);
  });
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
  // Default: no providers, no profiles, no models.
  mockSnapshot.mockReturnValue({
    data: {
      data: { subagent: { enabled: true, profiles: [] } },
    } as unknown as ReadEnvelopeDto<SettingsSnapshotDto>,
    isLoading: false,
    isError: false,
    isFetching: false,
  } as never);
  mockProviders.mockReturnValue({
    data: { providers: [] } as ProviderListResponseDto,
    isLoading: false,
    isError: false,
    isFetching: false,
  } as never);
  mockMutations.mockReturnValue({
    createProfile: { mutate: vi.fn(), isPending: false },
    updateProfile: { mutate: vi.fn(), isPending: false },
    deleteProfile: { mutate: vi.fn(), isPending: false },
  } as never);
  mockUseModels.mockReturnValue(emptyModelsQuery());
});

afterEach(() => cleanup());

describe("ProfilesSection", () => {
  it("renders built-in profiles from settings snapshot", () => {
    const profiles = [
      makeProfile({ id: "pi/search-cheap", is_builtin: true }),
      makeProfile({ id: "pi/review-cheap", is_builtin: true, model: "qwen-coder" }),
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

  it("shows ⚠ warning when profile is bound to a disabled provider", () => {
    mockSnapshot.mockReturnValue({
      data: {
        data: {
          subagent: {
            enabled: true,
            profiles: [makeProfile({ provider_id: "disabled-p", model: "some-model" })],
          },
        },
      } as unknown as ReadEnvelopeDto<SettingsSnapshotDto>,
      isLoading: false,
      isError: false,
      isFetching: false,
    } as never);
    mockProviders.mockReturnValue({
      data: {
        providers: [makeProvider({ id: "disabled-p", enabled: false })],
      } as ProviderListResponseDto,
      isLoading: false,
      isError: false,
      isFetching: false,
    } as never);
    // Provide models for the disabled provider containing the profile's model
    // so no stale warning interferes with the disabled-provider assertion.
    mockModelsPerProvider({
      "disabled-p": [makeModel({ provider_id: "disabled-p", model_id: "some-model" })],
    });

    renderWithProviders(<ProfilesSection />);
    expect(screen.getByText(/disabled provider/i)).toBeTruthy();
  });

  // ── Step 5a: display-state stale check via ProfileModelStatus ──────

  it("shows stale model warning when model not in provider's model catalog (Step 5a)", () => {
    mockSnapshot.mockReturnValue({
      data: {
        data: {
          subagent: {
            enabled: true,
            profiles: [makeProfile({ provider_id: "deepseek", model: "stale-model" })],
          },
        },
      } as unknown as ReadEnvelopeDto<SettingsSnapshotDto>,
      isLoading: false,
      isError: false,
      isFetching: false,
    } as never);
    mockProviders.mockReturnValue({
      data: {
        providers: [makeProvider({ id: "deepseek" })],
      } as ProviderListResponseDto,
      isLoading: false,
      isError: false,
      isFetching: false,
    } as never);
    // Catalog for deepseek does NOT contain "stale-model".
    mockModelsPerProvider({
      deepseek: [makeModel({ provider_id: "deepseek", model_id: "deepseek-chat" })],
    });

    renderWithProviders(<ProfilesSection />);
    expect(screen.getByText(/stale model/i)).toBeTruthy();
  });

  it("does NOT show stale warning when model IS in the provider's catalog", () => {
    mockSnapshot.mockReturnValue({
      data: {
        data: {
          subagent: {
            enabled: true,
            profiles: [makeProfile({ provider_id: "deepseek", model: "deepseek-chat" })],
          },
        },
      } as unknown as ReadEnvelopeDto<SettingsSnapshotDto>,
      isLoading: false,
      isError: false,
      isFetching: false,
    } as never);
    mockProviders.mockReturnValue({
      data: {
        providers: [makeProvider({ id: "deepseek" })],
      } as ProviderListResponseDto,
      isLoading: false,
      isError: false,
      isFetching: false,
    } as never);
    mockModelsPerProvider({
      deepseek: [makeModel({ provider_id: "deepseek", model_id: "deepseek-chat" })],
    });

    renderWithProviders(<ProfilesSection />);
    expect(screen.queryByText(/stale model/i)).toBeNull();
    // Model row shows the actual model id (not "—").
    expect(screen.getByText("deepseek-chat")).toBeTruthy();
  });

  it("does NOT show stale warning for an unbound profile (providerId is null)", () => {
    mockSnapshot.mockReturnValue({
      data: {
        data: {
          subagent: {
            enabled: true,
            profiles: [makeProfile({ provider_id: null, model: "deepseek-chat" })],
          },
        },
      } as unknown as ReadEnvelopeDto<SettingsSnapshotDto>,
      isLoading: false,
      isError: false,
      isFetching: false,
    } as never);
    mockProviders.mockReturnValue({
      data: { providers: [] } as ProviderListResponseDto,
      isLoading: false,
      isError: false,
      isFetching: false,
    } as never);

    renderWithProviders(<ProfilesSection />);
    expect(screen.queryByText(/stale model/i)).toBeNull();
  });

  // ── Step 5b/5c: edit-state models + cascade reset ─────────────────

  it("calls profileUpdate when binding provider + model", async () => {
    const updateMutate = vi.fn();
    mockSnapshot.mockReturnValue({
      data: {
        data: {
          subagent: {
            enabled: true,
            profiles: [makeProfile({ provider_id: null, model: "deepseek-chat" })],
          },
        },
      } as unknown as ReadEnvelopeDto<SettingsSnapshotDto>,
      isLoading: false,
      isError: false,
      isFetching: false,
    } as never);
    mockProviders.mockReturnValue({
      data: {
        providers: [makeProvider({ id: "deepseek" })],
      } as ProviderListResponseDto,
      isLoading: false,
      isError: false,
      isFetching: false,
    } as never);
    mockModelsPerProvider({
      deepseek: [makeModel({ provider_id: "deepseek", model_id: "deepseek-chat" })],
    });
    mockMutations.mockReturnValue({
      createProfile: { mutate: vi.fn(), isPending: false },
      updateProfile: { mutate: updateMutate, isPending: false },
      deleteProfile: { mutate: vi.fn(), isPending: false },
    } as never);

    renderWithProviders(<ProfilesSection />);
    fireEvent.click(screen.getByRole("button", { name: /edit/i }));
    fireEvent.change(screen.getByLabelText(/provider/i), {
      target: { value: "deepseek" },
    });
    // Wait for the cascade effect to settle (model auto-selects first available).
    await waitFor(() => {
      const saveBtn = screen.getByRole("button", { name: /save/i }) as HTMLButtonElement;
      expect(saveBtn.disabled).toBe(false);
    });
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
      data: {
        data: { subagent: { enabled: true, profiles } },
      } as unknown as ReadEnvelopeDto<SettingsSnapshotDto>,
      isLoading: false,
      isError: false,
      isFetching: false,
    } as never);
    mockProviders.mockReturnValue({
      data: { providers: [] } as ProviderListResponseDto,
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
    mockProviders.mockReturnValue({
      data: { providers: [] } as ProviderListResponseDto,
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

  // ── Step 5c: cascade reset on provider change ─────────────────────

  it("cascade-resets the model dropdown when provider changes (Step 5c)", async () => {
    mockSnapshot.mockReturnValue({
      data: {
        data: {
          subagent: {
            enabled: true,
            profiles: [makeProfile({ provider_id: null, model: "" })],
          },
        },
      } as unknown as ReadEnvelopeDto<SettingsSnapshotDto>,
      isLoading: false,
      isError: false,
      isFetching: false,
    } as never);
    mockProviders.mockReturnValue({
      data: {
        providers: [
          makeProvider({ id: "deepseek", name: "DeepSeek" }),
          makeProvider({ id: "openai", name: "OpenAI" }),
        ],
      } as ProviderListResponseDto,
      isLoading: false,
      isError: false,
      isFetching: false,
    } as never);
    mockModelsPerProvider({
      deepseek: [
        makeModel({ provider_id: "deepseek", model_id: "deepseek-chat" }),
        makeModel({ provider_id: "deepseek", model_id: "deepseek-reasoner" }),
      ],
      openai: [
        makeModel({ provider_id: "openai", model_id: "gpt-4" }),
        makeModel({ provider_id: "openai", model_id: "gpt-3.5-turbo" }),
      ],
    });

    renderWithProviders(<ProfilesSection />);
    fireEvent.click(screen.getByRole("button", { name: /edit/i }));

    // Select deepseek → model dropdown shows deepseek models, cascade
    // auto-selects the first one ("deepseek-chat").
    fireEvent.change(screen.getByLabelText(/provider/i), {
      target: { value: "deepseek" },
    });
    await waitFor(() => {
      const modelSelect = screen.getByLabelText(/model/i) as HTMLSelectElement;
      expect(modelSelect.innerHTML).toContain("deepseek-chat");
      expect(modelSelect.innerHTML).toContain("deepseek-reasoner");
      expect(modelSelect.innerHTML).not.toContain("gpt-4");
    });

    // Switch to openai → model dropdown now shows openai models only,
    // cascade auto-selects "gpt-4".
    fireEvent.change(screen.getByLabelText(/provider/i), {
      target: { value: "openai" },
    });
    await waitFor(() => {
      const modelSelect = screen.getByLabelText(/model/i) as HTMLSelectElement;
      expect(modelSelect.innerHTML).toContain("gpt-4");
      expect(modelSelect.innerHTML).not.toContain("deepseek-chat");
    });
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
        },
      } as unknown as ReadEnvelopeDto<SettingsSnapshotDto>,
      isLoading: false,
      isError: false,
      isFetching: false,
    } as never);
    mockProviders.mockReturnValue({
      data: { providers: [] } as ProviderListResponseDto,
      isLoading: false,
      isError: false,
      isFetching: false,
    } as never);
    mockMutations.mockReturnValue({
      createProfile: { mutate: vi.fn(), isPending: false },
      updateProfile: { mutate: updateMutate, isPending: false },
      deleteProfile: { mutate: vi.fn(), isPending: false },
    } as never);

    renderWithProviders(<ProfilesSection />);
    fireEvent.click(screen.getByRole("button", { name: /edit/i }));
    fireEvent.click(screen.getByRole("button", { name: /cancel/i }));
    expect(screen.queryByRole("button", { name: /save/i })).toBeNull();
    expect(updateMutate).not.toHaveBeenCalled();
  });

  // ── Step 5e: save gating ──────────────────────────────────────────

  it("disables Save button when model is not in selected provider's whitelist (Step 5e)", async () => {
    mockSnapshot.mockReturnValue({
      data: {
        data: {
          subagent: {
            enabled: true,
            profiles: [makeProfile({ provider_id: null, model: "deepseek-chat" })],
          },
        },
      } as unknown as ReadEnvelopeDto<SettingsSnapshotDto>,
      isLoading: false,
      isError: false,
      isFetching: false,
    } as never);
    mockProviders.mockReturnValue({
      data: {
        providers: [makeProvider({ id: "deepseek" })],
      } as ProviderListResponseDto,
      isLoading: false,
      isError: false,
      isFetching: false,
    } as never);
    mockModelsPerProvider({
      deepseek: [makeModel({ provider_id: "deepseek", model_id: "deepseek-chat" })],
    });

    renderWithProviders(<ProfilesSection />);
    fireEvent.click(screen.getByRole("button", { name: /edit/i }));
    fireEvent.change(screen.getByLabelText(/provider/i), {
      target: { value: "deepseek" },
    });
    // Cascade auto-selects "deepseek-chat" → Save enabled.
    await waitFor(() => {
      const saveBtn = screen.getByRole("button", { name: /save/i }) as HTMLButtonElement;
      expect(saveBtn.disabled).toBe(false);
    });

    // Manually clear the model selection (set to empty option) → Save disabled.
    fireEvent.change(screen.getByLabelText(/model/i), { target: { value: "" } });
    const saveBtn = screen.getByRole("button", { name: /save/i }) as HTMLButtonElement;
    expect(saveBtn.disabled).toBe(true);
  });

  it("enables Save when a valid provider + model are selected", async () => {
    mockSnapshot.mockReturnValue({
      data: {
        data: {
          subagent: {
            enabled: true,
            profiles: [makeProfile({ provider_id: null, model: "" })],
          },
        },
      } as unknown as ReadEnvelopeDto<SettingsSnapshotDto>,
      isLoading: false,
      isError: false,
      isFetching: false,
    } as never);
    mockProviders.mockReturnValue({
      data: {
        providers: [makeProvider({ id: "deepseek" })],
      } as ProviderListResponseDto,
      isLoading: false,
      isError: false,
      isFetching: false,
    } as never);
    mockModelsPerProvider({
      deepseek: [makeModel({ provider_id: "deepseek", model_id: "deepseek-chat" })],
    });

    renderWithProviders(<ProfilesSection />);
    fireEvent.click(screen.getByRole("button", { name: /edit/i }));
    // Note: when no provider is selected (editProviderId === ""), the
    // component intentionally leaves Save ENABLED — the save-gating
    // (`isEditModelStale`) only kicks in when a provider IS selected but
    // the model is stale. The disabled-when-stale case is covered by the
    // previous test. Here we only verify the enabled-after-select case.

    // Select provider → cascade auto-selects model → Save enabled.
    fireEvent.change(screen.getByLabelText(/provider/i), {
      target: { value: "deepseek" },
    });
    await waitFor(() => {
      const saveBtn = screen.getByRole("button", { name: /save/i }) as HTMLButtonElement;
      expect(saveBtn.disabled).toBe(false);
    });
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
        },
      } as unknown as ReadEnvelopeDto<SettingsSnapshotDto>,
      isLoading: false,
      isError: false,
      isFetching: false,
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
        },
      } as unknown as ReadEnvelopeDto<SettingsSnapshotDto>,
      isLoading: false,
      isError: false,
      isFetching: false,
    } as never);
    mockMutations.mockReturnValue({
      createProfile: { mutate: vi.fn(), isPending: false },
      updateProfile: { mutate: updateMutate, isPending: false },
      deleteProfile: { mutate: vi.fn(), isPending: false },
    } as never);

    renderWithProviders(<ProfilesSection />);
    fireEvent.click(screen.getByRole("button", { name: /edit/i }));
    fireEvent.click(screen.getByRole("button", { name: /save/i }));
    expect(screen.getByText(/first error/i)).toBeTruthy();

    fireEvent.click(screen.getByRole("button", { name: /edit/i }));
    expect(screen.queryByText(/first error/i)).toBeNull();
  });

  // ── Step 5d: degraded paths ───────────────────────────────────────

  it("shows degraded banner and skips stale markers when providers query fails", () => {
    mockSnapshot.mockReturnValue({
      data: {
        data: {
          subagent: {
            enabled: true,
            profiles: [makeProfile({ provider_id: "deepseek", model: "deepseek-chat" })],
          },
        },
      } as unknown as ReadEnvelopeDto<SettingsSnapshotDto>,
      isLoading: false,
      isError: false,
      isFetching: false,
    } as never);
    mockProviders.mockReturnValue({
      data: undefined,
      isLoading: false,
      isError: true,
      isFetching: false,
    } as never);

    renderWithProviders(<ProfilesSection />);
    expect(screen.getByText(/provider list unavailable/i)).toBeTruthy();
    expect(screen.queryByText(/stale model/i)).toBeNull();
    expect(screen.queryByText(/disabled provider/i)).toBeNull();
    expect(screen.getByText("pi/search-cheap")).toBeTruthy();
  });

  it("skips stale check and enables Save when models query fails (Step 5d models-degraded)", async () => {
    mockSnapshot.mockReturnValue({
      data: {
        data: {
          subagent: {
            enabled: true,
            profiles: [makeProfile({ provider_id: "deepseek", model: "deepseek-chat" })],
          },
        },
      } as unknown as ReadEnvelopeDto<SettingsSnapshotDto>,
      isLoading: false,
      isError: false,
      isFetching: false,
    } as never);
    mockProviders.mockReturnValue({
      data: {
        providers: [makeProvider({ id: "deepseek" })],
      } as ProviderListResponseDto,
      isLoading: false,
      isError: false,
      isFetching: false,
    } as never);
    // Models query fails for every provider — display + edit paths both
    // see isError=true.
    mockUseModels.mockReturnValue({
      data: undefined,
      isLoading: false,
      isError: true,
      isFetching: false,
    } as never);

    renderWithProviders(<ProfilesSection />);
    // Display-state: no false stale marker despite model not being in
    // (the unavailable) catalog.
    expect(screen.queryByText(/stale model/i)).toBeNull();
    // Model row still shows the bound model id (not "—").
    expect(screen.getByText("deepseek-chat")).toBeTruthy();

    // Edit-state: Save is enabled even though availableModels is empty,
    // because modelsDegraded skips the save-gating check.
    fireEvent.click(screen.getByRole("button", { name: /edit/i }));
    await waitFor(() => {
      const saveBtn = screen.getByRole("button", { name: /save/i }) as HTMLButtonElement;
      expect(saveBtn.disabled).toBe(false);
    });
  });
});
