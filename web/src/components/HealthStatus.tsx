"use client";

import { useEffect, useState } from "react";

import { api } from "@/lib/api/client";

type Status = "checking" | "online" | "offline";

const LABELS: Record<Status, string> = {
  checking: "Comprobando API…",
  online: "API en línea",
  offline: "API no disponible",
};

/** Shows whether the Rust API answers GET /health. */
export function HealthStatus() {
  const [status, setStatus] = useState<Status>("checking");

  useEffect(() => {
    let active = true;
    api
      .GET("/health")
      .then(({ data }) => {
        if (active) setStatus(data?.status === "ok" ? "online" : "offline");
      })
      .catch(() => {
        if (active) setStatus("offline");
      });
    return () => {
      active = false;
    };
  }, []);

  return (
    <p role="status" aria-live="polite" data-status={status}>
      {LABELS[status]}
    </p>
  );
}
