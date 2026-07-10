import { useMemo, useState } from "react";
import type {
  ModelCreateRequestDto,
  ProviderCreateRequestDto,
  ProviderDto,
  ProviderKind,
} from "@busytok/protocol-types";
import { useModelMutations, useProviderMutations, useProviders } from "../api/useBusytokData";
import { reportFrontendEventSafely } from "../logging/safeReporter";
import {
  deriveUniqueProviderName,
  parseTags,
  validateBaseUrl,
} from "../pages/providerFormUtils";

interface ProviderCreationFormProps {
  onClose: () => void;
}

/**
 * Submit state machine for the create-provider (+ optional inline model)
 * flow. Per spec §3 partial-success:
 *   - `idle`            → form is fresh, can submit
 *   - `provider-creating` → request in flight (currently unused for
 *                          disabling; the mock invoke is synchronous)
 *   - `partial-success` → provider created, model failed; Save disabled,
 *                          Retry Model enabled
 *   - `provider-failed` → provider.create errored; user can fix inputs
 *                          and click Save again
 *
 * There is no `success` state because on full success the form is closed
 * via `onClose` and unmounted.
 */
type SubmitState =
  | { kind: "idle" }
  | { kind: "provider-creating" }
  | { kind: "partial-success"; provider: ProviderDto; modelError: string }
  | { kind: "provider-failed"; error: string };

