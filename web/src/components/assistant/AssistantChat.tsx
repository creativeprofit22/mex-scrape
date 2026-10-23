"use client";

import { useEffect, useRef, useState, type FormEvent } from "react";

import { LoadError } from "@/components/feedback/LoadError";
import type { Locale } from "@/i18n/config";
import type { Dictionary } from "@/i18n/dictionaries";
import { api } from "@/lib/api/client";
import { buildChatHistory, type ChatMessage } from "@/lib/chatHistory";
import { errorMessage } from "@/lib/ui";
import { useRequireOnboarded } from "@/lib/useRequireOnboarded";
import { useSignInRedirect } from "@/lib/useSignInRedirect";

import { ChatComposer } from "./ChatComposer";
import { ChatLog } from "./ChatLog";
import { SuggestionCard, type CardState, type Suggestion } from "./SuggestionCard";

type Message = ChatMessage;

/**
 * Chat with the gg-agent assistant (`POST /assistant/chat`). The reply is
 * rendered as plain text. Match proposals become cards after the reply that
 * made them; only the user's Confirmar click writes a decision
 * (`POST /matches/decisions`), and Descartar changes nothing (ADR 0013).
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
  const toSignIn = useSignInRedirect(lang);
  const gate = useRequireOnboarded(lang, common.errors);
  const [messages, setMessages] = useState<Message[]>([]);
  const [draft, setDraft] = useState("");
  const [pending, setPending] = useState(false);
  const [error, setError] = useState<string | null>(null);
  // Proposals keyed by the index of the reply that made them; each id once.
  const [groups, setGroups] = useState<Record<number, Suggestion[]>>({});
  const [cards, setCards] = useState<Record<number, CardState>>({});
  const runRef = useRef(0);
  const logRef = useRef<HTMLOListElement>(null);
  const statusRef = useRef<HTMLParagraphElement>(null);
  const formRef = useRef<HTMLFormElement>(null);
  const errors = dict.errors as Record<string, string>;

  // Keep the newest entry visible above the pinned composer, no smooth motion.
  useEffect(() => {
    const target = pending ? statusRef.current : (logRef.current?.lastElementChild ?? null);
    const workspace = target?.closest(".workspace");
    const composer = formRef.current;
    if (!target || !workspace || !composer) return;
    const top = workspace.getBoundingClientRect().top;
    const bottom = composer.getBoundingClientRect().top;
    const box = target.getBoundingClientRect();
    let delta = 0;
    if (box.height > bottom - top || box.top < top) delta = box.top - top;
    else if (box.bottom > bottom) delta = box.bottom - bottom;
    if (delta !== 0) workspace.scrollBy({ top: delta, behavior: "auto" });
  }, [messages.length, pending, groups]);

  function unauthorized(status: number): boolean {
    if (status !== 401) return false;
    toSignIn();
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
    const replyIndex = shown.length;
    setMessages([...shown, { role: "assistant", content: data.reply }]);
    if (data.suggestions.length > 0) {
      setGroups((current) => {
        const known = new Set(Object.values(current).flatMap((list) => list.map((s) => s.id)));
        const fresh = data.suggestions.filter((s) => !known.has(s.id));
        return fresh.length > 0 ? { ...current, [replyIndex]: fresh } : current;
      });
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

  // Local only: no request, nothing changes on the server.
  function dismiss(s: Suggestion) {
    setCards((c) => ({ ...c, [s.id]: { status: "done", note: dict.dismissed } }));
  }

  function reset() {
    runRef.current += 1;
    setMessages([]);
    setGroups({});
    setCards({});
    setError(null);
    setPending(false);
  }

  if (gate.status === "error") {
    return <LoadError message={gate.message} retryLabel={common.retry} onRetry={gate.retry} />;
  }
  if (gate.status === "loading") {
    return <p role="status">{common.loading}</p>;
  }
  return (
    <div className="assistant">
      <header className="assistant__intro">
        <h1>{dict.heading}</h1>
        <p>{dict.intro}</p>
        <p className="hint">{dict.disclaimer}</p>
      </header>

      <ChatLog
        messages={messages}
        cardsAfter={(index) =>
          groups[index]?.map((s) => (
            <SuggestionCard
              key={s.id}
              suggestion={s}
              state={cards[s.id] ?? { status: "open" }}
              onConfirm={() => void confirm(s)}
              onDismiss={() => dismiss(s)}
              dict={dict}
              retailers={retailers}
            />
          ))
        }
        pending={pending}
        error={error}
        logRef={logRef}
        statusRef={statusRef}
        dict={dict}
      />

      <ChatComposer
        draft={draft}
        onDraft={setDraft}
        onSend={(event) => void send(event)}
        onReset={reset}
        pending={pending}
        canReset={messages.length > 0}
        formRef={formRef}
        dict={dict}
      />
    </div>
  );
}
