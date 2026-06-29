import { Share2 } from "lucide-react";

interface Props {
  onClick: () => void;
  disabled?: boolean;
}

export function ShareReceiptButton({ onClick, disabled }: Props) {
  return (
    <button
      type="button"
      className="refresh-button"
      onClick={onClick}
      disabled={disabled}
      aria-label="Share daily receipt"
      title="Share daily receipt"
    >
      <Share2 size={14} strokeWidth={1.75} />
    </button>
  );
}
