import { describe, it, expect, vi, beforeEach, afterEach } from "vitest";
import { render, screen, fireEvent, cleanup, waitFor } from "@testing-library/react";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import type {
  ProviderDto,
  ProviderListResponseDto,
  ProviderTestConnectionResponseDto,
  ModelListResponseDto,
  ReadEnvelopeDto,
  SettingsSnapshotDto,
} from "@busytok/protocol-types";

// ProvidersPage renders <ModelsSection /> and <ProfilesSection />, so every
// hook those children use must also be mocked here. The defaults returned
// by `beforeEach` keep the children in their empty/view states so provider-
// specific assertions are not polluted by child-component UI.
vi.mock("../api/useBusytokData", () => ({
  useProviders: vi.fn(),
  useProviderMutations: vi.fn(),
  useModels: vi.fn(),
  useModelMutations: vi.fn(),
  useSettingsSnapshot: vi.fn(),
  useProfileMutations: vi.fn(),
}));

vi.mock("../logging/reporter", () => ({
  reportFrontendEvent: vi.fn(),
}));
vi.mock("../logging/safeReporter", () => ({
  reportFrontendEventSafely: vi.fn(),
}));

import {
  useProviders,
  useProviderMutations,
  useModels,
  useModelMutations,
  useSettingsSnapshot,
  useProfileMutations,
} from "../api/useBusytokData";
import { reportFrontendEventSafely } from "../logging/safeReporter";
import { ProvidersPage } from "./ProvidersPage";

const mockUseProviders = vi.mocked(useProviders);
const mockUseProviderMutations = vi.mocked(useProviderMutations);
const mockUseModels = vi.mocked(useModels);
const mockUseModelMutations = vi.mocked(useModelMutations);
const mockUseSettingsSnapshot = vi.mocked(useSettingsSnapshot);
const mockUseProfileMutations = vi.mocked(useProfileMutations);

