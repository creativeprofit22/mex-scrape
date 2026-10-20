/**
 * Scripted fake model for local runs and the Playwright e2e. Only loaded when
 * `AGENT_PROVIDER=palsu` and `AGENT_ALLOW_FAKE_PROVIDER=true` outside
 * production (see `config.ts`). Deterministic: it reads the conversation and
 * the tool results, never the network.
 */
import { palsuText, palsuToolCall, registerPalsuProvider, type Message } from "@kenkaiiii/gg-ai";

const ASIN = /\bB0[A-Z0-9]{8}\b/;
const TIE_BREAK = /desempat|tie-?break|needs.?review|revisar|resolver/i;

interface ToolOutcome {
  name: string;
  content: string;
  isError: boolean;
}

function lastUserText(messages: Message[]): { text: string; index: number } {
  for (let i = messages.length - 1; i >= 0; i--) {
    const m = messages[i];
    if (m?.role === "user") {
      const text =
        typeof m.content === "string"
          ? m.content
          : m.content.flatMap((p) => (p.type === "text" ? [p.text] : [])).join(" ");
      return { text, index: i };
    }
  }
  return { text: "", index: -1 };
}

/** Tool results after the last user message, with the tool names. */
function outcomesSince(messages: Message[], index: number): ToolOutcome[] {
  const names = new Map<string, string>();
  const out: ToolOutcome[] = [];
  for (const m of messages.slice(index + 1)) {
    if (m.role === "assistant" && Array.isArray(m.content)) {
      for (const part of m.content) if (part.type === "tool_call") names.set(part.id, part.name);
    } else if (m.role === "tool") {
      for (const r of m.content) {
        const content =
          typeof r.content === "string"
            ? r.content
            : r.content.flatMap((p) => (p.type === "text" ? [p.text] : [])).join("");
        out.push({ name: names.get(r.toolCallId) ?? "", content, isError: r.isError === true });
      }
    }
  }
  return out;
}

function pesos(centavos: number): string {
  const fixed = (centavos / 100).toFixed(2);
  const [whole = "0", frac = "00"] = fixed.split(".");
  return `$${whole.replace(/\B(?=(\d{3})+(?!\d))/g, ",")}.${frac}`;
}

function parse(content: string): unknown {
  try {
    return JSON.parse(content);
  } catch {
    return undefined;
  }
}

/** Lowest 365-day in-stock low across listings, if any. */
function yearLow(history: unknown): { centavos: number; retailer: string } | undefined {
  const listings = (history as { stats?: { listings?: unknown[] } })?.stats?.listings;
  if (!Array.isArray(listings)) return undefined;
  let best: { centavos: number; retailer: string } | undefined;
  for (const l of listings as { retailer?: string; windows?: { days?: number; low_centavos?: number | null }[] }[]) {
    const w = l.windows?.find((x) => x.days === 365);
    if (typeof w?.low_centavos === "number" && (!best || w.low_centavos < best.centavos)) {
      best = { centavos: w.low_centavos, retailer: l.retailer ?? "" };
    }
  }
  return best;
}

type Review = { retailer: string; retailer_sku: string; asin: string; title: string };

function firstReview(offers: unknown): Review | undefined {
  const list = (offers as { offers?: unknown[] })?.offers;
  if (!Array.isArray(list)) return undefined;
  for (const o of list as { offer?: { retailer?: string; retailer_sku?: string; title?: string; amazon?: { asin?: string; match_status?: string } | null } }[]) {
    const offer = o.offer;
    if (offer?.amazon?.match_status === "needs_review" && offer.retailer && offer.retailer_sku && offer.amazon.asin) {
      return { retailer: offer.retailer, retailer_sku: offer.retailer_sku, asin: offer.amazon.asin, title: offer.title ?? "" };
    }
  }
  return undefined;
}

/** Decides the next scripted message from the conversation so far. */
export function scriptedResponse(messages: Message[]) {
  const { text, index } = lastUserText(messages);
  const outcomes = outcomesSince(messages, index);
  const last = outcomes.at(-1);

  if (!last) {
    if (TIE_BREAK.test(text)) return palsuToolCall("search_offers", { match_status: "needs_review", limit: 5 });
    const asin = ASIN.exec(text)?.[0];
    if (asin) return palsuToolCall("get_price_history", { asin, days: 365 });
    return palsuText("Puedo ayudarte con precios, temporadas como Hot Sale o Buen Fin, ROI y coincidencias por revisar.");
  }
  if (last.isError) return palsuText("No pude consultar los datos en este momento. Intenta de nuevo más tarde.");

  switch (last.name) {
    case "get_price_history": {
      const data = parse(last.content);
      const asin = (data as { stats?: { asin?: string } })?.stats?.asin ?? "";
      const low = yearLow(data);
      if (!low) {
        return palsuText(`No tengo precios guardados de ${asin}. Haz una nueva búsqueda para obtenerlos.`);
      }
      return palsuText(
        `Para preparar el Buen Fin: el precio más bajo de ${asin} en los últimos 365 días fue ${pesos(low.centavos)} (${low.retailer}). ` +
          "Si lo ves por debajo de ese precio, es buen momento para comprar.",
      );
    }
    case "search_offers": {
      const review = firstReview(parse(last.content));
      if (!review) return palsuText("No encontré coincidencias por revisar en tus búsquedas recientes.");
      return palsuToolCall("resolve_match", {
        retailer: review.retailer,
        retailer_sku: review.retailer_sku,
        asin: review.asin,
        decision: "accept",
        reason: "El título y la marca coinciden con el producto de Amazon.",
      });
    }
    case "resolve_match":
      return palsuText("Propuse aceptar esa coincidencia. Confírmala o descártala abajo; no cambia nada hasta que la confirmes.");
    default:
      return palsuText("Listo.");
  }
}

/** Registers the scripted provider under `palsu`. */
export function registerScriptedProvider() {
  return registerPalsuProvider({ defaultResponse: (messages) => scriptedResponse(messages) });
}
