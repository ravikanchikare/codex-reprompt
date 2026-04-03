//! Ring buffer of recent conversation turns for reprompt context.
//!
//! [`ThreadContextBuffer`] accumulates user messages and agent final-answer
//! text as they flow through ChatWidget event handlers.  Before spawning a
//! refinement call, `recent()` extracts the most recent N turns (bounded by
//! both count and total character budget) so the refinement model can resolve
//! anaphoric references like "fix that too".

use std::collections::VecDeque;

/// The role of a conversation turn in the context buffer.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ContextRole {
    User,
    Assistant,
}

/// A single conversation turn stored in the context buffer.
#[derive(Debug, Clone)]
pub(crate) struct ContextTurn {
    pub role: ContextRole,
    pub text: String,
}

/// Fixed-capacity ring buffer of recent conversation turns.
///
/// Stores up to `max_turns` entries internally, truncating each turn's text
/// at `max_chars_per_turn` on insertion.  `recent()` returns a chronologically
/// ordered subset that fits within the requested count and character budget.
pub(crate) struct ThreadContextBuffer {
    turns: VecDeque<ContextTurn>,
    max_turns: usize,
    max_chars_per_turn: usize,
}

impl ThreadContextBuffer {
    /// Create a new buffer with the given internal capacity.
    pub fn new(max_turns: usize) -> Self {
        Self {
            turns: VecDeque::with_capacity(max_turns),
            max_turns,
            max_chars_per_turn: 1000,
        }
    }

    /// Append a user message to the buffer.
    pub fn push_user(&mut self, text: &str) {
        self.push(ContextRole::User, text);
    }

    /// Append an assistant (agent) message to the buffer.
    pub fn push_assistant(&mut self, text: &str) {
        self.push(ContextRole::Assistant, text);
    }

    /// Clear the buffer (e.g. on thread switch).
    pub fn clear(&mut self) {
        self.turns.clear();
    }

    /// Return the most recent `n` turns that fit within `max_total_chars`,
    /// ordered oldest-first (chronological).
    pub fn recent(&self, n: usize, max_total_chars: usize) -> Vec<ContextTurn> {
        let mut result: Vec<&ContextTurn> = Vec::new();
        let mut total_chars = 0usize;

        // Walk backwards (newest first) collecting turns that fit.
        for turn in self.turns.iter().rev() {
            if result.len() >= n {
                break;
            }
            let turn_len = turn.text.len();
            if total_chars + turn_len > max_total_chars && !result.is_empty() {
                // Adding this turn would exceed the budget and we already have
                // at least one turn — stop here.
                break;
            }
            total_chars += turn_len;
            result.push(turn);
        }

        // Reverse to chronological order and clone.
        result.reverse();
        result.into_iter().cloned().collect()
    }

    fn push(&mut self, role: ContextRole, text: &str) {
        let truncated = if text.len() > self.max_chars_per_turn {
            // Truncate at a char boundary.
            let mut end = self.max_chars_per_turn;
            while !text.is_char_boundary(end) && end > 0 {
                end -= 1;
            }
            format!("{}...", &text[..end])
        } else {
            text.to_string()
        };

        if self.turns.len() >= self.max_turns {
            self.turns.pop_front();
        }
        self.turns.push_back(ContextTurn {
            role,
            text: truncated,
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn push_and_recent_returns_chronological_order() {
        let mut buf = ThreadContextBuffer::new(10);
        buf.push_user("first");
        buf.push_assistant("second");
        buf.push_user("third");

        let turns = buf.recent(10, 10000);
        assert_eq!(turns.len(), 3);
        assert_eq!(turns[0].role, ContextRole::User);
        assert_eq!(turns[0].text, "first");
        assert_eq!(turns[1].role, ContextRole::Assistant);
        assert_eq!(turns[1].text, "second");
        assert_eq!(turns[2].role, ContextRole::User);
        assert_eq!(turns[2].text, "third");
    }

    #[test]
    fn evicts_oldest_when_full() {
        let mut buf = ThreadContextBuffer::new(2);
        buf.push_user("a");
        buf.push_assistant("b");
        buf.push_user("c");

        let turns = buf.recent(10, 10000);
        assert_eq!(turns.len(), 2);
        assert_eq!(turns[0].text, "b");
        assert_eq!(turns[1].text, "c");
    }

    #[test]
    fn recent_respects_count_limit() {
        let mut buf = ThreadContextBuffer::new(10);
        buf.push_user("a");
        buf.push_assistant("b");
        buf.push_user("c");

        let turns = buf.recent(2, 10000);
        assert_eq!(turns.len(), 2);
        // Should be the most recent 2, in chronological order.
        assert_eq!(turns[0].text, "b");
        assert_eq!(turns[1].text, "c");
    }

    #[test]
    fn recent_respects_char_budget() {
        let mut buf = ThreadContextBuffer::new(10);
        buf.push_user("aaaa"); // 4 chars
        buf.push_assistant("bbbb"); // 4 chars
        buf.push_user("cccc"); // 4 chars

        // Budget of 8 chars: should fit the last 2 turns (8 chars total).
        let turns = buf.recent(10, 8);
        assert_eq!(turns.len(), 2);
        assert_eq!(turns[0].text, "bbbb");
        assert_eq!(turns[1].text, "cccc");
    }

    #[test]
    fn recent_allows_single_turn_exceeding_budget() {
        let mut buf = ThreadContextBuffer::new(10);
        buf.push_user("long text here");

        // Budget is smaller than the single turn, but we still return it
        // because we always allow at least one turn.
        let turns = buf.recent(10, 5);
        assert_eq!(turns.len(), 1);
        assert_eq!(turns[0].text, "long text here");
    }

    #[test]
    fn truncates_long_turns_on_insertion() {
        let mut buf = ThreadContextBuffer::new(10);
        let long_text = "x".repeat(2000);
        buf.push_user(&long_text);

        let turns = buf.recent(1, 10000);
        assert_eq!(turns.len(), 1);
        // 1000 chars + "..."
        assert_eq!(turns[0].text.len(), 1003);
        assert!(turns[0].text.ends_with("..."));
    }

    #[test]
    fn clear_empties_buffer() {
        let mut buf = ThreadContextBuffer::new(10);
        buf.push_user("hello");
        buf.push_assistant("world");
        buf.clear();

        let turns = buf.recent(10, 10000);
        assert!(turns.is_empty());
    }

    #[test]
    fn recent_with_zero_count_returns_empty() {
        let mut buf = ThreadContextBuffer::new(10);
        buf.push_user("hello");

        let turns = buf.recent(0, 10000);
        assert!(turns.is_empty());
    }

    #[test]
    fn empty_buffer_returns_empty() {
        let buf = ThreadContextBuffer::new(10);
        let turns = buf.recent(5, 10000);
        assert!(turns.is_empty());
    }
}