function makeProvider(overrides: Partial<ProviderDto> = {}): ProviderDto {
  return {
    id: "deepseek-prod",
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

function makeListResponse(
  providers: ProviderDto[] = [],
): ProviderListResponseDto {
  return { providers };
}

type ProvidersQueryResult = ReturnType<typeof useProviders>;
type ProviderMutationsResult = ReturnType<typeof useProviderMutations>;
type ModelsQueryResult = ReturnType<typeof useModels>;
type ModelMutationsResult = ReturnType<typeof useModelMutations>;

function mockProvidersQuery(
  data: ProviderListResponseDto,
  extras: Partial<ProvidersQueryResult> = {},
): ProvidersQueryResult {
  return {
    data,
    isLoading: false,
    isError: false,
    isFetching: false,
    ...extras,
  } as never;
}

function mockModelsQuery(
  data: ModelListResponseDto,
  extras: Partial<ModelsQueryResult> = {},
): ModelsQueryResult {
  return {
    data,
    isLoading: false,
    isError: false,
    isFetching: false,
    ...extras,
  } as never;
}

function mockMutations(
  overrides: {
    createMutate?: ReturnType<typeof vi.fn>;
    updateMutate?: ReturnType<typeof vi.fn>;
    deleteMutate?: ReturnType<typeof vi.fn>;
    testMutate?: ReturnType<typeof vi.fn>;
    createPending?: boolean;
    testPending?: boolean;
  } = {},
): ProviderMutationsResult {
  return {
    createProvider: {
      mutate: overrides.createMutate ?? vi.fn(),
      isPending: overrides.createPending ?? false,
    },
    updateProvider: {
      mutate: overrides.updateMutate ?? vi.fn(),
      isPending: false,
    },
    deleteProvider: {
      mutate: overrides.deleteMutate ?? vi.fn(),
      isPending: false,
    },
    testConnection: {
      mutate: overrides.testMutate ?? vi.fn(),
      isPending: overrides.testPending ?? false,
    },
  } as never;
}

function mockModelMutations(): ModelMutationsResult {
  return {
    createModel: { mutate: vi.fn(), isPending: false },
    updateModel: { mutate: vi.fn(), isPending: false },
    deleteModel: { mutate: vi.fn(), isPending: false },
    tagsUpdate: { mutate: vi.fn(), isPending: false },
  } as never;
}

function renderPage() {
  const qc = new QueryClient({ defaultOptions: { queries: { retry: false } } });
  return render(
    <QueryClientProvider client={qc}>
      <ProvidersPage />
    </QueryClientProvider>,
  );
}

beforeEach(() => {
  vi.clearAllMocks();
  mockUseProviders.mockReturnValue(mockProvidersQuery(makeListResponse([])));
  mockUseProviderMutations.mockReturnValue(mockMutations());
  // Empty models list keeps ModelsSection in its "no models match" state.
  mockUseModels.mockReturnValue(mockModelsQuery({ models: [] }));
  mockUseModelMutations.mockReturnValue(mockModelMutations());
  mockUseSettingsSnapshot.mockReturnValue({
    data: {
      data: {
        subagent: {
          enabled: true,
          profiles: [],
        },
      },
    } as unknown as ReadEnvelopeDto<SettingsSnapshotDto>,
    isLoading: false,
    isError: false,
    isFetching: false,
  } as never);
  mockUseProfileMutations.mockReturnValue({
    createProfile: { mutate: vi.fn(), isPending: false },
    updateProfile: { mutate: vi.fn(), isPending: false },
    deleteProfile: { mutate: vi.fn(), isPending: false },
  } as never);
});

afterEach(() => cleanup());

describe("ProvidersPage", () => {
  it("renders empty state when no providers", () => {
    renderPage();
    expect(screen.getByText(/no providers configured/i)).toBeTruthy();
  });

  it("renders provider list with name, id, kind, base_url", () => {
    mockUseProviders.mockReturnValue(
      mockProvidersQuery(
        makeListResponse([
          makeProvider({
            id: "deepseek-prod",
            name: "DeepSeek",
            base_url: "https://api.deepseek.com/v1",
            provider_kind: "openai_compatible",
          }),
        ]),
      ),
    );
    renderPage();
    expect(screen.getByText("DeepSeek")).toBeTruthy();
    expect(screen.getByText("deepseek-prod")).toBeTruthy();
    // provider_kind column is rendered (Step 3 requirement).
    expect(screen.getByText("openai_compatible")).toBeTruthy();
    expect(screen.getByText("https://api.deepseek.com/v1")).toBeTruthy();
  });

  it("shows Add Provider button", () => {
    renderPage();
    expect(screen.getByRole("button", { name: /add provider/i })).toBeTruthy();
  });

  it("shows the create form with only Name, Base URL, API Key fields (no id/env-name/models)", () => {
    renderPage();
    fireEvent.click(screen.getByRole("button", { name: /add provider/i }));
    // Editable fields present.
    expect(screen.getByLabelText(/^name$/i)).toBeTruthy();
    expect(screen.getByLabelText(/base url/i)).toBeTruthy();
    expect(screen.getByLabelText(/^api key$/i)).toBeTruthy();
    // Removed fields are NOT rendered as inputs.
    expect(screen.queryByLabelText(/provider id/i)).toBeNull();
    expect(screen.queryByLabelText(/api key env name/i)).toBeNull();
    expect(screen.queryByLabelText(/^models$/i)).toBeNull();
  });

  it("submits the create form with provider_kind hardcoded and no id/api_key_env_name/models", () => {
    const createMutate = vi.fn(
      (_payload: unknown, opts?: { onSuccess?: () => void }) => {
        opts?.onSuccess?.();
      },
    );
    mockUseProviderMutations.mockReturnValue(
      mockMutations({ createMutate }),
    );
    renderPage();
    fireEvent.click(screen.getByRole("button", { name: /add provider/i }));

    fireEvent.change(screen.getByLabelText(/^name$/i), {
      target: { value: "OpenAI" },
    });
    fireEvent.change(screen.getByLabelText(/base url/i), {
      target: { value: "https://api.openai.com/v1" },
    });
    fireEvent.change(screen.getByLabelText(/^api key$/i), {
      target: { value: "sk-test" },
    });

    fireEvent.click(screen.getByRole("button", { name: /save/i }));

    expect(createMutate).toHaveBeenCalledTimes(1);
    const arg = createMutate.mock.calls[0][0] as Record<string, unknown>;
    // Required fields.
    expect(arg).toMatchObject({
      name: "OpenAI",
      provider_kind: "openai_compatible",
      base_url: "https://api.openai.com/v1",
      api_key: "sk-test",
    });
    // Removed fields are NOT in the payload.
    expect(arg.id).toBeUndefined();
    expect(arg.api_key_env_name).toBeUndefined();
    expect(arg.base_url_env_name).toBeUndefined();
    expect(arg.models).toBeUndefined();
    expect(vi.mocked(reportFrontendEventSafely)).toHaveBeenCalledWith(
      expect.objectContaining({ event_code: "provider.added" }),
    );
  });

  it("sends api_key=null when the create form is submitted with an empty key", () => {
    const createMutate = vi.fn();
    mockUseProviderMutations.mockReturnValue(
      mockMutations({ createMutate }),
    );
    renderPage();
    fireEvent.click(screen.getByRole("button", { name: /add provider/i }));
    fireEvent.change(screen.getByLabelText(/^name$/i), {
      target: { value: "NoKey" },
    });
    fireEvent.change(screen.getByLabelText(/base url/i), {
      target: { value: "https://api.example.com/v1" },
    });
    // Leave api_key empty.
    fireEvent.click(screen.getByRole("button", { name: /save/i }));
    const arg = createMutate.mock.calls[0][0] as Record<string, unknown>;
    expect(arg.api_key).toBeNull();
  });

  it("calls deleteProvider.mutate when Delete is clicked (after confirm)", () => {
    const deleteMutate = vi.fn((_id: unknown, opts?: { onSuccess?: () => void }) => {
      opts?.onSuccess?.();
    });
    const confirmSpy = vi.spyOn(globalThis, "confirm").mockReturnValue(true);
    mockUseProviderMutations.mockReturnValue(
      mockMutations({ deleteMutate }),
    );
    mockUseProviders.mockReturnValue(
      mockProvidersQuery(
        makeListResponse([makeProvider({ id: "deepseek-prod" })]),
      ),
    );
    renderPage();
    fireEvent.click(screen.getByRole("button", { name: /delete/i }));
    expect(confirmSpy).toHaveBeenCalled();
    expect(deleteMutate).toHaveBeenCalledWith(
      "deepseek-prod",
      expect.anything(),
    );
    expect(vi.mocked(reportFrontendEventSafely)).toHaveBeenCalledWith(
      expect.objectContaining({ event_code: "provider.deleted" }),
    );
    confirmSpy.mockRestore();
  });

  it("does not call deleteProvider.mutate when confirm is cancelled", () => {
    const deleteMutate = vi.fn();
    const confirmSpy = vi.spyOn(globalThis, "confirm").mockReturnValue(false);
    mockUseProviderMutations.mockReturnValue(
      mockMutations({ deleteMutate }),
    );
    mockUseProviders.mockReturnValue(
      mockProvidersQuery(makeListResponse([makeProvider()])),
    );
    renderPage();
    fireEvent.click(screen.getByRole("button", { name: /delete/i }));
    expect(confirmSpy).toHaveBeenCalled();
    expect(deleteMutate).not.toHaveBeenCalled();
    confirmSpy.mockRestore();
  });

  it("calls testConnection.mutate and shows success result when Test Connection is clicked", async () => {
    const testMutate = vi.fn(
      (
        _id: string,
        opts?: {
          onSuccess?: (r: ProviderTestConnectionResponseDto) => void;
        },
      ) => {
        opts?.onSuccess?.({ ok: true, error: null, models_detected: null });
      },
    );
    mockUseProviderMutations.mockReturnValue(
      mockMutations({ testMutate: testMutate as never }),
    );
    mockUseProviders.mockReturnValue(
      mockProvidersQuery(
        makeListResponse([makeProvider({ id: "deepseek-prod" })]),
      ),
    );
    renderPage();
    fireEvent.click(screen.getByRole("button", { name: /test connection/i }));
    expect(testMutate).toHaveBeenCalledWith(
      "deepseek-prod",
      expect.objectContaining({}),
    );
    await waitFor(() => {
      expect(screen.getByText(/✓|connected|success/i)).toBeTruthy();
    });
    expect(vi.mocked(reportFrontendEventSafely)).toHaveBeenCalledWith(
      expect.objectContaining({ event_code: "provider.tested" }),
    );
  });

  it("shows failure result when Test Connection returns ok=false", async () => {
    const testMutate = vi.fn(
      (
        _id: string,
        opts?: {
          onSuccess?: (r: ProviderTestConnectionResponseDto) => void;
        },
      ) => {
        opts?.onSuccess?.({
          ok: false,
          error: "Invalid API key",
          models_detected: null,
        });
      },
    );
    mockUseProviderMutations.mockReturnValue(
      mockMutations({ testMutate: testMutate as never }),
    );
    mockUseProviders.mockReturnValue(
      mockProvidersQuery(makeListResponse([makeProvider()])),
    );
    renderPage();
    fireEvent.click(screen.getByRole("button", { name: /test connection/i }));
    await waitFor(() => {
      expect(screen.getByText(/✗|failed/i)).toBeTruthy();
    });
  });

  it("shows a warning when a provider is disabled", () => {
    mockUseProviders.mockReturnValue(
      mockProvidersQuery(
        makeListResponse([makeProvider({ enabled: false })]),
      ),
    );
    renderPage();
    // Use the specific warning text — bare `/disabled/i` also matches the
    // ModelsSection "Show disabled" toggle label and aria-label.
    expect(
      screen.getByText(/this provider is disabled and will not be used/i),
    ).toBeTruthy();
  });

  it("shows API key status badge (stored / not set)", () => {
    mockUseProviders.mockReturnValue(
      mockProvidersQuery(
        makeListResponse([
          makeProvider({ id: "with-key", name: "WithKey", has_api_key: true }),
          makeProvider({ id: "no-key", name: "NoKey", has_api_key: false }),
        ]),
      ),
    );
    renderPage();
    expect(screen.getByText(/stored|api key set/i)).toBeTruthy();
    expect(screen.getByText(/not set|missing/i)).toBeTruthy();
  });

  it("toggles a provider enabled state via the toggle (payload only has id + enabled)", () => {
    const updateMutate = vi.fn();
    mockUseProviderMutations.mockReturnValue(
      mockMutations({ updateMutate }),
    );
    mockUseProviders.mockReturnValue(
      mockProvidersQuery(
        makeListResponse([makeProvider({ id: "deepseek-prod", enabled: true })]),
      ),
    );
    renderPage();
    const toggle = screen.getByRole("checkbox", {
      name: /enable/i,
    }) as HTMLInputElement;
    fireEvent.click(toggle);
    expect(updateMutate).toHaveBeenCalledTimes(1);
    const arg = updateMutate.mock.calls[0][0] as Record<string, unknown>;
    // Three-state contract: only id + enabled in the patch; omitted fields
    // (name, base_url, api_key) are absent so the backend preserves them.
    expect(arg).toMatchObject({ id: "deepseek-prod", enabled: false });
    expect(arg.name).toBeUndefined();
    expect(arg.base_url).toBeUndefined();
    expect(arg.api_key).toBeUndefined();
  });

  it("clicking Edit shows the inline form pre-filled (Name + Base URL only); Provider ID is read-only", () => {
    mockUseProviders.mockReturnValue(
      mockProvidersQuery(
        makeListResponse([
          makeProvider({
            id: "deepseek-prod",
            name: "DeepSeek",
            base_url: "https://api.deepseek.com/v1",
            provider_kind: "openai_compatible",
          }),
        ]),
      ),
    );
    renderPage();
    fireEvent.click(screen.getByRole("button", { name: /^edit$/i }));

    // Editable fields pre-filled from the provider.
    expect((screen.getByLabelText(/^name$/i) as HTMLInputElement).value).toBe(
      "DeepSeek",
    );
    expect(
      (screen.getByLabelText(/base url/i) as HTMLInputElement).value,
    ).toBe("https://api.deepseek.com/v1");
    // Provider ID is shown read-only as text (not as an input).
    expect(screen.getByText("deepseek-prod")).toBeTruthy();
    expect(screen.queryByLabelText(/provider id/i)).toBeNull();
    // api_key has its own Update Key flow — NOT in the edit form.
    expect(screen.queryByLabelText(/^api key$/i)).toBeNull();
    // Removed fields are NOT in the edit form either.
    expect(screen.queryByLabelText(/api key env name/i)).toBeNull();
    expect(screen.queryByLabelText(/^models$/i)).toBeNull();
  });

  it("submitting the edit form calls updateProvider.mutate with only id+name+base_url (omitted api_key/enabled)", () => {
    const updateMutate = vi.fn((_payload: unknown, opts?: { onSuccess?: () => void }) => {
      opts?.onSuccess?.();
    });
    mockUseProviderMutations.mockReturnValue(mockMutations({ updateMutate }));
    mockUseProviders.mockReturnValue(
      mockProvidersQuery(
        makeListResponse([
          makeProvider({
            id: "deepseek-prod",
            name: "DeepSeek",
            base_url: "https://api.deepseek.com/v1",
          }),
        ]),
      ),
    );
    renderPage();
    fireEvent.click(screen.getByRole("button", { name: /^edit$/i }));

    fireEvent.change(screen.getByLabelText(/^name$/i), {
      target: { value: "DeepSeek Renamed" },
    });

    fireEvent.click(screen.getByRole("button", { name: /save/i }));

    expect(updateMutate).toHaveBeenCalledTimes(1);
    const arg = updateMutate.mock.calls[0][0] as Record<string, unknown>;
    // Three-state api_key contract: only id + name + base_url in the patch.
    expect(arg).toMatchObject({
      id: "deepseek-prod",
      name: "DeepSeek Renamed",
      base_url: "https://api.deepseek.com/v1",
    });
    // Omitted fields are absent (no api_key, no enabled, no env names, no models).
    expect(arg.api_key).toBeUndefined();
    expect(arg.enabled).toBeUndefined();
    expect(arg.api_key_env_name).toBeUndefined();
    expect(arg.models).toBeUndefined();
    // On success the row exits edit mode (form fields gone).
    expect(screen.queryByLabelText(/^name$/i)).toBeNull();
  });

  it("clicking Cancel in the edit form reverts to view mode", () => {
    mockUseProviders.mockReturnValue(
      mockProvidersQuery(
        makeListResponse([makeProvider({ id: "deepseek-prod", name: "DeepSeek" })]),
      ),
    );
    renderPage();
    fireEvent.click(screen.getByRole("button", { name: /^edit$/i }));
    expect(screen.getByLabelText(/^name$/i)).toBeTruthy();
    fireEvent.click(screen.getByRole("button", { name: /cancel/i }));
    expect(screen.queryByLabelText(/^name$/i)).toBeNull();
  });

  it("renders loading state", () => {
    mockUseProviders.mockReturnValue({
      data: undefined,
      isLoading: true,
      isError: false,
      isFetching: false,
    } as never);
    renderPage();
    expect(screen.getByText("Loading providers...")).toBeTruthy();
    expect(screen.getByText("Providers")).toBeTruthy();
    expect(screen.getByText("Loading")).toBeTruthy();
  });

  it("renders error state", () => {
    mockUseProviders.mockReturnValue({
      data: undefined,
      isLoading: false,
      isError: true,
      isFetching: false,
    } as never);
    renderPage();
    expect(screen.getByText("Providers unavailable")).toBeTruthy();
    expect(screen.getByText("Could not load providers.")).toBeTruthy();
    expect(screen.getByText("Error")).toBeTruthy();
  });

  it("shows stored key placeholder for provider with API key", () => {
    mockUseProviders.mockReturnValue(
      mockProvidersQuery(
        makeListResponse([
          makeProvider({ id: "with-key", name: "WithKey", has_api_key: true }),
        ]),
      ),
    );
    renderPage();
    const input = screen.getByPlaceholderText("•••• (stored)");
    expect(input).toBeTruthy();
    expect((input as HTMLInputElement).type).toBe("password");
  });

  it("shows enter key placeholder for provider without API key", () => {
    mockUseProviders.mockReturnValue(
      mockProvidersQuery(
        makeListResponse([
          makeProvider({ id: "no-key", name: "NoKey", has_api_key: false }),
        ]),
      ),
    );
    renderPage();
    const input = screen.getByPlaceholderText("Enter API key");
    expect(input).toBeTruthy();
    expect((input as HTMLInputElement).type).toBe("password");
  });

  it("submits API key update via Update Key button (payload only has id + api_key) and clears input", () => {
    const updateMutate = vi.fn();
    mockUseProviderMutations.mockReturnValue(
      mockMutations({ updateMutate }),
    );
    mockUseProviders.mockReturnValue(
      mockProvidersQuery(
        makeListResponse([
          makeProvider({ id: "deepseek-prod", name: "DeepSeek", has_api_key: true }),
        ]),
      ),
    );
    renderPage();
    const input = screen.getByPlaceholderText("•••• (stored)") as HTMLInputElement;
    fireEvent.change(input, { target: { value: "sk-new" } });
    fireEvent.click(screen.getByRole("button", { name: /update key/i }));
    expect(updateMutate).toHaveBeenCalledTimes(1);
    const arg = updateMutate.mock.calls[0][0] as Record<string, unknown>;
    // Three-state contract: only id + api_key in the patch.
    expect(arg).toMatchObject({ id: "deepseek-prod", api_key: "sk-new" });
    expect(arg.name).toBeUndefined();
    expect(arg.base_url).toBeUndefined();
    expect(arg.enabled).toBeUndefined();
    expect(input.value).toBe("");
  });

  it("submits API key update via Enter key", () => {
    const updateMutate = vi.fn();
    mockUseProviderMutations.mockReturnValue(
      mockMutations({ updateMutate }),
    );
    mockUseProviders.mockReturnValue(
      mockProvidersQuery(
        makeListResponse([
          makeProvider({ id: "deepseek-prod", name: "DeepSeek", has_api_key: false }),
        ]),
      ),
    );
    renderPage();
    const input = screen.getByPlaceholderText("Enter API key") as HTMLInputElement;
    fireEvent.change(input, { target: { value: "sk-enter" } });
    fireEvent.keyDown(input, { key: "Enter" });
    expect(updateMutate).toHaveBeenCalledWith(
      expect.objectContaining({
        id: "deepseek-prod",
        api_key: "sk-enter",
      }),
      expect.anything(),
    );
    expect(input.value).toBe("");
  });

  it("does not call updateProvider.mutate when API key input is empty", () => {
    const updateMutate = vi.fn();
    mockUseProviderMutations.mockReturnValue(
      mockMutations({ updateMutate }),
    );
    mockUseProviders.mockReturnValue(
      mockProvidersQuery(
        makeListResponse([
          makeProvider({ id: "deepseek-prod", name: "DeepSeek", has_api_key: true }),
        ]),
      ),
    );
    renderPage();
    fireEvent.click(screen.getByRole("button", { name: /update key/i }));
    expect(updateMutate).not.toHaveBeenCalled();
  });

  it("clears and hides the form when Cancel is clicked", () => {
    renderPage();
    fireEvent.click(screen.getByRole("button", { name: /add provider/i }));
    expect(screen.getByLabelText(/^name$/i)).toBeTruthy();
    fireEvent.click(screen.getByRole("button", { name: /cancel/i }));
    expect(screen.queryByLabelText(/^name$/i)).toBeNull();
  });

  it("shows Testing state when Test Connection is pending", () => {
    const testMutate = vi.fn();
    mockUseProviderMutations.mockReturnValue(
      mockMutations({ testMutate: testMutate as never, testPending: true }),
    );
    mockUseProviders.mockReturnValue(
      mockProvidersQuery(
        makeListResponse([makeProvider({ id: "deepseek-prod" })]),
      ),
    );
    renderPage();
    fireEvent.click(screen.getByRole("button", { name: /test connection/i }));
    expect(screen.getByRole("button", { name: /testing/i })).toBeTruthy();
  });

  // ── onError paths ──────────────────────────────────────────────

  it("shows error when createProvider fails", () => {
    const createMutate = vi.fn(
      (_payload: unknown, opts?: { onError?: (e: Error) => void }) => {
        opts?.onError?.(new Error("create failed"));
      },
    );
    mockUseProviderMutations.mockReturnValue(mockMutations({ createMutate }));
    renderPage();
    fireEvent.click(screen.getByRole("button", { name: /add provider/i }));
    fireEvent.change(screen.getByLabelText(/^name$/i), {
      target: { value: "OpenAI" },
    });
    fireEvent.change(screen.getByLabelText(/base url/i), {
      target: { value: "https://api.openai.com/v1" },
    });
    fireEvent.click(screen.getByRole("button", { name: /save/i }));
    expect(screen.getByText("create failed")).toBeTruthy();
  });

  it("shows error when toggle fails", () => {
    const updateMutate = vi.fn(
      (_payload: unknown, opts?: { onError?: (e: Error) => void }) => {
        opts?.onError?.(new Error("toggle failed"));
      },
    );
    mockUseProviderMutations.mockReturnValue(mockMutations({ updateMutate }));
    mockUseProviders.mockReturnValue(
      mockProvidersQuery(
        makeListResponse([makeProvider({ id: "deepseek-prod", enabled: true })]),
      ),
    );
    renderPage();
    fireEvent.click(screen.getByRole("checkbox", { name: /enable/i }));
    expect(screen.getByText("toggle failed")).toBeTruthy();
  });

  it("shows error when API key update fails", () => {
    const updateMutate = vi.fn(
      (_payload: unknown, opts?: { onError?: (e: Error) => void }) => {
        opts?.onError?.(new Error("key update failed"));
      },
    );
    mockUseProviderMutations.mockReturnValue(mockMutations({ updateMutate }));
    mockUseProviders.mockReturnValue(
      mockProvidersQuery(
        makeListResponse([makeProvider({ id: "deepseek-prod", has_api_key: true })]),
      ),
    );
    renderPage();
    const input = screen.getByPlaceholderText("•••• (stored)") as HTMLInputElement;
    fireEvent.change(input, { target: { value: "sk-new" } });
    fireEvent.click(screen.getByRole("button", { name: /update key/i }));
    expect(screen.getByText("key update failed")).toBeTruthy();
  });

  it("shows error when delete fails", () => {
    const deleteMutate = vi.fn(
      (_id: unknown, opts?: { onError?: (e: Error) => void }) => {
        opts?.onError?.(new Error("delete failed"));
      },
    );
    const confirmSpy = vi.spyOn(globalThis, "confirm").mockReturnValue(true);
    mockUseProviderMutations.mockReturnValue(mockMutations({ deleteMutate }));
    mockUseProviders.mockReturnValue(
      mockProvidersQuery(makeListResponse([makeProvider({ id: "deepseek-prod" })])),
    );
    renderPage();
    fireEvent.click(screen.getByRole("button", { name: /delete/i }));
    expect(screen.getByText("delete failed")).toBeTruthy();
    confirmSpy.mockRestore();
  });

  it("shows failure result when Test Connection errors", async () => {
    const testMutate = vi.fn(
      (_id: string, opts?: { onError?: (e: Error) => void }) => {
        opts?.onError?.(new Error("network error"));
      },
    );
    mockUseProviderMutations.mockReturnValue(
      mockMutations({ testMutate: testMutate as never }),
    );
    mockUseProviders.mockReturnValue(
      mockProvidersQuery(
        makeListResponse([makeProvider({ id: "deepseek-prod" })]),
      ),
    );
    renderPage();
    fireEvent.click(screen.getByRole("button", { name: /test connection/i }));
    await waitFor(() => {
      expect(screen.getByText(/✗|failed/i)).toBeTruthy();
    });
  });

  it("shows error when inline edit submit fails (handleEditSubmit onError)", () => {
    const updateMutate = vi.fn(
      (_payload: unknown, opts?: { onError?: (e: Error) => void }) => {
        opts?.onError?.(new Error("edit submit failed"));
      },
    );
    mockUseProviderMutations.mockReturnValue(mockMutations({ updateMutate }));
    mockUseProviders.mockReturnValue(
      mockProvidersQuery(
        makeListResponse([
          makeProvider({ id: "deepseek-prod", name: "DeepSeek", enabled: true }),
        ]),
      ),
    );
    renderPage();
    fireEvent.click(screen.getByRole("button", { name: /^edit$/i }));
    fireEvent.click(screen.getByRole("button", { name: /save/i }));
    expect(screen.getByText("edit submit failed")).toBeTruthy();
  });
});
