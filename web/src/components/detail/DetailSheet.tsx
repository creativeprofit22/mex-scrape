"use client";

import { useEffect, useRef, type ReactNode, type RefObject } from "react";
import { createPortal } from "react-dom";

/**
 * Marks every other child of `<body>` `inert` and returns the undo. The modal
 * already blocks them, but the explicit attribute also takes the covered page
 * out of the accessibility tree and any layout scan. Only nodes this call
 * changed are restored, so a node that was already inert stays inert.
 */
function inertBackground(dialog: HTMLDialogElement): () => void {
  const changed: Element[] = [];
  for (const node of Array.from(document.body.children)) {
    if (node === dialog || node.hasAttribute("inert")) continue;
    node.setAttribute("inert", "");
    changed.push(node);
  }
  return () => {
    for (const node of changed) node.removeAttribute("inert");
  };
}

/**
 * A native modal `<dialog>` (side sheet at 768–1199px, bottom sheet below;
 * detail.css), rendered straight into `<body>` so the rest of the page can be
 * made `inert` while it is open. `showModal()`/`close()` follow `open`; the
 * dialog's `close` event (Esc, the × or a backdrop click, which all call
 * `close()`) lifts `inert` first, then reports back through `onClose`, so the
 * row's title button can take focus again. Nothing waits on animation or
 * transition events.
 * see sources#dialog-sync
 */
export function DetailSheet({
  dialogRef: ref,
  open,
  labelledBy,
  onClose,
  children,
}: {
  /** The `<dialog>`, so the panel's × can call `close()`. */
  dialogRef: RefObject<HTMLDialogElement | null>;
  open: boolean;
  labelledBy: string;
  onClose: () => void;
  children: ReactNode;
}) {
  const onCloseRef = useRef(onClose);
  const restoreRef = useRef<(() => void) | null>(null);

  useEffect(() => {
    onCloseRef.current = onClose;
  }, [onClose]);

  useEffect(() => {
    const dialog = ref.current;
    if (!dialog) return;
    const handleClose = () => {
      restoreRef.current?.();
      restoreRef.current = null;
      onCloseRef.current();
    };
    dialog.addEventListener("close", handleClose);
    return () => dialog.removeEventListener("close", handleClose);
  }, [ref]);

  useEffect(() => {
    const dialog = ref.current;
    if (!dialog) return;
    if (!open) {
      if (dialog.open) dialog.close();
      return;
    }
    if (!dialog.open) dialog.showModal();
    restoreRef.current = inertBackground(dialog);
    // Unmounting while open (resizing to ≥1200px) must not leave the page inert.
    return () => {
      restoreRef.current?.();
      restoreRef.current = null;
    };
  }, [ref, open]);

  // DetailPanel renders nothing until the media query is known, so this only
  // runs in the browser.
  return createPortal(
    <dialog
      ref={ref}
      className="detail-sheet"
      aria-labelledby={labelledBy}
      data-testid="detail-panel"
      // The dialog box is fully covered by .detail-sheet__body, so a click
      // whose target is the dialog itself landed on the backdrop.
      onClick={(event) => {
        if (event.target === event.currentTarget) event.currentTarget.close();
      }}
    >
      <div className="detail-sheet__body">{open ? children : null}</div>
    </dialog>,
    document.body,
  );
}
