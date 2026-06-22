import { Component } from "react";
import { flushBuffer, reportFrontendError } from "../logging/reporter";

/** Catch rendering errors so the user sees a message, not a white screen. */
export class ErrorBoundary extends Component<
  { children: React.ReactNode },
  { hasError: boolean; error: Error | null }
> {
  state = { hasError: false, error: null } as { hasError: boolean; error: Error | null };

  static getDerivedStateFromError(error: Error) {
    return { hasError: true, error };
  }

  componentDidCatch(error: Error, info: React.ErrorInfo) {
    console.error("Render error caught by ErrorBoundary:", error.message, info.componentStack);

    reportFrontendError({
      event_code: "gui.render_error",
      message: error.message,
      details: {
        stack: error.stack,
        component_stack: info.componentStack ?? undefined,
      },
    });

    // Best-effort: trigger full buffer flush after render crash.
    void flushBuffer();
  }

  render() {
    if (this.state.hasError) {
      return (
        <div className="error-boundary" role="alert">
          <h1 className="error-boundary__title">Something went wrong</h1>
          <pre className="error-boundary__message">
            {this.state.error?.message ?? "Unknown error"}
          </pre>
          <button
            type="button"
            className="error-boundary__reload btn btn--primary"
            onClick={() => window.location.reload()}
          >
            Reload
          </button>
        </div>
      );
    }
    return this.props.children;
  }
}
