import * as Dialog from "@radix-ui/react-dialog";

interface ConfirmDialogProps {
  open: boolean;
  title: string;
  body: string;
  detail?: string;
  confirmLabel: string;
  loading?: boolean;
  error?: string | null;
  onConfirm: () => void;
  onCancel: () => void;
}

export function ConfirmDialog({
  open,
  title,
  body,
  detail,
  confirmLabel,
  loading = false,
  error = null,
  onConfirm,
  onCancel,
}: ConfirmDialogProps) {
  return (
    <Dialog.Root
      open={open}
      onOpenChange={(next) => {
        if (!next && !loading) onCancel();
      }}
    >
      <Dialog.Portal>
        <Dialog.Overlay className="confirm-dialog__overlay" />
        <Dialog.Content className="confirm-dialog">
          <Dialog.Title className="confirm-dialog__title">{title}</Dialog.Title>
          <Dialog.Description className="confirm-dialog__body">
            {body}
          </Dialog.Description>
          {detail ? (
            <p className="confirm-dialog__detail">{detail}</p>
          ) : null}
          {error ? (
            <p className="confirm-dialog__error" role="alert">{error}</p>
          ) : null}
          <footer className="confirm-dialog__actions">
            <Dialog.Close asChild>
              <button
                type="button"
                className="btn btn--secondary"
                disabled={loading}
              >
                Cancel
              </button>
            </Dialog.Close>
            <button
              type="button"
              className="btn btn--danger"
              disabled={loading}
              onClick={onConfirm}
            >
              {loading ? "Deleting…" : confirmLabel}
            </button>
          </footer>
        </Dialog.Content>
      </Dialog.Portal>
    </Dialog.Root>
  );
}
