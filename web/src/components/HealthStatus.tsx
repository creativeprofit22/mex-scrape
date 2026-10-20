"use client";

import { useEffect, useState } from "react";

import { api } from "@/lib/api/client";

type Status = "checking" | "online" | "offline";

/** Shows whether the Rust API answers GET /health. */
export function HealthStatus({ labels }: { labels: Record<Status, string> }) {
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
      {labels[status]}
    </p>
  );
}
