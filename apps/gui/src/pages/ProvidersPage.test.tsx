import { describe, it, expect, vi, beforeEach, afterEach } from "vitest";
import { render, screen, fireEvent, cleanup, waitFor } from "@testing-library/react";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import type {
  ProviderDto,
  ProviderListResponseDto,
} from "@busytok/protocol-types";

vi.mock("../api/useBusytokData", () => ({
  useProviders: vi.fn(),
  useProviderMutations: vi.fn(),
}));

// Mock the reporter so telemetry emission does not trip jsdom/Tauri invoke paths.
vi.mock("../logging/reporter", () => ({
  reportFrontendEvent: vi.fn(),
}));
vi.mock("../logging/safeReporter", () => ({
  reportFrontendEventSafely: vi.fn(),
}));

import { useProviders, useProviderMutations } from "../api/useBusytokData";
import { reportFrontendEventSafely } from "../logging/safeReporter";
import { ProvidersPage } from "./ProvidersPage";

const mockUseProviders = vi.mocked(useProviders);
const mockUseProviderMutations = vi.mocked(useProviderMutations);

function makeProvider(overrides: Partial<ProviderDto> = {}): ProviderDto {
  return {
    id: "deepseek-prod",
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

function makeListResponse(
  providers: ProviderDto[] = [],
): ProviderListResponseDto {
  return { providers };
}

// Partial mock shapes cast to the full hook return types. Tests only drive
// `data`, `isLoading`, `isError`, `isFetching` and the mutation `mutate` /
// `isPending` slots, so we cast via `never` to satisfy TypeScript without
// enumerating the full UseQueryResult / UseMutationResult surface.
type ProvidersQueryResult = ReturnType<typeof useProviders>;
type ProviderMutationsResult = ReturnType<typeof useProviderMutations>;

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
});

afterEach(() => cleanup());

describe("ProvidersPage", () => {
  it("renders empty state when no providers", () => {
    renderPage();
    expect(screen.getByText(/no providers/i)).toBeTruthy();
  });

  it("renders provider list", () => {
    mockUseProviders.mockReturnValue(
      mockProvidersQuery(
        makeListResponse([
          makeProvider({
            id: "deepseek-prod",
            name: "DeepSeek",
            base_url: "https://api.deepseek.com/v1",
            models: ["deepseek-chat"],
          }),
        ]),
      ),
    );
    renderPage();
    expect(screen.getByText("DeepSeek")).toBeTruthy();
    expect(screen.getByText("deepseek-chat")).toBeTruthy();
  });

  it("shows Add Provider button", () => {
    renderPage();
    expect(screen.getByRole("button", { name: /add provider/i })).toBeTruthy();
  });

  it("shows the add-provider form when Add Provider is clicked", () => {
    renderPage();
    fireEvent.click(screen.getByRole("button", { name: /add provider/i }));
    // Form fields visible
    expect(screen.getByLabelText(/provider id/i)).toBeTruthy();
    expect(screen.getByLabelText(/^name$/i)).toBeTruthy();
    expect(screen.getByLabelText(/base url/i)).toBeTruthy();
    expect(screen.getByLabelText(/api key env name/i)).toBeTruthy();
    expect(screen.getByLabelText(/models/i)).toBeTruthy();
    expect(screen.getByLabelText(/^api key$/i)).toBeTruthy();
  });

  it("submits the create form and calls createProvider.mutate", () => {
    const createMutate = vi.fn((_payload: unknown, opts?: { onSuccess?: () => void }) => {
      opts?.onSuccess?.();
    });
    mockUseProviderMutations.mockReturnValue(
      mockMutations({ createMutate }),
    );
    renderPage();
    fireEvent.click(screen.getByRole("button", { name: /add provider/i }));

    fireEvent.change(screen.getByLabelText(/provider id/i), {
      target: { value: "openai-prod" },
    });
    fireEvent.change(screen.getByLabelText(/^name$/i), {
      target: { value: "OpenAI" },
    });
    fireEvent.change(screen.getByLabelText(/base url/i), {
      target: { value: "https://api.openai.com/v1" },
    });
    fireEvent.change(screen.getByLabelText(/api key env name/i), {
      target: { value: "OPENAI_API_KEY" },
    });
    fireEvent.change(screen.getByLabelText(/models/i), {
      target: { value: "gpt-4, gpt-3.5-turbo" },
    });
    fireEvent.change(screen.getByLabelText(/^api key$/i), {
      target: { value: "sk-test" },
    });

    fireEvent.click(screen.getByRole("button", { name: /save/i }));

    expect(createMutate).toHaveBeenCalledTimes(1);
    const arg = createMutate.mock.calls[0][0];
    expect(arg).toMatchObject({
      id: "openai-prod",
      name: "OpenAI",
      base_url: "https://api.openai.com/v1",
      api_key_env_name: "OPENAI_API_KEY",
      models: ["gpt-4", "gpt-3.5-turbo"],
      api_key: "sk-test",
    });
    expect(vi.mocked(reportFrontendEventSafely)).toHaveBeenCalledWith(
      expect.objectContaining({ event_code: "provider.added" }),
    );
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
          onSuccess?: (r: {
            ok: boolean;
            error: string | null;
            models_detected: string[] | null;
          }) => void;
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
          onSuccess?: (r: {
            ok: boolean;
            error: string | null;
            models_detected: string[] | null;
          }) => void;
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
    expect(screen.getByText(/disabled/i)).toBeTruthy();
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
    expect(screen.getByText(/stored|has api key|api key set/i)).toBeTruthy();
    expect(screen.getByText(/not set|no api key|missing/i)).toBeTruthy();
  });

  it("toggles a provider enabled state via the toggle", () => {
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
    expect(updateMutate).toHaveBeenCalledWith(
      expect.objectContaining({ id: "deepseek-prod", enabled: false }),
      expect.anything(),
    );
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
    // Badge text rendered by PageState for the loading kind.
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
    // Badge text rendered by PageState for the error kind.
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

  it("submits API key update via Update Key button and clears input", () => {
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
    expect(updateMutate).toHaveBeenCalledWith(
      expect.objectContaining({
        id: "deepseek-prod",
        api_key: "sk-new",
        enabled: null,
      }),
      expect.anything(),
    );
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
});
