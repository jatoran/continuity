//! Command registry: id → handler mapping with predicate-gated dispatch.

use std::borrow::Borrow;
use std::sync::Arc;

use ahash::{AHashMap, AHashSet};
use serde_json::Value;

use crate::{CommandId, Context, ContextPredicate, Error};

/// Argument bundle passed to a command handler. JSON keeps the schema flexible
/// while remaining inspectable in tests and the palette.
pub type Args = Value;

/// A command handler closure.
pub type Handler = Arc<dyn Fn(&Args, &mut dyn Context) -> Result<(), Error> + Send + Sync>;

struct Entry {
    handler: Handler,
    when: ContextPredicate,
}

/// The command registry.
///
/// Tracks every registered command id, its predicate-gated handler, and an
/// opt-in `palette_safe` flag set used by restricted palette modes (e.g. the
/// slash-command palette) to filter out destructive actions.
#[derive(Default)]
pub struct Registry {
    by_id: AHashMap<CommandId, Entry>,
    palette_safe: AHashSet<CommandId>,
    /// Optional one-line palette descriptions per command id. Populated
    /// at registration time via [`Self::set_description`]; surfaced by
    /// the palette / slash-palette as the row's secondary text.
    descriptions: AHashMap<CommandId, &'static str>,
}

impl Registry {
    /// An empty registry.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Register `handler` under `id`, optionally gated by `when`.
    ///
    /// Re-registering the same id silently replaces the existing entry.
    /// The `palette_safe` flag is preserved across re-registration.
    pub fn register(&mut self, id: CommandId, when: ContextPredicate, handler: Handler) {
        self.by_id.insert(id, Entry { handler, when });
    }

    /// Register `handler` under `id` and flag it as safe for restricted
    /// palette modes (slash-command palette, future palette filters).
    ///
    /// Equivalent to calling [`Self::register`] followed by
    /// [`Self::mark_palette_safe`].
    pub(crate) fn register_palette_safe(
        &mut self,
        id: CommandId,
        when: ContextPredicate,
        handler: Handler,
    ) {
        self.register(id, when, handler);
        self.mark_palette_safe(id);
    }

    /// Mark `id` as safe to surface in restricted palette modes.
    ///
    /// "Safe" means non-destructive: insertion, navigation, view toggles.
    /// Destructive commands (`buffer.delete`, `tab.close`, `trash.purge`,
    /// `selection.clear_secondary`, …) must NOT be marked safe.
    ///
    /// Calling this for an unregistered id is allowed — the flag is purely
    /// metadata. If the command is later registered, the flag still applies.
    pub(crate) fn mark_palette_safe(&mut self, id: CommandId) {
        self.palette_safe.insert(id);
    }

    /// `true` when `id` is flagged safe for restricted palette modes.
    #[must_use]
    pub fn is_palette_safe(&self, id: &str) -> bool {
        self.palette_safe.contains(id)
    }

    /// Attach a one-line palette description to `id`. Surfaced as the
    /// secondary-text row in both the command palette and the slash
    /// palette. `&'static str` so descriptions live next to the
    /// registration call site without runtime allocation.
    pub fn set_description(&mut self, id: CommandId, description: &'static str) {
        self.descriptions.insert(id, description);
    }

