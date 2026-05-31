//! Window-level commands (Phase 14).
//!
//! These commands ask the registry (owned by `app`) to spawn a new top-
//! level window. The handlers themselves carry no editor state; they
//! delegate to closures provided by `app` at registration time. The
//! `tear_off` variant queries the active context for the focused tab's
//! buffer id (and removes it from the local pane tree) before delegating
//! to its spawn closure.

use std::sync::Arc;

use crate::context::Context;
use crate::predicate::ContextPredicate;
use crate::registry::Registry;
use crate::CommandId;

/// Open a fresh empty window.
pub const WINDOW_NEW_WINDOW: CommandId = CommandId("window.new_window");
/// Tear the focused tab off into a new window.
pub const WINDOW_TEAR_OFF_FOCUSED_TAB: CommandId = CommandId("window.tear_off_focused_tab");

/// `app`-supplied callback type for `window.new_window`.
pub type NewWindowHandler =
    Arc<dyn Fn(&serde_json::Value, &mut dyn Context) -> Result<(), crate::Error> + Send + Sync>;

/// `app`-supplied callback type for `window.tear_off_focused_tab`.
pub type TearOffHandler =
    Arc<dyn Fn(&serde_json::Value, &mut dyn Context) -> Result<(), crate::Error> + Send + Sync>;

/// Register Phase-14 window commands. The two closures encapsulate every
/// piece of `app`-owned state the handlers need (registry channel, editor
/// handle); the command crate itself stays decoupled from `app`.
pub fn register_window_commands<NH, TH>(
    reg: &mut Registry,
    new_window_handler: NH,
    tear_off_handler: TH,
) where
    NH: Fn(&serde_json::Value, &mut dyn Context) -> Result<(), crate::Error>
        + Send
        + Sync
        + 'static,
    TH: Fn(&serde_json::Value, &mut dyn Context) -> Result<(), crate::Error>
        + Send
        + Sync
        + 'static,
{
    reg.register(
        WINDOW_NEW_WINDOW,
        ContextPredicate::always(),
        Arc::new(new_window_handler),
    );
    reg.register(
        WINDOW_TEAR_OFF_FOCUSED_TAB,
        ContextPredicate::always(),
        Arc::new(tear_off_handler),
    );
}
