import { Fragment, type ReactNode, type Ref } from "react";

import type { Dictionary } from "@/i18n/dictionaries";
import type { ChatMessage } from "@/lib/chatHistory";

/**
 * The conversation: messages in order, with the proposals a reply produced
 * listed right after it. The status line and the alert follow the log.
 */
export function ChatLog({
  messages,
  cardsAfter,
  pending,
  error,
  logRef,
  statusRef,
  dict,
}: {
  messages: ChatMessage[];
  /** Suggestion cards to show after the message at this index, if any. */
  cardsAfter: (index: number) => ReactNode;
  pending: boolean;
  error: string | null;
  logRef: Ref<HTMLOListElement>;
  statusRef: Ref<HTMLParagraphElement>;
  dict: Dictionary["assistant"];
}) {
  return (
    <section aria-label={dict.conversation} className="chat" aria-busy={pending || undefined}>
      <ol
        ref={logRef}
        className="chat-log"
        aria-live="polite"
        aria-relevant="additions"
        data-testid="chat-log"
      >
        {messages.map((m, i) => {
          const cards = cardsAfter(i);
          return (
            <Fragment key={i}>
              <li className={m.role === "user" ? "chat-user" : "chat-bot"}>
                <span className="chat-who">{m.role === "user" ? dict.you : dict.bot}</span>
                <p className="chat-text">{m.content}</p>
              </li>
              {cards ? (
                <li className="chat-cards">
                  <section aria-labelledby={`chat-cards-${i}`}>
                    <h2 id={`chat-cards-${i}`} className="chat-cards__heading">
                      {dict.suggestionsHeading}
                    </h2>
                    <ul className="chat-cards__list">{cards}</ul>
                  </section>
                </li>
              ) : null}
            </Fragment>
          );
        })}
      </ol>
      {messages.length === 0 ? (
        <p className="hint chat-empty" data-state="empty">
          {dict.empty}
        </p>
      ) : null}
      <p ref={statusRef} role="status" className="chat-status" data-testid="chat-status">
        {pending ? dict.thinking : ""}
      </p>
      <p role="alert" className="error">
        {error}
      </p>
    </section>
  );
}
