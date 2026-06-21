import * as Dialog from "@radix-ui/react-dialog";
import { X } from "lucide-react";
import type { ReactNode } from "react";

interface DetailDrawerProps {
  open: boolean;
  title: string;
  description?: string;
  onClose: () => void;
  children: ReactNode;
}

const VISUALLY_HIDDEN_STYLE = {
  position: "absolute" as const,
  width: "1px",
  height: "1px",
  padding: 0,
  margin: "-1px",
  overflow: "hidden",
  clip: "rect(0, 0, 0, 0)",
  whiteSpace: "nowrap" as const,
  border: 0,
};

export function DetailDrawer({ open, title, description, onClose, children }: DetailDrawerProps) {
  const resolvedDescription = description ?? "Details for the selected item.";

  return (
    <Dialog.Root
      modal={false}
      open={open}
      onOpenChange={(next) => {
        if (!next) onClose();
      }}
    >
      <Dialog.Portal>
        <Dialog.Overlay className="detail-drawer__overlay" aria-hidden="true" />
        <Dialog.Content className="detail-drawer">
          <header className="detail-drawer__header">
            <div>
              <Dialog.Title>{title}</Dialog.Title>
              <Dialog.Description style={description ? undefined : VISUALLY_HIDDEN_STYLE}>
                {resolvedDescription}
              </Dialog.Description>
            </div>
            <Dialog.Close className="desktop-icon-button" aria-label="Close detail">
              <X size={16} />
            </Dialog.Close>
          </header>
          <div className="detail-drawer__body">{children}</div>
        </Dialog.Content>
      </Dialog.Portal>
    </Dialog.Root>
  );
}
