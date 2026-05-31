//! Phase 17.8 §B4 + §B9 — agent-friendliness lint rules.
//!
//! Both rules live here so [`crate::conventions`] stays under the 600-line
//! file cap. They're invoked from `check_rust_file` like every other
//! per-line check, and surface their violations on the same channel
//! ([`crate::conventions::Violation`]).
//!
//! - **B4** rejects `pub use ...` outside `lib.rs` / `main.rs` so the
//!   crate's public surface lives in one place.
//! - **B9** rejects `use ... as Alias;` so the canonical name remains
//!   greppable; legitimate collisions opt out with an inline
//!   `// alias: <reason>` comment.

use crate::conventions::Violation;
use crate::scan::find_word;

/// §B4: `pub use ...` is allowed only in `lib.rs`. Deeper re-exports
/// hide the canonical definition site and force agents to follow
/// re-export chains; the lib-level surface stays the one place to look.
pub(crate) fn check_pub_use_only_in_lib_rs(
    code: &str,
    raw: &str,
    path: &str,
    line: usize,
    out: &mut Vec<Violation>,
) {
    let trimmed = code.trim_start();
    if !trimmed.starts_with("pub use ") {
        return;
    }
    if path.ends_with("/lib.rs") || path.ends_with("/main.rs") {
        return;
    }
    if raw.contains("// alias:") {
        return;
    }
    out.push(Violation::new(
        "conventions:pub-use-only-in-lib-rs",
        path,
        Some(line),
        "`pub use` is allowed only in lib.rs — move the re-export to the crate root \
or change the import to a direct path",
    ));
}

/// §B9: `use ... as Alias;` makes `Alias::foo` ungreppable. Disallowed
/// outside test code; legitimate collisions opt out with an inline
/// `// alias: <reason>` comment.
pub(crate) fn check_no_use_aliasing(
    code: &str,
    raw: &str,
    path: &str,
    line: usize,
    out: &mut Vec<Violation>,
) {
    let trimmed = code.trim_start();
    if !(trimmed.starts_with("use ") || trimmed.starts_with("pub use ")) {
        return;
    }
    if find_word(trimmed, "as").map(|idx| idx > 0).unwrap_or(false) {
        if raw.contains("// alias:") {
            return;
        }
        out.push(Violation::new(
            "conventions:no-use-aliasing",
            path,
            Some(line),
            "`use ... as Alias;` is forbidden — the alias hides the canonical name. \
If two imports genuinely collide, qualify at the call site or add `// alias: <reason>` to opt out",
        ));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pub_use_only_in_lib_rs_flags_non_lib_rs() {
        let mut v = Vec::new();
        check_pub_use_only_in_lib_rs(
            "pub use crate::foo::Bar;",
            "pub use crate::foo::Bar;",
            "crates/x/src/inner.rs",
            1,
            &mut v,
        );
        assert_eq!(v.len(), 1);
        v.clear();
        check_pub_use_only_in_lib_rs(
            "pub use crate::foo::Bar;",
            "pub use crate::foo::Bar;",
            "crates/x/src/lib.rs",
            1,
            &mut v,
        );
        assert!(v.is_empty());
        v.clear();
        check_pub_use_only_in_lib_rs(
            "pub use foo::Bar;",
            "pub use foo::Bar; // alias: external re-export held here for the legacy path",
            "crates/x/src/inner.rs",
            1,
            &mut v,
        );
        assert!(v.is_empty());
        v.clear();
        check_pub_use_only_in_lib_rs(
            "use crate::foo::Bar;",
            "use crate::foo::Bar;",
            "crates/x/src/inner.rs",
            1,
            &mut v,
        );
        assert!(v.is_empty());
    }

    #[test]
    fn no_use_aliasing_flags_aliased_imports() {
        let mut v = Vec::new();
        check_no_use_aliasing(
            "use foo::Bar as Baz;",
            "use foo::Bar as Baz;",
            "x.rs",
            1,
            &mut v,
        );
        assert_eq!(v.len(), 1);
        v.clear();
        check_no_use_aliasing(
            "use foo::Error as CommandError;",
            "use foo::Error as CommandError; // alias: collides with crate::Error",
            "x.rs",
            1,
            &mut v,
        );
        assert!(v.is_empty());
        v.clear();
        check_no_use_aliasing(
            "let x = 1; // as documentation",
            "let x = 1; // as documentation",
            "x.rs",
            1,
            &mut v,
        );
        assert!(v.is_empty());
        v.clear();
        check_no_use_aliasing("use foo::Bar;", "use foo::Bar;", "x.rs", 1, &mut v);
        assert!(v.is_empty());
    }
}
