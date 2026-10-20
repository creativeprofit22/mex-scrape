/** Mirrors `MAX_CHAT_TEXT_CHARS` in crates/domain/src/assistant.rs. */
export const MAX_MESSAGE_CHARS = 2000;
/** Mirrors `MAX_CHAT_TURNS`: the oldest turns are dropped beyond this. */
export const MAX_TURNS = 20;
/** Mirrors `MAX_CHAT_TOTAL_CHARS`: the oldest turns are dropped beyond this. */
export const MAX_HISTORY_CHARS = 16000;

export type ChatMessage = { role: "user" | "assistant"; content: string };

const ELLIPSIS = "…";

/** Length in code points, as the api counts (Rust `chars()`). */
function charCount(text: string): number {
  return Array.from(text).length;
}

/** Cuts `text` to `MAX_MESSAGE_CHARS` code points, ending in "…" when cut. */
function truncate(text: string): string {
  const chars = Array.from(text);
  if (chars.length <= MAX_MESSAGE_CHARS) return text;
  return chars.slice(0, MAX_MESSAGE_CHARS - 1).join("").trimEnd() + ELLIPSIS;
}

/**
 * Builds the history to send to `POST /assistant/chat` from the displayed
 * conversation (which keeps full replies). Assistant replies can be up to
 * 8000 characters but the api caps each message, the message count and the
 * total, so long replies are truncated and the oldest turns dropped. The
 * result starts with a user message and ends with the newest one.
 */
export function buildChatHistory(messages: readonly ChatMessage[]): ChatMessage[] {
  const history = messages
    .map((m) => (m.role === "assistant" ? { role: m.role, content: truncate(m.content) } : m))
    .filter((m) => m.content.trim() !== "");
  let total = history.reduce((sum, m) => sum + charCount(m.content), 0);
  while (history.length > 1 && (history.length > MAX_TURNS || total > MAX_HISTORY_CHARS)) {
    total -= charCount(history.shift()!.content);
  }
  // The api wants the turns to start with the user.
  while (history.length > 1 && history[0].role !== "user") history.shift();
  return history;
}
