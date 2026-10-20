//! AI assistant chat and per-tenant match decisions (ADR 0013).
//!
//! The assistant runs in a sidecar whose output is untrusted (ADR 0004);
//! these types bound what the api accepts from the browser and the sidecar.

use std::fmt;

/// Longest single chat message, in characters.
pub const MAX_CHAT_TEXT_CHARS: usize = 2_000;
/// Most messages in one chat history (user and assistant turns).
pub const MAX_CHAT_TURNS: usize = 20;
/// Most characters across a whole chat history.
pub const MAX_CHAT_TOTAL_CHARS: usize = 16_000;
/// Longest assistant reply the api passes on, in characters.
pub const MAX_AGENT_REPLY_CHARS: usize = 8_000;
/// Longest reason attached to a match suggestion, in characters.
pub const MAX_SUGGESTION_REASON_CHARS: usize = 300;

/// A reviewer's verdict on a `needs_review` match, scoped to one tenant.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum MatchDecision {
    /// The listing is the Amazon product.
    Accept,
    /// The listing is not the Amazon product.
    Reject,
}

impl MatchDecision {
    /// Stable lowercase name.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Accept => "accept",
            Self::Reject => "reject",
        }
    }

    /// Reads a name produced by [`Self::as_str`].
    #[must_use]
    pub fn parse(s: &str) -> Option<Self> {
        match s {
            "accept" => Some(Self::Accept),
            "reject" => Some(Self::Reject),
            _ => None,
        }
    }
}

/// Who wrote a chat message.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum ChatRole {
    /// The signed-in seller.
    User,
    /// The assistant (an earlier reply echoed back by the browser).
    Assistant,
}

impl ChatRole {
    /// Stable lowercase name.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::User => "user",
            Self::Assistant => "assistant",
        }
    }

    /// Reads a name produced by [`Self::as_str`].
    #[must_use]
    pub fn parse(s: &str) -> Option<Self> {
        match s {
            "user" => Some(Self::User),
            "assistant" => Some(Self::Assistant),
            _ => None,
        }
    }
}

/// Why chat input was rejected. Message text is never echoed.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ChatError {
    /// A message is empty after trimming.
    EmptyMessage,
    /// A message is longer than [`MAX_CHAT_TEXT_CHARS`].
    MessageTooLong,
    /// No messages at all.
    EmptyHistory,
    /// More than [`MAX_CHAT_TURNS`] messages.
    TooManyTurns,
    /// More than [`MAX_CHAT_TOTAL_CHARS`] characters in total.
    HistoryTooLong,
    /// The last message is not from the user.
    LastNotUser,
}

impl fmt::Display for ChatError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::EmptyMessage => f.write_str("chat message is empty"),
            Self::MessageTooLong => write!(
                f,
                "chat message is longer than {MAX_CHAT_TEXT_CHARS} characters"
            ),
            Self::EmptyHistory => f.write_str("chat has no messages"),
            Self::TooManyTurns => write!(f, "chat has more than {MAX_CHAT_TURNS} messages"),
            Self::HistoryTooLong => write!(
                f,
                "chat is longer than {MAX_CHAT_TOTAL_CHARS} characters in total"
            ),
            Self::LastNotUser => f.write_str("last chat message must be from the user"),
        }
    }
}

impl std::error::Error for ChatError {}

/// Removes control characters except `\n` and `\t`.
#[must_use]
pub fn strip_control_chars(raw: &str) -> String {
    raw.chars()
        .filter(|c| !c.is_control() || matches!(c, '\n' | '\t'))
        .collect()
}

/// One chat message: control characters removed, trimmed,
/// 1..=[`MAX_CHAT_TEXT_CHARS`] characters.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct ChatText(String);

impl ChatText {
    /// Cleans and validates a raw message.
    ///
    /// # Errors
    /// [`ChatError::EmptyMessage`] or [`ChatError::MessageTooLong`].
    pub fn parse(raw: &str) -> Result<Self, ChatError> {
        let cleaned = strip_control_chars(raw);
        let text = cleaned.trim();
        if text.is_empty() {
            Err(ChatError::EmptyMessage)
        } else if text.chars().count() > MAX_CHAT_TEXT_CHARS {
            Err(ChatError::MessageTooLong)
        } else {
            Ok(Self(text.to_owned()))
        }
    }

    /// The cleaned text.
    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }

    /// Length in characters.
    #[must_use]
    pub fn chars(&self) -> usize {
        self.0.chars().count()
    }
}

/// One message in a [`ChatHistory`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ChatMessage {
    /// Author.
    pub role: ChatRole,
    /// Text.
    pub text: ChatText,
}

/// A validated chat: 1..=[`MAX_CHAT_TURNS`] messages, at most
/// [`MAX_CHAT_TOTAL_CHARS`] characters, the last one from the user.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ChatHistory(Vec<ChatMessage>);

