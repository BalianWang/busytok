import type { InputHTMLAttributes } from "react";

type NativeTextAssistDisabledProps = Pick<
  InputHTMLAttributes<HTMLInputElement>,
  "autoCapitalize" | "autoComplete" | "autoCorrect" | "spellCheck"
> & {
  "data-form-type": "other";
};

// Suppress WebKit/macOS native input suggestions so prompt search and tag
// fields only show the app's own suggestion UI.
export const nativeTextAssistDisabledProps: NativeTextAssistDisabledProps = {
  autoCapitalize: "off",
  autoComplete: "off",
  autoCorrect: "off",
  spellCheck: false,
  "data-form-type": "other",
};
