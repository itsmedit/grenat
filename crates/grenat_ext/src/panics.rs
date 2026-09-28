//! Panics inside an entry point: caught — they never unwind into Grenat —
//! and reported through the error Grenat raises, with where they happened,
//! rather than printed. Panics elsewhere (a facet's own threads) go to the
//! hook that was there before.

use std::cell::{Cell, RefCell};
use std::panic::{AssertUnwindSafe, catch_unwind};
use std::sync::Once;

thread_local! {
    /// Whether this thread runs an entry point's body.
    static CATCHING: Cell<bool> = const { Cell::new(false) };
    /// Where the last panic caught on this thread happened.
    static LOCATION: RefCell<Option<String>> = const { RefCell::new(None) };
}

/// Runs `body`; a panic becomes its message (and location).
pub(crate) fn catch<T>(body: impl FnOnce() -> T) -> Result<T, String> {
    quiet_hook();
    let was = CATCHING.with(|c| c.replace(true));
    let result = catch_unwind(AssertUnwindSafe(body));
    CATCHING.with(|c| c.set(was));
    result.map_err(|payload| {
        let message = match (payload.downcast_ref::<&str>(), payload.downcast_ref::<String>()) {
            (Some(text), _) => (*text).to_string(),
            (_, Some(text)) => text.clone(),
            _ => "a panic without a message".into(),
        };
        match LOCATION.with(|l| l.borrow_mut().take()) {
            Some(at) => format!("{message} (at {at})"),
            None => message,
        }
    })
}

/// Installs, once, a hook that keeps the location of the panics caught
/// here, and hands the others to the previous hook.
fn quiet_hook() {
    static INSTALLED: Once = Once::new();
    INSTALLED.call_once(|| {
        let previous = std::panic::take_hook();
        std::panic::set_hook(Box::new(move |info| {
            if CATCHING.with(Cell::get) {
                let at = info.location().map(|l| format!("{}:{}", l.file(), l.line()));
                LOCATION.with(|l| *l.borrow_mut() = at);
            } else {
                previous(info);
            }
        }));
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_panic_becomes_its_message_and_location() {
        assert_eq!(catch(|| 42), Ok(42));
        let e = catch(|| panic!("boom {}", 42)).unwrap_err();
        assert!(e.starts_with("boom 42 (at crates/grenat_ext/src/panics.rs:"), "{e}");
        let e = catch(|| std::panic::panic_any(7)).unwrap_err();
        assert!(e.starts_with("a panic without a message (at "), "{e}");
        // outside, as before
        assert!(!CATCHING.with(Cell::get));
    }
}
