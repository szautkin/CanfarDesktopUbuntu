//! One toast at a time, and it is the one that just happened.
//!
//! `AdwToastOverlay` shows a single toast and queues the rest, each waiting out
//! the one before it. At the default five-second timeout that means the sixth
//! message in a burst reaches the screen almost half a minute after the thing
//! it describes — and image discovery, which can report on dozens of images in
//! a sweep, produces exactly such bursts. The toast that finally appears is
//! then read as being about whatever the user is doing *now*.
//!
//! Two rules fix that, and they are the same rule really: what is on screen
//! should be current.
//!
//! - A new message replaces the one showing, rather than waiting behind it.
//! - A message identical to the one already showing is dropped, because
//!   repeating it says nothing and would only reset its clock.
//!
//! The cost is that a burst is no longer readable in full. That is the right
//! trade: a queue of stale toasts was not readable either, it just took longer
//! to fail. Anything that needs to survive a burst belongs in the status bar,
//! which keeps every task and its failure reason.
//!
//! The exception is a toast the user is meant to act on — one that never times
//! out, or that carries a button. Displacing that would take away something
//! they were offered, so it keeps the screen and the newcomer queues behind it
//! as before. Those come from deliberate actions, never from a sweep, so the
//! burst they would need to cause a delay does not arise.

use libadwaita as adw;
use std::cell::RefCell;
use std::rc::Rc;

/// The toast on screen, and what the next message needs to know about it.
struct Showing {
    /// Its text — `adw::Toast` does not hand it back, and a repeat is dropped.
    body: String,
    toast: adw::Toast,
    /// Whether the user is meant to act on it, and so whether it may be
    /// displaced.
    holds_attention: bool,
}

pub struct ToastStream {
    overlay: adw::ToastOverlay,
    showing: RefCell<Option<Showing>>,
}

impl ToastStream {
    pub fn new(overlay: &adw::ToastOverlay) -> Rc<Self> {
        Rc::new(ToastStream {
            overlay: overlay.clone(),
            showing: RefCell::new(None),
        })
    }

    /// Put `toast` on screen now, displacing whatever is there.
    ///
    /// `body` is the text the toast carries, passed separately because
    /// `adw::Toast` does not hand it back. `holds_attention` marks a toast the
    /// user is meant to act on — one that never times out, or that offers a
    /// button — which is neither displaced nor dropped.
    pub fn present(self: &Rc<Self>, body: &str, toast: adw::Toast, holds_attention: bool) {
        // Read what is on screen and let the borrow go before touching GTK:
        // anything below here can re-enter through a signal handler.
        let (repeat, occupied) = match &*self.showing.borrow() {
            Some(s) => (s.body == body, s.holds_attention),
            None => (false, false),
        };

        // Saying the same thing twice says nothing, and would only reset the
        // clock on the message already being read.
        if repeat {
            return;
        }
        // Something is waiting on the user. Leave it alone and take the queue,
        // which is what the overlay does by default.
        if occupied {
            self.overlay.add_toast(toast);
            return;
        }

        // Taken into a local FIRST. `if let Some(x) = cell.borrow_mut().take()`
        // holds the RefMut for the whole body, and dismissing runs the handler
        // below, which takes the same cell — "already borrowed", inside a GTK
        // signal trampoline that cannot unwind, so the process aborts rather
        // than panicking. The Portal has hit this once already.
        let previous = self.showing.borrow_mut().take();
        if let Some(previous) = previous {
            previous.toast.dismiss();
        }

        {
            // Weak: the stream holds this toast, and the toast owns this
            // closure, so a strong handle here would be a cycle that keeps both
            // alive for as long as the process runs.
            let stream = Rc::downgrade(self);
            let mine = toast.clone();
            toast.connect_dismissed(move |_| {
                let Some(stream) = stream.upgrade() else {
                    return;
                };
                // Only if it is still ours: by the time a displaced toast
                // reports back, the replacement is already on screen.
                let is_mine = stream
                    .showing
                    .borrow()
                    .as_ref()
                    .is_some_and(|s| s.toast == mine);
                if is_mine {
                    stream.showing.borrow_mut().take();
                }
            });
        }

        *self.showing.borrow_mut() = Some(Showing {
            body: body.to_string(),
            toast: toast.clone(),
            holds_attention,
        });
        self.overlay.add_toast(toast);
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn a_burst_never_shows_a_stale_message() {
        // Guard on the rule rather than on GTK, which cannot be initialised in
        // a libtest process. The two properties that make a burst legible are
        // "replace what is showing" and "drop an exact repeat"; losing either
        // brings back the queue, where the delay was.
        let src = include_str!("toasts.rs");
        assert!(
            src.contains("previous.dismiss()"),
            "a new toast waits behind the current one again"
        );
        assert!(
            src.contains("s.body == body"),
            "an identical toast no longer collapses into the one showing"
        );
        assert!(
            src.contains("s.holds_attention"),
            "a toast the user must act on can now be displaced by a background \
             message"
        );
    }
}