impl ChatHistory {
    /// Validates raw `(role, text)` pairs in order.
    ///
    /// # Errors
    /// The first [`ChatError`] found.
    pub fn parse<'a, I>(raw: I) -> Result<Self, ChatError>
    where
        I: IntoIterator<Item = (ChatRole, &'a str)>,
    {
        let mut messages = Vec::new();
        let mut total = 0_usize;
        for (role, text) in raw {
            if messages.len() == MAX_CHAT_TURNS {
                return Err(ChatError::TooManyTurns);
            }
            let text = ChatText::parse(text)?;
            total += text.chars();
            if total > MAX_CHAT_TOTAL_CHARS {
                return Err(ChatError::HistoryTooLong);
            }
            messages.push(ChatMessage { role, text });
        }
        match messages.last() {
            None => Err(ChatError::EmptyHistory),
            Some(last) if last.role != ChatRole::User => Err(ChatError::LastNotUser),
            Some(_) => Ok(Self(messages)),
        }
    }

    /// The messages, oldest first.
    #[must_use]
    pub fn messages(&self) -> &[ChatMessage] {
        &self.0
    }
}

/// A non-negative token count.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Default)]
pub struct TokenCount(u64);

impl TokenCount {
    /// Zero tokens.
    pub const ZERO: Self = Self(0);

    /// Wraps a count.
    #[must_use]
    pub const fn new(value: u64) -> Self {
        Self(value)
    }

    /// The count.
    #[must_use]
    pub const fn get(self) -> u64 {
        self.0
    }

    /// Sum, saturating at `u64::MAX`.
    #[must_use]
    pub const fn saturating_add(self, other: Self) -> Self {
        Self(self.0.saturating_add(other.0))
    }

    /// Difference, saturating at zero.
    #[must_use]
    pub const fn saturating_sub(self, other: Self) -> Self {
        Self(self.0.saturating_sub(other.0))
    }
}

/// Input and output tokens used by a chat.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct TokenUsage {
    /// Prompt tokens (including tool results).
    pub input: TokenCount,
    /// Generated tokens.
    pub output: TokenCount,
}

impl TokenUsage {
    /// Input plus output.
    #[must_use]
    pub const fn total(self) -> TokenCount {
        self.input.saturating_add(self.output)
    }
}

/// Why an assistant run ended.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum AgentStop {
    /// The model finished its answer.
    Done,
    /// The turn limit was reached.
    MaxTurns,
    /// The tenant's daily token budget ran out mid-run.
    TokenBudget,
}

impl AgentStop {
    /// Stable snake_case name.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Done => "done",
            Self::MaxTurns => "max_turns",
            Self::TokenBudget => "token_budget",
        }
    }

    /// Reads a name produced by [`Self::as_str`].
    #[must_use]
    pub fn parse(s: &str) -> Option<Self> {
        match s {
            "done" => Some(Self::Done),
            "max_turns" => Some(Self::MaxTurns),
            "token_budget" => Some(Self::TokenBudget),
            _ => None,
        }
    }
}

/// Cleans an assistant reply: control characters removed, trimmed and cut
/// to [`MAX_AGENT_REPLY_CHARS`] characters.
#[must_use]
pub fn clean_agent_reply(raw: &str) -> String {
    strip_control_chars(raw)
        .trim()
        .chars()
        .take(MAX_AGENT_REPLY_CHARS)
        .collect()
}

/// Why a suggestion reason was rejected.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SuggestionReasonError;

impl fmt::Display for SuggestionReasonError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "suggestion reason must be 1-{MAX_SUGGESTION_REASON_CHARS} characters"
        )
    }
}

impl std::error::Error for SuggestionReasonError {}

/// Why the assistant proposes a decision: control characters removed,
/// whitespace collapsed, 1..=[`MAX_SUGGESTION_REASON_CHARS`] characters.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct SuggestionReason(String);

impl SuggestionReason {
    /// Cleans and validates a raw reason.
    ///
    /// # Errors
    /// [`SuggestionReasonError`] if empty or too long.
    pub fn parse(raw: &str) -> Result<Self, SuggestionReasonError> {
        let text = strip_control_chars(raw)
            .split_whitespace()
            .collect::<Vec<_>>()
            .join(" ");
        let n = text.chars().count();
        if n == 0 || n > MAX_SUGGESTION_REASON_CHARS {
            Err(SuggestionReasonError)
        } else {
            Ok(Self(text))
        }
    }

    /// The cleaned reason.
    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn names_round_trip() {
        for d in [MatchDecision::Accept, MatchDecision::Reject] {
            assert_eq!(MatchDecision::parse(d.as_str()), Some(d));
        }
        for r in [ChatRole::User, ChatRole::Assistant] {
            assert_eq!(ChatRole::parse(r.as_str()), Some(r));
        }
        for s in [AgentStop::Done, AgentStop::MaxTurns, AgentStop::TokenBudget] {
            assert_eq!(AgentStop::parse(s.as_str()), Some(s));
        }
        assert_eq!(MatchDecision::parse("Accept"), None);
        assert_eq!(ChatRole::parse("system"), None);
        assert_eq!(AgentStop::parse("error"), None);
    }