    /// Lookup the registered description for `id`, if any.
    #[must_use]
    pub fn description(&self, id: &str) -> Option<&'static str> {
        self.descriptions.get(id).copied()
    }

    /// Iterate registered command ids that are flagged palette-safe.
    pub fn palette_safe_ids(&self) -> impl Iterator<Item = CommandId> + '_ {
        self.palette_safe.iter().copied()
    }

    /// Number of registered commands.
    #[must_use]
    pub fn len(&self) -> usize {
        self.by_id.len()
    }

    /// `true` when no commands are registered.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.by_id.is_empty()
    }

    /// Iterate registered command ids.
    pub fn ids(&self) -> impl Iterator<Item = CommandId> + '_ {
        self.by_id.keys().copied()
    }

    /// Dispatch `id` with `args` against `ctx`.
    ///
    /// # Errors
    ///
    /// - [`Error::UnknownCommand`] when `id` is not registered.
    /// - [`Error::NotApplicable`] when the registered `when` predicate
    ///   evaluates false against `ctx`.
    /// - Any error returned by the handler itself.
    pub fn dispatch(&self, id: CommandId, args: &Args, ctx: &mut dyn Context) -> Result<(), Error> {
        self.dispatch_traced(id.as_str(), args, ctx)
    }

    /// Dispatch a dynamically loaded command name with `args` against `ctx`.
    ///
    /// # Errors
    ///
    /// See [`Self::dispatch`].
    pub fn dispatch_name(&self, id: &str, args: &Args, ctx: &mut dyn Context) -> Result<(), Error> {
        self.dispatch_traced(id, args, ctx)
    }

    /// Inner dispatch with timed `event:command_dispatch` emission.
    /// Captures id, outcome, and duration so user-action history is
    /// reconstructable from the TSV.
    fn dispatch_traced(&self, id: &str, args: &Args, ctx: &mut dyn Context) -> Result<(), Error> {
        let trace_started = continuity_trace::is_enabled().then(std::time::Instant::now);
        let result = match self.handler_for_name(id, ctx) {
            Ok(handler) => handler(args, ctx),
            Err(e) => Err(e),
        };
        if let Some(started) = trace_started {
            let dur_us = u64::try_from(started.elapsed().as_micros()).unwrap_or(u64::MAX);
            let outcome = outcome_token(&result);
            let args_kind = match args {
                Value::Null => "null",
                Value::Bool(_) => "bool",
                Value::Number(_) => "number",
                Value::String(_) => "string",
                Value::Array(_) => "array",
                Value::Object(_) => "object",
            };
            continuity_trace::log_event_us(
                "command_dispatch",
                dur_us,
                &format!("id={id} outcome={outcome} args={args_kind}"),
            );
        }
        result
    }

    /// Resolve a registered handler after applying its context predicate.
    ///
    /// This returns a cloned handler so callers can drop the registry borrow
    /// before invoking it with a mutable context.
    ///
    /// # Errors
    ///
    /// - [`Error::UnknownCommand`] when `id` is not registered.
    /// - [`Error::NotApplicable`] when the registered predicate evaluates
    ///   false against `ctx`.
    pub fn handler_for_name(&self, id: &str, ctx: &dyn Context) -> Result<Handler, Error> {
        let entry = self
            .by_id
            .get(id)
            .ok_or_else(|| Error::UnknownCommand(id.to_string()))?;
        if !entry.when.evaluate(ctx) {
            return Err(Error::NotApplicable(id.to_string()));
        }
        Ok(Arc::clone(&entry.handler))
    }
}

/// Map a dispatch result to a stable token surfaced as
/// `outcome=<token>` on `event:command_dispatch`. Stable enough that
/// the analyzer can group failures without diffing strings.
fn outcome_token(result: &Result<(), Error>) -> &'static str {
    match result {
        Ok(()) => "ok",
        Err(Error::UnknownCommand(_)) => "unknown_command",
        Err(Error::NotApplicable(_)) => "not_applicable",
        Err(Error::UnsupportedContext(_)) => "unsupported_context",
        Err(_) => "err",
    }
}

impl Borrow<str> for CommandId {
    fn borrow(&self) -> &str {
        self.as_str()
    }
}

#[cfg(test)]
mod tests {
    use std::collections::HashMap;
    use std::sync::Mutex;

    use super::*;

    struct EmptyCtx;
    impl Context for EmptyCtx {
        fn lookup(&self, _: &str) -> Option<&str> {
            None
        }
    }
    impl crate::ViewContext for EmptyCtx {}
    impl crate::FindContext for EmptyCtx {}

    struct MapCtx(HashMap<&'static str, &'static str>);
    impl Context for MapCtx {
        fn lookup(&self, key: &str) -> Option<&str> {
            self.0.get(key).copied()
        }
    }
    impl crate::ViewContext for MapCtx {}
    impl crate::FindContext for MapCtx {}

