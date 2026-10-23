"use client";

import { useId, type FormEvent, type KeyboardEvent, type Ref } from "react";

import type { Dictionary } from "@/i18n/dictionaries";
import { MAX_MESSAGE_CHARS } from "@/lib/chatHistory";

/**
 * The message box, pinned to the bottom of the workspace. Enter sends,
 * Shift+Enter starts a new line.
 */
export function ChatComposer({
  draft,
  onDraft,
  onSend,
  onReset,
  pending,
  canReset,
  formRef,
  dict,
}: {
  draft: string;
  onDraft: (value: string) => void;
  onSend: (event: FormEvent<HTMLFormElement>) => void;
  onReset: () => void;
  pending: boolean;
  canReset: boolean;
  formRef: Ref<HTMLFormElement>;
  dict: Dictionary["assistant"];
}) {
  const inputId = useId();

  function onKeyDown(event: KeyboardEvent<HTMLTextAreaElement>) {
    if (event.key === "Enter" && !event.shiftKey && !event.nativeEvent.isComposing) {
      event.preventDefault();
      event.currentTarget.form?.requestSubmit();
    }
  }

  return (
    <form ref={formRef} className="chat-composer" onSubmit={onSend}>
      <label htmlFor={inputId} className="chat-composer__label">
        {dict.label}
      </label>
      <span className="field chat-composer__field">
        <textarea
          id={inputId}
          name="message"
          rows={2}
          required
          maxLength={MAX_MESSAGE_CHARS}
          placeholder={dict.placeholder}
          value={draft}
          onChange={(e) => onDraft(e.target.value)}
          onKeyDown={onKeyDown}
        />
      </span>
      <div className="chat-composer__actions">
        <button type="submit" disabled={pending || draft.trim() === ""}>
          {dict.send}
        </button>
        <button type="button" className="secondary" onClick={onReset} disabled={!canReset}>
          {dict.newChat}
        </button>
      </div>
    </form>
  );
}