    #[test]
    fn chat_text_is_cleaned_and_bounded() {
        assert_eq!(
            ChatText::parse("  hola\u{0}\u{7}\n mundo ")
                .unwrap()
                .as_str(),
            "hola\n mundo"
        );
        assert_eq!(ChatText::parse(" \u{1b} "), Err(ChatError::EmptyMessage));
        let max = "ñ".repeat(MAX_CHAT_TEXT_CHARS);
        assert_eq!(ChatText::parse(&max).unwrap().chars(), MAX_CHAT_TEXT_CHARS);
        assert_eq!(
            ChatText::parse(&format!("{max}a")),
            Err(ChatError::MessageTooLong)
        );
    }

    #[test]
    fn history_rules() {
        let ok = ChatHistory::parse([
            (ChatRole::User, "hola"),
            (ChatRole::Assistant, "¿en qué te ayudo?"),
            (ChatRole::User, "Buen Fin"),
        ])
        .unwrap();
        assert_eq!(ok.messages().len(), 3);
        assert_eq!(ok.messages()[2].text.as_str(), "Buen Fin");

        assert_eq!(ChatHistory::parse([]), Err(ChatError::EmptyHistory));
        assert_eq!(
            ChatHistory::parse([(ChatRole::User, "a"), (ChatRole::Assistant, "b")]),
            Err(ChatError::LastNotUser)
        );
        assert_eq!(
            ChatHistory::parse([(ChatRole::User, "  ")]),
            Err(ChatError::EmptyMessage)
        );
        let many: Vec<_> = (0..=MAX_CHAT_TURNS)
            .map(|_| (ChatRole::User, "x"))
            .collect();
        assert_eq!(ChatHistory::parse(many), Err(ChatError::TooManyTurns));
        let exact: Vec<_> = (0..MAX_CHAT_TURNS).map(|_| (ChatRole::User, "x")).collect();
        assert!(ChatHistory::parse(exact).is_ok());

        let big = "x".repeat(MAX_CHAT_TEXT_CHARS);
        let long: Vec<_> = (0..9).map(|_| (ChatRole::User, big.as_str())).collect();
        assert_eq!(ChatHistory::parse(long), Err(ChatError::HistoryTooLong));
        let fits: Vec<_> = (0..8).map(|_| (ChatRole::User, big.as_str())).collect();
        assert!(ChatHistory::parse(fits).is_ok());
    }

    #[test]
    fn echoed_assistant_replies_share_the_message_cap() {
        // Replies may be up to MAX_AGENT_REPLY_CHARS, but an echoed reply is a
        // chat message like any other: the browser truncates it before resending.
        let reply = "x".repeat(MAX_CHAT_TEXT_CHARS + 1);
        assert_eq!(
            ChatHistory::parse([
                (ChatRole::User, "hola"),
                (ChatRole::Assistant, reply.as_str()),
                (ChatRole::User, "¿y luego?"),
            ]),
            Err(ChatError::MessageTooLong)
        );
        let cut = format!("{}…", "x".repeat(MAX_CHAT_TEXT_CHARS - 1));
        assert!(
            ChatHistory::parse([
                (ChatRole::User, "hola"),
                (ChatRole::Assistant, cut.as_str()),
                (ChatRole::User, "¿y luego?"),
            ])
            .is_ok()
        );
    }

    #[test]
    fn tokens_saturate() {
        let a = TokenCount::new(10);
        assert_eq!(a.saturating_sub(TokenCount::new(20)), TokenCount::ZERO);
        assert_eq!(
            TokenCount::new(u64::MAX).saturating_add(a),
            TokenCount::new(u64::MAX)
        );
        let usage = TokenUsage {
            input: TokenCount::new(7),
            output: TokenCount::new(5),
        };
        assert_eq!(usage.total().get(), 12);
    }

    #[test]
    fn reply_and_reason_are_cleaned() {
        assert_eq!(clean_agent_reply(" a\u{0}b \n"), "ab");
        let long = "x".repeat(MAX_AGENT_REPLY_CHARS + 50);
        assert_eq!(
            clean_agent_reply(&long).chars().count(),
            MAX_AGENT_REPLY_CHARS
        );
        assert_eq!(
            SuggestionReason::parse(" mismo  GTIN\n\u{7}y marca ")
                .unwrap()
                .as_str(),
            "mismo GTIN y marca"
        );
        assert_eq!(SuggestionReason::parse("\t "), Err(SuggestionReasonError));
        assert_eq!(
            SuggestionReason::parse(&"x".repeat(MAX_SUGGESTION_REASON_CHARS + 1)),
            Err(SuggestionReasonError)
        );
    }
}
