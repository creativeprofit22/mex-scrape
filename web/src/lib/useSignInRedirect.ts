import { useRouter } from "next/navigation";
import { useCallback } from "react";

import { useSession } from "@/components/shell/SessionProvider";
import type { Locale } from "@/i18n/config";
import { currentPath, signInPath } from "@/lib/ui";

/**
 * What a page does on a 401: re-read the frame's session (so its links stop
 * offering account and sign-out) and go to sign-in, which brings the user back
 * to this page. Use it for every 401 so the frame never shows a stale session.
 */
export function useSignInRedirect(lang: Locale): () => void {
  const router = useRouter();
  const { refresh } = useSession();
  return useCallback(() => {
    refresh();
    router.replace(signInPath(lang, currentPath()));
  }, [lang, refresh, router]);
}
