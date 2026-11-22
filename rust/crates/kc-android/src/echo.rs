//! Rust-side clipboard echo suppression.
//!
//! Kotlin is only a stateless `ClipboardManager` adapter, so the decision of
//! "did we just send this text ourselves?" must live in Rust. The registry's
//! `ClipboardPlugin` keeps its own recent-content queue for *incoming*
//! packets, but the trait object cannot be downcast back to it; this small
//! helper mirrors that queue for the *outgoing* / local-clipboard path.

use std::collections::hash_map::DefaultHasher;
use std::collections::VecDeque;
use std::hash::{Hash, Hasher};
use std::sync::Mutex;

/// How many recently sent / received texts to remember.
const CAPACITY: usize = 8;

#[derive(Debug, Default)]
pub struct ClipboardEcho {
    recent: Mutex<VecDeque<u64>>,
}

fn hash_text(text: &str) -> u64 {
    let mut hasher = DefaultHasher::new();
    text.hash(&mut hasher);
    hasher.finish()
}

impl ClipboardEcho {
    /// Record `text` as recently handled by Rust.
    pub fn remember(&self, text: &str) {
        let hash = hash_text(text);
        if let Ok(mut queue) = self.recent.lock() {
            queue.retain(|&h| h != hash);
            queue.push_back(hash);
            while queue.len() > CAPACITY {
                queue.pop_front();
            }
        }
    }

    /// Whether `text` was handled recently and should not be sent / surfaced
    /// again. A poisoned lock means "not recent", i.e. fail open.
    pub fn is_recent(&self, text: &str) -> bool {
        let hash = hash_text(text);
        self.recent
            .lock()
            .map(|queue| queue.contains(&hash))
            .unwrap_or(false)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn remembers_and_evicts() {
        let echo = ClipboardEcho::default();
        assert!(!echo.is_recent("hello"));
        echo.remember("hello");
        assert!(echo.is_recent("hello"));
        assert!(!echo.is_recent("other"));

        for i in 0..CAPACITY + 2 {
            echo.remember(&format!("item-{i}"));
        }
        assert!(!echo.is_recent("hello"));
        assert!(echo.is_recent(&format!("item-{}", CAPACITY + 1)));
    }

    #[test]
    fn re_remembering_moves_entry_to_the_back() {
        let echo = ClipboardEcho::default();
        echo.remember("a");
        echo.remember("b");
        echo.remember("a");
        for i in 0..CAPACITY - 1 {
            echo.remember(&format!("x-{i}"));
        }
        assert!(echo.is_recent("a"));
        assert!(!echo.is_recent("b"));
    }
}