export function ProviderCreationForm({ onClose }: ProviderCreationFormProps) {
  const providersQuery = useProviders();
  const { createProvider } = useProviderMutations();
  const { createModel } = useModelMutations();

  const [baseUrl, setBaseUrl] = useState("");
  const [apiKey, setApiKey] = useState("");
  const [kind, setKind] = useState<ProviderKind>("openai_compatible");
  const [modelName, setModelName] = useState("");
  const [modelTags, setModelTags] = useState("");
  const [urlError, setUrlError] = useState<string | null>(null);
  const [state, setState] = useState<SubmitState>({ kind: "idle" });

  const existingNames = useMemo(
    () => new Set((providersQuery.data?.providers ?? []).map((p) => p.name)),
    [providersQuery.data],
  );

  // Save is enabled only when the form is in a submittable state (idle or
  // provider-failed) and the required fields are valid. In `partial-success`
  // the provider already exists — re-submitting would create a duplicate.
  // Also disabled while a mutation is in-flight (f4).
  const isMutationPending = createProvider.isPending || createModel.isPending;
  const canSubmit =
    validateBaseUrl(baseUrl) === null &&
    apiKey.trim().length > 0 &&
    (state.kind === "idle" || state.kind === "provider-failed") &&
    !isMutationPending;

  const handleBlurUrl = () => setUrlError(validateBaseUrl(baseUrl));

  const buildProviderPayload = (): ProviderCreateRequestDto => {
    const name = deriveUniqueProviderName(baseUrl, kind, existingNames);
    return {
      name,
      provider_kind: kind,
      base_url: baseUrl.trim(),
      api_key: apiKey,
      enabled: true,
    };
  };

  const buildModelPayload = (providerId: string): ModelCreateRequestDto => ({
    provider_id: providerId,
    model_id: modelName.trim(),
    display_name: modelName.trim(),
    context_window: 200000,
    max_tokens: 8192,
    reasoning: true,
    enabled: true,
    tags: parseTags(modelTags),
  });

  const reportModelError = (providerId: string, err: Error) => {
    reportFrontendEventSafely({
      level: "ERROR",
      event_code: "model.add.failed",
      message: "Model creation failed",
      details: { provider_id: providerId, model_id: modelName.trim(), error: err.message },
    });
  };

  const reportModelAdded = (providerId: string) => {
    reportFrontendEventSafely({
      level: "INFO",
      event_code: "model.added",
      message: "Model added",
      details: { provider_id: providerId, model_id: modelName.trim() },
    });
  };

  const handleProviderSuccess = (provider: ProviderDto) => {
    reportFrontendEventSafely({
      level: "INFO",
      event_code: "provider.added",
      message: "Provider added",
      details: { name: provider.name },
    });

    if (!modelName.trim()) {
      // No model to create — full success
      onClose();
      return;
    }

    // Try to create the inline model. Failure here is recoverable: the
    // form stays open in `partial-success` so the user can fix the model
    // name and retry just `model.create` without re-creating the provider.
    createModel.mutate(buildModelPayload(provider.id), {
      onSuccess: () => {
        reportModelAdded(provider.id);
        onClose();
      },
      onError: (err: Error) => {
        reportModelError(provider.id, err);
        setState({ kind: "partial-success", provider, modelError: err.message });
      },
    });
  };

  const handleProviderError = (err: Error) => {
    reportFrontendEventSafely({
      level: "ERROR",
      event_code: "provider.add.failed",
      message: "Provider creation failed",
      details: { error: err.message },
    });
    setState({ kind: "provider-failed", error: err.message });
  };

  const handleSubmit = () => {
    const urlErr = validateBaseUrl(baseUrl);
    if (urlErr) {
      setUrlError(urlErr);
      return;
    }
    if (!apiKey.trim()) return;

    setState({ kind: "provider-creating" });
    createProvider.mutate(buildProviderPayload(), {
      onSuccess: handleProviderSuccess,
      onError: handleProviderError,
    });
  };

  const handleRetryModel = () => {
    if (state.kind !== "partial-success") return;
    const provider = state.provider;
    createModel.mutate(buildModelPayload(provider.id), {
      onSuccess: () => {
        reportModelAdded(provider.id);
        onClose();
      },
      onError: (err: Error) => {
        reportModelError(provider.id, err);
        setState({ kind: "partial-success", provider, modelError: err.message });
      },
    });
  };

  return (
    <div className="provider-card">
      <div className="provider-card__header">
        <strong>新建 Provider</strong>
      </div>
      <div className="provider-card__body">
        <div className="field-group">
          <label className="field-label" htmlFor="new-prov-url">Base URL</label>
          <input
            id="new-prov-url"
            className="field-input"
            type="text"
            placeholder="Base URL (https://...)"
            value={baseUrl}
            aria-invalid={urlError !== null}
            onChange={(e) => setBaseUrl(e.target.value)}
            onBlur={handleBlurUrl}
          />
        </div>
        {urlError && (
          <div className="field-error" role="alert">{urlError}</div>
        )}
        <div className="field-group">
          <label className="field-label" htmlFor="new-prov-key">API Key</label>
          <input
            id="new-prov-key"
            className="field-input"
            type="password"
            placeholder="API Key"
            value={apiKey}
            onChange={(e) => setApiKey(e.target.value)}
          />
        </div>
        <div className="field-group">
          <label className="field-label" htmlFor="new-prov-kind">Kind</label>
          <select
            id="new-prov-kind"
            className="field-select"
            value={kind}
            onChange={(e) => setKind(e.target.value as ProviderKind)}
          >
            <option value="openai_compatible">openai_compatible</option>
            <option value="anthropic_compatible">anthropic_compatible</option>
          </select>
        </div>
        <hr />
        <div>同步创建 Model</div>
        <div className="field-group">
          <label className="field-label" htmlFor="new-prov-model-name">Model Name</label>
          <input
            id="new-prov-model-name"
            className="field-input"
            type="text"
            placeholder="model name (optional)"
            value={modelName}
            onChange={(e) => setModelName(e.target.value)}
          />
        </div>
        <div className="field-group">
          <label className="field-label" htmlFor="new-prov-model-tags">Model Tags</label>
          <input
            id="new-prov-model-tags"
            className="field-input"
            type="text"
            placeholder="tags (comma-separated, optional)"
            value={modelTags}
            onChange={(e) => setModelTags(e.target.value)}
          />
        </div>

        {state.kind === "partial-success" && (
          <div className="provider-card__error-banner" role="alert">
            Provider 已创建，但 Model 创建失败：{state.modelError}
          </div>
        )}
        {state.kind === "provider-failed" && (
          <div className="provider-card__error-banner" role="alert">
            Provider 创建失败：{state.error}
          </div>
        )}

        <div className="provider-card__actions">
          <button type="button" className="btn btn--primary" onClick={handleSubmit} disabled={!canSubmit}>
            保存
          </button>
          {state.kind === "partial-success" && (
            <button type="button" className="btn btn--secondary" onClick={handleRetryModel} disabled={isMutationPending}>
              重试 Model
            </button>
          )}
          <button type="button" className="btn btn--secondary" onClick={onClose} disabled={isMutationPending}>
            取消
          </button>
        </div>
      </div>
    </div>
  );
}
