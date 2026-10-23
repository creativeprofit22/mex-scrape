/** Which decision is being saved, if any. */
export type SavingDecision = "accept" | "reject" | null;

/**
 * Confirmar (primary) and Descartar (secondary). While one is saving both
 * are disabled and the busy one says so. What each click does is up to the
 * caller: in the assistant, Descartar only changes local state.
 */
export function MatchDecisionButtons({
  saving,
  onConfirm,
  onReject,
  labels,
}: {
  saving: SavingDecision;
  onConfirm: () => void;
  onReject: () => void;
  labels: {
    confirm: string;
    reject: string;
    saving: string;
    /** Accessible names when several cards share the same visible labels. */
    confirmName?: string;
    rejectName?: string;
  };
}) {
  return (
    <div className="match__actions">
      <button
        type="button"
        disabled={saving !== null}
        aria-busy={saving === "accept" || undefined}
        aria-label={saving === "accept" ? undefined : labels.confirmName}
        onClick={onConfirm}
      >
        {saving === "accept" ? labels.saving : labels.confirm}
      </button>
      <button
        type="button"
        className="secondary"
        disabled={saving !== null}
        aria-busy={saving === "reject" || undefined}
        aria-label={saving === "reject" ? undefined : labels.rejectName}
        onClick={onReject}
      >
        {saving === "reject" ? labels.saving : labels.reject}
      </button>
    </div>
  );
}
