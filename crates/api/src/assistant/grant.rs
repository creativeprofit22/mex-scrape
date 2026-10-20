//! Short-lived tool grants (ADR 0013).
//!
//! For each chat the api signs `{v, user_id, tenant, chat_id, exp_ms}`
//! with HMAC-SHA256 under `AGENT_GRANT_KEY`. The sidecar sends the grant
//! back on every tool call; the internal routes take the user, tenant and
//! chat only from a valid, unexpired grant, never from tool arguments. The
//! sidecar never sees the session cookie.
//!
//! Wire form: `base64url(json) "." base64url(tag)` (no padding).

use app::ChatId;
use base64::Engine as _;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use domain::{TenantId, UnixMillis, UserId};
use hmac::{Hmac, KeyInit as _, Mac as _};
use http_guard::Secret;
use serde::{Deserialize, Serialize};
use sha2::Sha256;

/// Shortest accepted `AGENT_GRANT_KEY`, in bytes.
pub const MIN_GRANT_KEY_BYTES: usize = 32;

/// How long a grant stays valid.
pub const GRANT_TTL_MS: i64 = 120_000;

/// Longest grant accepted (anything longer is refused unread).
const MAX_GRANT_CHARS: usize = 1_024;

/// Version of the signed payload.
const GRANT_VERSION: u8 = 1;

type HmacSha256 = Hmac<Sha256>;

/// What a grant authorizes: tool calls for one user's chat.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Grant {
    /// The signed-in user who started the chat.
    pub user: UserId,
    /// Their tenant.
    pub tenant: TenantId,
    /// The chat.
    pub chat_id: ChatId,
    /// Expiry (epoch milliseconds).
    pub expires_at: UnixMillis,
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Wire {
    v: u8,
    user_id: i64,
    tenant: String,
    chat_id: String,
    exp_ms: i64,
}

/// The HMAC key. No `Debug`, `Display` or `Serialize`.
pub struct GrantKey(Secret);

impl GrantKey {
    /// `None` when the key is shorter than [`MIN_GRANT_KEY_BYTES`].
    #[must_use]
    pub fn new(key: Secret) -> Option<Self> {
        (key.expose_secret().len() >= MIN_GRANT_KEY_BYTES).then_some(Self(key))
    }

    /// The key, for redaction.
    pub(crate) fn secret(&self) -> &Secret {
        &self.0
    }

    fn mac(&self, payload: &[u8]) -> Option<HmacSha256> {
        // HMAC takes keys of any length, so this never fails in practice.
        let mut mac = HmacSha256::new_from_slice(self.0.expose_secret().as_bytes()).ok()?;
        mac.update(payload);
        Some(mac)
    }

    /// A grant for `user`'s chat valid until `now` + [`GRANT_TTL_MS`].
    #[must_use]
    pub fn sign(
        &self,
        user: UserId,
        tenant: &TenantId,
        chat_id: &ChatId,
        now: UnixMillis,
    ) -> String {
        let wire = Wire {
            v: GRANT_VERSION,
            user_id: user.get(),
            tenant: tenant.as_str().to_owned(),
            chat_id: chat_id.as_str().to_owned(),
            exp_ms: now.0.saturating_add(GRANT_TTL_MS),
        };
        // Serializing plain integers and strings cannot fail.
        let payload = serde_json::to_vec(&wire).unwrap_or_default();
        let tag = self
            .mac(&payload)
            .map(|m| m.finalize().into_bytes().to_vec())
            .unwrap_or_default();
        format!(
            "{}.{}",
            URL_SAFE_NO_PAD.encode(&payload),
            URL_SAFE_NO_PAD.encode(tag)
        )
    }

    /// The grant in `token` if its tag is valid (constant-time compare)
    /// and it has not expired at `now`; `None` otherwise.
    #[must_use]
    pub fn verify(&self, token: &str, now: UnixMillis) -> Option<Grant> {
        if token.len() > MAX_GRANT_CHARS {
            return None;
        }
        let (payload, tag) = token.split_once('.')?;
        let payload = URL_SAFE_NO_PAD.decode(payload).ok()?;
        let tag = URL_SAFE_NO_PAD.decode(tag).ok()?;
        self.mac(&payload)?.verify_slice(&tag).ok()?;
        let wire: Wire = serde_json::from_slice(&payload).ok()?;
        if wire.v != GRANT_VERSION || wire.exp_ms <= now.0 {
            return None;
        }
        // A grant never outlives its TTL, even if the clock was set back.
        if wire.exp_ms > now.0.saturating_add(GRANT_TTL_MS) {
            return None;
        }
        Some(Grant {
            user: UserId::new(wire.user_id)?,
            tenant: TenantId::new(&wire.tenant).ok()?,
            chat_id: ChatId::parse(&wire.chat_id)?,
            expires_at: UnixMillis(wire.exp_ms),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn key(raw: &str) -> GrantKey {
        GrantKey::new(Secret::new(raw.to_owned()).unwrap()).unwrap()
    }

    fn parts() -> (UserId, TenantId, ChatId) {
        let user = UserId::new(7).unwrap();
        (
            user,
            TenantId::for_user(user),
            ChatId::parse("chat-0001").unwrap(),
        )
    }

    #[test]
    fn short_keys_are_refused() {
        assert!(GrantKey::new(Secret::new("x".repeat(31)).unwrap()).is_none());
        assert!(GrantKey::new(Secret::new("x".repeat(32)).unwrap()).is_some());
    }

    #[test]
    fn round_trips_until_expiry() {
        let k = key(&"k".repeat(40));
        let (user, tenant, chat) = parts();
        let token = k.sign(user, &tenant, &chat, UnixMillis(1_000));
        let grant = k.verify(&token, UnixMillis(1_000)).unwrap();
        assert_eq!(grant.user, user);
        assert_eq!(grant.tenant, tenant);
        assert_eq!(grant.chat_id, chat);
        assert_eq!(grant.expires_at, UnixMillis(1_000 + GRANT_TTL_MS));
        assert!(
            k.verify(&token, UnixMillis(1_000 + GRANT_TTL_MS - 1))
                .is_some()
        );
        assert!(k.verify(&token, UnixMillis(1_000 + GRANT_TTL_MS)).is_none());
        // Clock set back past the TTL window.
        assert!(k.verify(&token, UnixMillis(-200_000)).is_none());
    }

    #[test]
    fn other_keys_and_tampering_are_refused() {
        let k = key(&"k".repeat(40));
        let (user, tenant, chat) = parts();
        let token = k.sign(user, &tenant, &chat, UnixMillis(0));
        assert!(key(&"j".repeat(40)).verify(&token, UnixMillis(0)).is_none());

        let (payload, tag) = token.split_once('.').unwrap();
        let forged = serde_json::to_vec(&Wire {
            v: 1,
            user_id: 8,
            tenant: "user-8".to_owned(),
            chat_id: "chat-0001".to_owned(),
            exp_ms: GRANT_TTL_MS,
        })
        .unwrap();
        let forged = format!("{}.{tag}", URL_SAFE_NO_PAD.encode(forged));
        assert!(k.verify(&forged, UnixMillis(0)).is_none());
        assert!(k.verify(payload, UnixMillis(0)).is_none());
        assert!(k.verify("", UnixMillis(0)).is_none());
        assert!(k.verify(&"a".repeat(2_000), UnixMillis(0)).is_none());
        assert!(k.verify(&format!("{payload}.!!"), UnixMillis(0)).is_none());
    }
}
