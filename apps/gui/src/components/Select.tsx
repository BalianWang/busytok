import * as RadixSelect from "@radix-ui/react-select";
import { useRef, type ReactNode } from "react";

interface AppSelectProps {
  value: string;
  onValueChange: (value: string) => void;
  label: string;
  "aria-label"?: string;
  children: ReactNode;
  size?: "default" | "dense";
}

export function AppSelect({
  value,
  onValueChange,
  label,
  "aria-label": ariaLabel,
  children,
  size = "default",
}: AppSelectProps) {
  const triggerRef = useRef<HTMLButtonElement>(null);

  return (
    <div className="app-select">
      <span
        className="app-select__label"
        onClick={() => triggerRef.current?.focus()}
      >
        {label}
      </span>
      <RadixSelect.Root value={value} onValueChange={onValueChange}>
        <RadixSelect.Trigger asChild aria-label={ariaLabel ?? label}>
          <button
            type="button"
            ref={triggerRef}
            className={`app-select__trigger app-select__trigger--${size}`}
          >
            <RadixSelect.Value />
            <RadixSelect.Icon className="app-select__icon" aria-hidden>
              <svg
                width="12"
                height="12"
                viewBox="0 0 12 12"
                fill="none"
                stroke="currentColor"
                strokeWidth="1.5"
                strokeLinecap="round"
                strokeLinejoin="round"
              >
                <path d="M3 4.5L6 7.5L9 4.5" />
              </svg>
            </RadixSelect.Icon>
          </button>
        </RadixSelect.Trigger>
        <RadixSelect.Portal>
          <RadixSelect.Content
            className="app-select__content"
            position="popper"
            sideOffset={4}
          >
            <RadixSelect.Viewport>
              {children}
            </RadixSelect.Viewport>
          </RadixSelect.Content>
        </RadixSelect.Portal>
      </RadixSelect.Root>
    </div>
  );
}

interface AppSelectItemProps {
  value: string;
  children: ReactNode;
  disabled?: boolean;
}

export function AppSelectItem({ value, children, disabled }: AppSelectItemProps) {
  return (
    <RadixSelect.Item value={value} disabled={disabled} className="app-select__item">
      <RadixSelect.ItemIndicator className="app-select__check" aria-hidden>
        <svg width="12" height="12" viewBox="0 0 12 12" fill="none" stroke="currentColor" strokeWidth="2" strokeLinecap="round" strokeLinejoin="round">
          <path d="M2 6L5 9L10 3" />
        </svg>
      </RadixSelect.ItemIndicator>
      <RadixSelect.ItemText>{children}</RadixSelect.ItemText>
    </RadixSelect.Item>
  );
}