    fn map_ctx(pairs: &[(&'static str, &'static str)]) -> MapCtx {
        MapCtx(pairs.iter().copied().collect())
    }

    #[test]
    fn dispatch_calls_registered_handler() {
        let counter = Arc::new(Mutex::new(0u32));
        let c2 = Arc::clone(&counter);
        let mut reg = Registry::new();
        reg.register(
            CommandId("test.tick"),
            ContextPredicate::always(),
            Arc::new(move |_args, _ctx| {
                *c2.lock().unwrap() += 1;
                Ok(())
            }),
        );
        let mut ctx = EmptyCtx;
        reg.dispatch(CommandId("test.tick"), &Value::Null, &mut ctx)
            .unwrap();
        reg.dispatch(CommandId("test.tick"), &Value::Null, &mut ctx)
            .unwrap();
        assert_eq!(*counter.lock().unwrap(), 2);
    }

    #[test]
    fn dispatch_unknown_returns_unknown_command() {
        let reg = Registry::new();
        let mut ctx = EmptyCtx;
        let err = reg
            .dispatch(CommandId("nope"), &Value::Null, &mut ctx)
            .unwrap_err();
        assert!(matches!(err, Error::UnknownCommand(_)));
    }

    #[test]
    fn predicate_blocks_dispatch() {
        let mut reg = Registry::new();
        reg.register(
            CommandId("md.bold"),
            ContextPredicate::parse("language == 'markdown'"),
            Arc::new(|_, _| Ok(())),
        );
        // Without context, predicate is false → not applicable.
        let mut empty = EmptyCtx;
        let err = reg
            .dispatch(CommandId("md.bold"), &Value::Null, &mut empty)
            .unwrap_err();
        assert!(matches!(err, Error::NotApplicable(_)));
        // With matching context, dispatch succeeds.
        let mut ctx = map_ctx(&[("language", "markdown")]);
        reg.dispatch(CommandId("md.bold"), &Value::Null, &mut ctx)
            .unwrap();
    }

    #[test]
    fn palette_safe_flag_round_trip() {
        let mut reg = Registry::new();
        let safe = CommandId("ins.timestamp");
        let unsafe_id = CommandId("buf.delete");
        reg.register_palette_safe(safe, ContextPredicate::always(), Arc::new(|_, _| Ok(())));
        reg.register(
            unsafe_id,
            ContextPredicate::always(),
            Arc::new(|_, _| Ok(())),
        );
        assert!(reg.is_palette_safe(safe.as_str()));
        assert!(!reg.is_palette_safe(unsafe_id.as_str()));
        assert!(!reg.is_palette_safe("never.registered"));
        let mut safe_ids: Vec<&'static str> =
            reg.palette_safe_ids().map(|id| id.as_str()).collect();
        safe_ids.sort_unstable();
        assert_eq!(safe_ids, vec!["ins.timestamp"]);
    }

    #[test]
    fn mark_palette_safe_persists_across_reregister() {
        let mut reg = Registry::new();
        let id = CommandId("ins.uuid");
        reg.register(id, ContextPredicate::always(), Arc::new(|_, _| Ok(())));
        reg.mark_palette_safe(id);
        // Re-register replacing the handler — flag must stick.
        reg.register(id, ContextPredicate::always(), Arc::new(|_, _| Ok(())));
        assert!(reg.is_palette_safe(id.as_str()));
    }

    #[test]
    fn ids_iterates_all_registered() {
        let mut reg = Registry::new();
        reg.register(
            CommandId("a"),
            ContextPredicate::always(),
            Arc::new(|_, _| Ok(())),
        );
        reg.register(
            CommandId("b"),
            ContextPredicate::always(),
            Arc::new(|_, _| Ok(())),
        );
        let mut ids: Vec<&'static str> = reg.ids().map(|i| i.as_str()).collect();
        ids.sort_unstable();
        assert_eq!(ids, vec!["a", "b"]);
    }
}
