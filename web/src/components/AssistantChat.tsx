"use client";

import { useRouter } from "next/navigation";
import { useId, useRef, useState, type FormEvent, type KeyboardEvent } from "react";

import type { Locale } from "@/i18n/config";
import type { Dictionary } from "@/i18n/dictionaries";
import { interpolate } from "@/i18n/format";
import { api, type Schemas } from "@/lib/api/client";
import { buildChatHistory, MAX_MESSAGE_CHARS, type ChatMessage } from "@/lib/chatHistory";
import { errorMessage, retailerName, signInPath } from "@/lib/ui";
import { useRequireOnboarded } from "@/lib/useRequireOnboarded";

type Message = ChatMessage;
type Suggestion = Schemas["MatchSuggestionResponse"];
type CardState = { status: "open" } | { status: "busy" } | { status: "done"; note: string } | { status: "error"; note: string };

/**
 * Chat with the gg-agent assistant (`POST /assistant/chat`). The reply is
 * rendered as plain text. Match proposals become cards; only the user's
 * Confirmar click writes a decision (`POST /matches/decisions`), and
 * Descartar changes nothing.
 */
export function AssistantChat({
  lang,
  dict,
  common,
  retailers,
}: {
  lang: Locale;
  dict: Dictionary["assistant"];
  common: Dictionary["common"];
  retailers: Dictionary["retailers"];
}) {
  const router = useRouter();
  const gate = useRequireOnboarded(lang, common.errors);
  const [messages, setMessages] = useState<Message[]>([]);
  const [draft, setDraft] = useState("");
  const [pending, setPending] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const [suggestions, setSuggestions] = useState<Suggestion[]>([]);
  const [cards, setCards] = useState<Record<number, CardState>>({});
  const runRef = useRef(0);
  const inputId = useId();
  const errors = dict.errors as Record<string, string>;

  function unauthorized(status: number): boolean {
    if (status !== 401) return false;
    router.replace(signInPath(lang));
    return true;
  }

  async function send(event?: FormEvent<HTMLFormElement>) {
    event?.preventDefault();
    const text = draft.trim();
    if (text === "" || pending) return;
    const run = ++runRef.current;
    // The log keeps full replies; only the request is trimmed to the api's caps.
    const shown: Message[] = [...messages, { role: "user" as const, content: text }];
    const history = buildChatHistory(shown);
    setMessages(shown);
    setDraft("");
    setError(null);
    setPending(true);
    const { data, error: failure, response } = await api.POST("/assistant/chat", {
      body: { messages: history },
    });
    if (runRef.current !== run) return;
    setPending(false);
    if (unauthorized(response.status)) return;
    if (!data) {
      setError(errorMessage(response.status, failure?.error, common.errors, errors));
      return;
    }
    setMessages([...shown, { role: "assistant", content: data.reply }]);
    if (data.suggestions.length > 0) {
      setSuggestions((current) => {
        const known = new Set(current.map((s) => s.id));
        return [...current, ...data.suggestions.filter((s) => !known.has(s.id))];
      });
    }
  }

  function onKeyDown(event: KeyboardEvent<HTMLTextAreaElement>) {
    if (event.key === "Enter" && !event.shiftKey && !event.nativeEvent.isComposing) {
      event.preventDefault();
      event.currentTarget.form?.requestSubmit();
    }
  }

  async function confirm(s: Suggestion) {
    setCards((c) => ({ ...c, [s.id]: { status: "busy" } }));
    const { data, error: failure, response } = await api.POST("/matches/decisions", {
      body: {
        suggestion_id: s.id,
        retailer: s.retailer,
        retailer_sku: s.retailer_sku,
        asin: s.asin,
        decision: s.decision,
      },
    });
    if (unauthorized(response.status)) return;
    if (!data) {
      setCards((c) => ({
        ...c,
        [s.id]: {
          status: "error",
          note: errorMessage(response.status, failure?.error, common.errors, errors),
        },
      }));
      return;
    }
    setCards((c) => ({
      ...c,
      [s.id]: { status: "done", note: data.decision === "accept" ? dict.accepted : dict.rejected },
    }));
  }

  function dismiss(s: Suggestion) {
    setCards((c) => ({ ...c, [s.id]: { status: "done", note: dict.dismissed } }));
  }

  function reset() {
    runRef.current += 1;
    setMessages([]);
    setSuggestions([]);
    setCards({});
    setError(null);
    setPending(false);
  }

  if (gate.status === "error") {
    return (
      <p role="alert" className="error">
        {gate.message}
      </p>
    );
  }
  if (gate.status === "loading") {
    return <p role="status">{common.loading}</p>;
  }
  return (
    <>
      <h1>{dict.heading}</h1>
      <p>{dict.intro}</p>
      <p className="hint">{dict.disclaimer}</p>

      <section aria-label={dict.conversation} className="chat">
        <ol className="chat-log" aria-live="polite" aria-relevant="additions" data-testid="chat-log">
          {messages.map((m, i) => (
            <li key={i} className={m.role === "user" ? "chat-user" : "chat-bot"}>
              <span className="chat-who">{m.role === "user" ? dict.you : dict.bot}</span>
              <p className="chat-text">{m.content}</p>
            </li>
          ))}
        </ol>
        {messages.length === 0 ? <p className="hint">{dict.empty}</p> : null}
        <p role="status" data-testid="chat-status">
          {pending ? dict.thinking : ""}
        </p>
        <p role="alert" className="error">
          {error}
        </p>
      </section>

      <form className="stack" onSubmit={send}>
        <label htmlFor={inputId}>{dict.label}</label>
        <textarea
          id={inputId}
          name="message"
          rows={3}
          required
          maxLength={MAX_MESSAGE_CHARS}
          placeholder={dict.placeholder}
          value={draft}
          onChange={(e) => setDraft(e.target.value)}
          onKeyDown={onKeyDown}
        />
        <div className="actions">
          <button type="submit" disabled={pending || draft.trim() === ""}>
            {dict.send}
          </button>
          <button type="button" className="secondary" onClick={reset} disabled={messages.length === 0}>
            {dict.newChat}
          </button>
        </div>
      </form>

      {suggestions.length > 0 ? (
        <section aria-labelledby="suggestions-heading">
          <h2 id="suggestions-heading">{dict.suggestionsHeading}</h2>
          <ul className="suggestions">
            {suggestions.map((s) => {
              const state = cards[s.id] ?? { status: "open" };
              const title = s.title || s.retailer_sku;
              return (
                <li key={s.id} className="suggestion" data-testid="suggestion">
                  <p>
                    <strong>{title}</strong>
                  </p>
                  <p>
                    {interpolate(dict.suggestion, {
                      retailer: retailerName(s.retailer, retailers),
                      sku: s.retailer_sku,
                      asin: s.asin,
                    })}
                  </p>
                  <p>
                    <span className={s.decision === "accept" ? "badge ok" : "badge warn"}>
                      {s.decision === "accept" ? dict.proposeAccept : dict.proposeReject}
                    </span>{" "}
                    {interpolate(dict.reason, { reason: s.reason })}
                  </p>
                  {state.status === "done" ? (
                    <p role="status">{state.note}</p>
                  ) : (
                    <>
                      <div className="actions">
                        <button
                          type="button"
                          onClick={() => void confirm(s)}
                          disabled={state.status === "busy"}
                          aria-label={interpolate(dict.confirmLabel, { title })}
                        >
                          {dict.confirm}
                        </button>
                        <button
                          type="button"
                          className="secondary"
                          onClick={() => dismiss(s)}
                          disabled={state.status === "busy"}
                          aria-label={interpolate(dict.dismissLabel, { title })}
                        >
                          {dict.dismiss}
                        </button>
                      </div>
                      <p role="alert" className="error">
                        {state.status === "error" ? state.note : null}
                      </p>
                    </>
                  )}
                </li>
              );
            })}
          </ul>
        </section>
      ) : null}
    </>
  );
}
