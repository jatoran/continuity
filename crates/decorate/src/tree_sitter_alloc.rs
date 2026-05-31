//! Exact allocation counter for tree-sitter's C heap.
//!
//! tree-sitter does not expose the in-memory size of a parsed `Tree`; the
//! `descendant_count * 64` proxy in [`crate::tree_cache`] is a documented
//! 1.5–3× lower bound. To attribute per-buffer memory honestly we route
//! tree-sitter's own allocator through counting wrappers (via
//! [`tree_sitter::set_allocator`]) and expose the exact live byte total as
//! [`tree_sitter_heap_bytes`].
//!
//! # Safety contract
//!
//! [`install`] MUST run before tree-sitter performs its first allocation in
//! the process — i.e. before the first `tree_sitter::Parser` is created. It
//! swaps the global allocator function pointers; a block allocated by the
//! previous allocator but freed by ours (or vice versa) would corrupt the
//! heap, because our wrappers prefix every block with a size header. The
//! sole `tree_sitter::Parser::new()` call site is
//! [`crate::parser::MarkdownParser::new`], which calls [`install`] first, so
//! the contract holds on every launch path (app, tests, e2e). The call is
//! idempotent (a `Once`), so repeated parser construction is free.
//!
//! Thread ownership: the counter is a process-global [`AtomicUsize`]; the
//! wrappers run on whichever thread tree-sitter allocates from (typically
//! the decoration worker pool).

use std::alloc::{alloc, alloc_zeroed, dealloc, Layout};
use std::os::raw::c_void;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Once;

/// Net live bytes held by tree-sitter allocations (sum of requested sizes,
/// less frees). Excludes our per-block [`HEADER`] overhead.
static LIVE_BYTES: AtomicUsize = AtomicUsize::new(0);

/// Bytes reserved ahead of every returned pointer to stash the requested
/// size. Equal to the max alignment we hand back (16 = `max_align_t` on
/// x86-64), so the pointer tree-sitter sees stays 16-byte aligned.
const HEADER: usize = 16;

static INSTALL: Once = Once::new();

/// Exact live tree-sitter heap in bytes, process-wide. Zero until
/// [`install`] has run and tree-sitter has allocated.
#[must_use]
pub fn tree_sitter_heap_bytes() -> usize {
    LIVE_BYTES.load(Ordering::Relaxed)
}

/// Install the counting allocator into tree-sitter. Idempotent. See the
/// module-level safety contract: call before the first parse.
pub fn install() {
    INSTALL.call_once(|| {
        // SAFETY: invoked once, before any tree-sitter allocation (caller
        // contract, upheld by `MarkdownParser::new`). The four wrappers are
        // a consistent malloc/calloc/realloc/free family keyed on a size
        // header written at allocation time.
        unsafe {
            tree_sitter::set_allocator(
                Some(ts_malloc),
                Some(ts_calloc),
                Some(ts_realloc),
                Some(ts_free),
            );
        }
    });
}

/// Layout for a `requested + HEADER` block, 16-byte aligned. `None` only on
/// an absurd size that overflows `isize` (treated as allocation failure).
#[inline]
fn block_layout(requested: usize) -> Option<Layout> {
    let total = requested.checked_add(HEADER)?;
    Layout::from_size_align(total, HEADER).ok()
}

/// Allocate `size` user bytes behind a size header, returning the
/// user-visible (header-offset) pointer, or null on failure.
///
/// # Safety
/// Standard allocator contract; `base` is freshly allocated so the header
/// write is in-bounds.
unsafe fn alloc_with_header(size: usize, zeroed: bool) -> *mut c_void {
    let Some(layout) = block_layout(size) else {
        return std::ptr::null_mut();
    };
    let base = if zeroed {
        alloc_zeroed(layout)
    } else {
        alloc(layout)
    };
    if base.is_null() {
        return std::ptr::null_mut();
    }
    *(base as *mut usize) = size;
    LIVE_BYTES.fetch_add(size, Ordering::Relaxed);
    base.add(HEADER) as *mut c_void
}

unsafe extern "C" fn ts_malloc(size: usize) -> *mut c_void {
    alloc_with_header(size, false)
}

unsafe extern "C" fn ts_calloc(count: usize, size: usize) -> *mut c_void {
    let Some(total) = count.checked_mul(size) else {
        return std::ptr::null_mut();
    };
    alloc_with_header(total, true)
}

unsafe extern "C" fn ts_realloc(ptr: *mut c_void, new_size: usize) -> *mut c_void {
    if ptr.is_null() {
        return alloc_with_header(new_size, false);
    }
    let base = (ptr as *mut u8).sub(HEADER);
    let old_size = *(base as *mut usize);
    let Some(old_layout) = block_layout(old_size) else {
        return std::ptr::null_mut();
    };
    let Some(new_total) = new_size.checked_add(HEADER) else {
        return std::ptr::null_mut();
    };
    let new_base = std::alloc::realloc(base, old_layout, new_total);
    if new_base.is_null() {
        return std::ptr::null_mut();
    }
    *(new_base as *mut usize) = new_size;
    if new_size >= old_size {
        LIVE_BYTES.fetch_add(new_size - old_size, Ordering::Relaxed);
    } else {
        LIVE_BYTES.fetch_sub(old_size - new_size, Ordering::Relaxed);
    }
    new_base.add(HEADER) as *mut c_void
}

unsafe extern "C" fn ts_free(ptr: *mut c_void) {
    if ptr.is_null() {
        return;
    }
    let base = (ptr as *mut u8).sub(HEADER);
    let size = *(base as *mut usize);
    LIVE_BYTES.fetch_sub(size, Ordering::Relaxed);
    let layout = block_layout(size).expect("invariant: layout valid on free (was valid on alloc)");
    dealloc(base, layout);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn heap_grows_when_a_tree_is_held_and_shrinks_when_dropped() {
        // Installing twice is harmless (Once); other tests may have already
        // installed it.
        install();
        let mut parser = crate::parser::MarkdownParser::new().expect("grammar loads");
        let source = "# Heading\n\nsome *body* text with `code` and a [link](url).\n".repeat(200);

        let before = tree_sitter_heap_bytes();
        let tree = parser.parse(&source, None).expect("parse succeeds");
        let with_tree = tree_sitter_heap_bytes();
        assert!(
            with_tree > before,
            "holding a parsed tree must raise the live count ({before} -> {with_tree})"
        );

        drop(tree);
        drop(parser);
        let after = tree_sitter_heap_bytes();
        assert!(
            after <= with_tree,
            "dropping the tree + parser must not raise the live count ({with_tree} -> {after})"
        );
    }
}
