//! Context predicates.
//!
//! A predicate is a tiny boolean expression over a [`Context`]. The grammar
//! is intentionally small for groundwork; it can grow as the keymap and
//! palette demand more expressiveness.
//!
//! Currently supported atomic forms:
//! - `true` / `false`
//! - `attribute_path`            — truthy if the context flags it as set
//! - `path == 'literal'`         — string equality against a context lookup
//! - `path != 'literal'`         — string inequality
//!
//! Combining: `&&` (and). No precedence-aware parser yet — separate the
//! conjuncts with `&&`.

use crate::Context;

/// A parsed context predicate.
#[derive(Debug, Clone)]
pub struct ContextPredicate {
    conjuncts: Vec<Atom>,
}

#[derive(Debug, Clone)]
enum Atom {
    Const(bool),
    Flag(String),
    Eq(String, String),
    Ne(String, String),
}

impl ContextPredicate {
    /// Parse a predicate.
    ///
    /// An empty string is treated as the always-true predicate.
    #[must_use]
    pub fn parse(s: &str) -> Self {
        let s = s.trim();
        if s.is_empty() {
            return Self {
                conjuncts: vec![Atom::Const(true)],
            };
        }
        let mut conjuncts = Vec::new();
        for part in s.split("&&") {
            conjuncts.push(parse_atom(part.trim()));
        }
        Self { conjuncts }
    }

    /// Evaluate against a context.
    #[must_use]
    pub fn evaluate(&self, ctx: &dyn Context) -> bool {
        self.conjuncts.iter().all(|a| eval_atom(a, ctx))
    }

    /// The constant `true` predicate.
    #[must_use]
    pub fn always() -> Self {
        Self {
            conjuncts: vec![Atom::Const(true)],
        }
    }
}

fn parse_atom(s: &str) -> Atom {
    if s == "true" {
        return Atom::Const(true);
    }
    if s == "false" {
        return Atom::Const(false);
    }
    if let Some((lhs, rhs)) = s.split_once("==") {
        return Atom::Eq(lhs.trim().to_string(), strip_quotes(rhs.trim()).to_string());
    }
    if let Some((lhs, rhs)) = s.split_once("!=") {
        return Atom::Ne(lhs.trim().to_string(), strip_quotes(rhs.trim()).to_string());
    }
    Atom::Flag(s.to_string())
}

fn eval_atom(a: &Atom, ctx: &dyn Context) -> bool {
    match a {
        Atom::Const(b) => *b,
        Atom::Flag(k) => ctx.flag(k),
        Atom::Eq(k, v) => ctx.lookup(k).is_some_and(|got| got == v),
        Atom::Ne(k, v) => ctx.lookup(k).is_none_or(|got| got != v),
    }
}

fn strip_quotes(s: &str) -> &str {
    s.strip_prefix('\'')
        .and_then(|s| s.strip_suffix('\''))
        .or_else(|| s.strip_prefix('"').and_then(|s| s.strip_suffix('"')))
        .unwrap_or(s)
}

#[cfg(test)]
mod tests {
    use std::collections::HashMap;

    use super::*;

    struct MapCtx(HashMap<&'static str, &'static str>);

    impl Context for MapCtx {
        fn lookup(&self, key: &str) -> Option<&str> {
            self.0.get(key).copied()
        }
    }
    impl crate::ViewContext for MapCtx {}
    impl crate::FindContext for MapCtx {}
    impl crate::EditConfigContext for MapCtx {}

    fn ctx(pairs: &[(&'static str, &'static str)]) -> MapCtx {
        MapCtx(pairs.iter().copied().collect())
    }

    #[test]
    fn empty_is_true() {
        let p = ContextPredicate::parse("");
        assert!(p.evaluate(&ctx(&[])));
    }

    #[test]
    fn const_true_and_false() {
        assert!(ContextPredicate::parse("true").evaluate(&ctx(&[])));
        assert!(!ContextPredicate::parse("false").evaluate(&ctx(&[])));
    }

    #[test]
    fn equality_string_literal() {
        let p = ContextPredicate::parse("language == 'markdown'");
        assert!(p.evaluate(&ctx(&[("language", "markdown")])));
        assert!(!p.evaluate(&ctx(&[("language", "rust")])));
    }

    #[test]
    fn inequality() {
        let p = ContextPredicate::parse("language != 'markdown'");
        assert!(!p.evaluate(&ctx(&[("language", "markdown")])));
        assert!(p.evaluate(&ctx(&[("language", "rust")])));
        assert!(p.evaluate(&ctx(&[]))); // missing key counts as not-equal.
    }

    #[test]
    fn bare_flag() {
        let p = ContextPredicate::parse("editor.focused");
        assert!(p.evaluate(&ctx(&[("editor.focused", "true")])));
        assert!(!p.evaluate(&ctx(&[])));
    }

    #[test]
    fn conjunction() {
        let p = ContextPredicate::parse("editor.focused && language == 'markdown'");
        assert!(p.evaluate(&ctx(&[
            ("editor.focused", "true"),
            ("language", "markdown")
        ])));
        assert!(!p.evaluate(&ctx(&[("editor.focused", "true")])));
        assert!(!p.evaluate(&ctx(&[("language", "markdown")])));
    }

    #[test]
    fn double_quotes_also_supported() {
        let p = ContextPredicate::parse(r#"language == "markdown""#);
        assert!(p.evaluate(&ctx(&[("language", "markdown")])));
    }
}
