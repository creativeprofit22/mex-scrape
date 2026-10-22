"use client";

import { createContext, useCallback, useContext, useEffect, useMemo, useState } from "react";

import { api, type Schemas } from "@/lib/api/client";

/**
 * What the frame knows about the visitor. `loading` and `unknown` show no
 * session links at all, so the frame never offers sign-in to a signed-in user
 * (or sign-out to a signed-out one). `unknown` keeps the failed reply so a
 * page can explain it.
 */
export type Session =
  | { status: "loading" }
  | { status: "signedIn"; me: Schemas["MeResponse"] }
  | { status: "signedOut" }
  | { status: "unknown"; httpStatus: number; code: string | undefined };

type SessionContextValue = { session: Session; refresh: () => void };

const SessionContext = createContext<SessionContextValue | null>(null);

/**
 * Reads `GET /me` once for the document (it wraps every page from the root
 * layout, so it survives navigation between the public and app frames).
 * 401 is signed out; any other failure is `unknown`. Call `refresh` after
 * anything that changes the session (sign-in, sign-out, retry).
 */
export function SessionProvider({ children }: { children: React.ReactNode }) {
  const [session, setSession] = useState<Session>({ status: "loading" });
  const [attempt, setAttempt] = useState(0);

  useEffect(() => {
    let active = true;
    void api.GET("/me").then(({ data, error, response }) => {
      if (!active) return;
      if (data) setSession({ status: "signedIn", me: data });
      else if (response.status === 401) setSession({ status: "signedOut" });
      else setSession({ status: "unknown", httpStatus: response.status, code: error?.error });
    });
    return () => {
      active = false;
    };
  }, [attempt]);

  const refresh = useCallback(() => {
    setSession({ status: "loading" });
    setAttempt((n) => n + 1);
  }, []);

  const value = useMemo(() => ({ session, refresh }), [session, refresh]);
  return <SessionContext value={value}>{children}</SessionContext>;
}

/** The current session and a way to re-read it. Must sit under `SessionProvider`. */
export function useSession(): SessionContextValue {
  const value = useContext(SessionContext);
  if (value === null) throw new Error("useSession needs a SessionProvider");
  return value;
}
