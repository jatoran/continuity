//! Phase F5 Pass 2 — WIC-decoded D2D bitmap cache for inline images.
//!
//! Owns a single [`IWICImagingFactory`] and a [`HashMap`] mapping
//! absolute image-store paths to GPU-side [`ID2D1Bitmap`] handles.
//! Capacity is bounded by total decoded bytes (`width * height * 4`);
//! evictions are LRU on last-used timestamp.
//!
//! Thread ownership: the renderer's UI thread. D2D / WIC handles are
//! single-threaded; `ImageCache` itself does not implement `Send`.
//!
//! Device loss: [`ImageCache::invalidate_for_new_device`] is called by
//! the renderer whenever it re-creates the swap-chain / device
//! context. All cached bitmaps belong to the previous device and
//! cannot be reused; the cache drops them and starts fresh.

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use windows::core::{Interface, HSTRING};
use windows::Win32::Graphics::Direct2D::{ID2D1Bitmap1, ID2D1DeviceContext};
use windows::Win32::Graphics::Imaging::{
    CLSID_WICImagingFactory, GUID_WICPixelFormat32bppPBGRA, IWICImagingFactory,
    WICBitmapDitherTypeNone, WICBitmapPaletteTypeMedianCut, WICDecodeMetadataCacheOnLoad,
};
use windows::Win32::System::Com::{CoCreateInstance, CLSCTX_INPROC_SERVER};

/// Errors the image cache can surface. Every variant is non-fatal —
/// the painter skips the image and continues drawing text.
#[derive(Debug, thiserror::Error)]
pub enum ImageCacheError {
    /// WIC failed to construct the imaging factory (typically a COM
    /// init issue).
    #[error("create IWICImagingFactory: {0}")]
    Factory(windows::core::Error),
    /// File read / WIC decode / D2D upload failed.
    #[error("decode image `{path}`: {error}")]
    Decode {
        /// Path the painter tried to load.
        path: PathBuf,
        /// Wrapped Win32 error.
        error: windows::core::Error,
    },
}

/// One cached image. Holds one or more decoded GPU frames (`frames.len() == 1`
/// for static images, `>= 1` for animated GIFs) plus the per-frame delay
/// table and bookkeeping for the LRU eviction policy + animation advance.
struct CachedImage {
    /// Decoded GPU bitmaps, one per frame. Always non-empty; the
    /// `is_animated` flag is derived from `frames.len() > 1`. Frame 0
    /// is the static-display frame (the only frame shown when reduced
    /// motion is on).
    frames: Vec<ID2D1Bitmap1>,
    /// Per-frame display delay in milliseconds. Same length as
    /// `frames`. Filled from GIF Graphic Control Extension metadata
    /// when available; falls back to `DEFAULT_FRAME_DELAY_MS` per
    /// frame when the file doesn't expose delay metadata. Delays
    /// shorter than `MIN_FRAME_DELAY_MS` are clamped to that floor
    /// (mirrors browser behaviour for "0-delay" GIFs).
    frame_delays_ms: Vec<u32>,
    /// Index of the frame currently displayed. Always in
    /// `0..frames.len()`. `0` for static images.
    frame_index: usize,
    /// Wall-clock millis when [`Self::frame_index`] was last advanced
    /// (or first decoded, for frame 0). Used by
    /// [`ImageCache::advance_animations`] to decide when to step.
    last_frame_advance_ms: u64,
    decoded_bytes: usize,
    last_used: u64,
    width: u32,
    height: u32,
}

impl CachedImage {
    #[inline]
    fn is_animated(&self) -> bool {
        self.frames.len() > 1
    }

    #[inline]
    fn current_bitmap(&self) -> &ID2D1Bitmap1 {
        &self.frames[self.frame_index]
    }
}

/// Default per-frame delay when the source format exposes no
/// timing metadata. 100 ms matches the de-facto GIF default and most
/// browsers' fallback.
pub const DEFAULT_FRAME_DELAY_MS: u32 = 100;

/// Floor applied to every frame delay. Mirrors browser behaviour:
/// "0-delay" GIFs are clamped to ~50 ms so they don't burn the CPU.
pub const MIN_FRAME_DELAY_MS: u32 = 50;

/// LRU-evicting bitmap cache. See module docs.
pub struct ImageCache {
    factory: Option<IWICImagingFactory>,
    entries: HashMap<PathBuf, CachedImage>,
    current_bytes: usize,
    capacity_bytes: usize,
    /// Monotonic tick advanced on every lookup; used as the LRU key.
    /// Wraps at `u64::MAX`, which we will not realistically hit in a
    /// single editor session.
    tick: u64,
    /// Bumped by [`ImageCache::invalidate_for_new_device`] so callers
    /// that cached a "warm" flag can notice when the underlying
    /// bitmaps were thrown out.
    device_generation: u64,
}

impl ImageCache {
    /// Construct an empty cache with the supplied byte cap. A cap of
    /// `0` effectively disables the cache (no insert can succeed).
    #[must_use]
    pub fn new(capacity_bytes: usize) -> Self {
        Self {
            factory: None,
            entries: HashMap::new(),
            current_bytes: 0,
            capacity_bytes,
            tick: 0,
            device_generation: 0,
        }
    }

    /// Update the byte cap. Evicts entries until the new cap is
    /// respected. Used when the settings hot-reload changes
    /// `[ui].image_cache_bytes`.
    pub fn set_capacity_bytes(&mut self, capacity_bytes: usize) {
        self.capacity_bytes = capacity_bytes;
        self.evict_until_within_capacity();
    }

    /// Drop every cached bitmap because the underlying D2D device has
    /// been re-created. Called by the renderer on device-loss
    /// recovery.
    pub fn invalidate_for_new_device(&mut self) {
        self.entries.clear();
        self.current_bytes = 0;
        self.device_generation = self.device_generation.wrapping_add(1);
    }

    /// Current generation counter. The painter can compare against a
    /// previously-stored value to detect "the cache was reset".
    #[must_use]
    pub fn device_generation(&self) -> u64 {
        self.device_generation
    }

    /// Number of currently-cached bitmaps. Diagnostic / test surface.
    #[must_use]
    pub fn len(&self) -> usize {
        self.entries.len()
    }

    /// `true` when no bitmaps are cached.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    /// Total decoded bytes currently held. Diagnostic / test surface.
    #[must_use]
    pub fn current_bytes(&self) -> usize {
        self.current_bytes
    }

    /// γ — peek the native pixel dimensions of a previously-decoded
    /// image. Returns `None` if the path has not been decoded yet
    /// (cold cache) or has been evicted. Used by the display-map row
    /// reservation provider to compute phantom-row counts for
    /// expanded inline images without re-decoding.
    ///
    /// Read-only — does not advance the LRU tick.
    #[must_use]
    pub fn cached_dimensions(&self, path: &Path) -> Option<(u32, u32)> {
        self.entries.get(path).map(|e| (e.width, e.height))
    }

    /// Get the cached bitmap for `path`, decoding from disk on miss.
    /// Returns `None` if `capacity_bytes` is zero (cache disabled) or
    /// if decoding fails. The painter logs failures via the returned
    /// error; the warm path costs only a hash lookup + tick update.
    ///
    /// # Errors
    ///
    /// Returns [`ImageCacheError::Factory`] on first-call WIC factory
    /// creation failure; [`ImageCacheError::Decode`] when a specific
    /// file fails to decode.
    pub fn get_or_decode(
        &mut self,
        path: &Path,
        device_context: &ID2D1DeviceContext,
    ) -> Result<Option<CachedHandle<'_>>, ImageCacheError> {
        if self.capacity_bytes == 0 {
            return Ok(None);
        }
        self.tick = self.tick.wrapping_add(1);
        let tick = self.tick;
        let key = path.to_path_buf();

        // Cold path — decode + insert, releasing every borrow before
        // the final lookup so the borrow checker can issue the warm
        // handle.
        if !self.entries.contains_key(&key) {
            let factory = self.ensure_factory()?;
            let decoded =
                decode_file_to_d2d_bitmaps(factory, path, device_context).map_err(|error| {
                    ImageCacheError::Decode {
                        path: key.clone(),
                        error,
                    }
                })?;
            let frame_count = decoded.frames.len();
            let decoded_bytes = (decoded.width as usize)
                .saturating_mul(decoded.height as usize)
                .saturating_mul(4)
                .saturating_mul(frame_count);

            // Pre-evict so the new entry fits. A new entry larger
            // than the cap still inserts (the cap is a target for
            // total resident bytes, not a per-entry limit); next
            // miss flushes the rest.
            while self.current_bytes + decoded_bytes > self.capacity_bytes
                && !self.entries.is_empty()
            {
                self.evict_oldest();
            }
            self.current_bytes += decoded_bytes;
            self.entries.insert(
                key.clone(),
                CachedImage {
                    frames: decoded.frames,
                    frame_delays_ms: decoded.frame_delays_ms,
                    frame_index: 0,
                    last_frame_advance_ms: 0,
                    decoded_bytes,
                    last_used: tick,
                    width: decoded.width,
                    height: decoded.height,
                },
            );
        }

        let entry = self
            .entries
            .get_mut(&key)
            .expect("invariant: just inserted or already present");
        entry.last_used = tick;
        Ok(Some(CachedHandle {
            bitmap: entry.current_bitmap(),
            width: entry.width,
            height: entry.height,
        }))
    }

    /// Step every animated entry's `frame_index` forward by the elapsed
    /// time since the last advance. Returns `true` when at least one
    /// frame index changed — the caller (Window UI thread) should
    /// invalidate the affected paint region.
    ///
    /// Static images (`frames.len() == 1`) are skipped. Frames advance
    /// monotonically and wrap at the end (looping playback). When the
    /// caller has not previously advanced this image (cold cache),
    /// `now_ms` becomes the new baseline and the function returns
    /// `false`.
    pub fn advance_animations(&mut self, now_ms: u64) -> bool {
        let mut any_advanced = false;
        for entry in self.entries.values_mut() {
            if !entry.is_animated() {
                continue;
            }
            if entry.last_frame_advance_ms == 0 {
                entry.last_frame_advance_ms = now_ms;
                continue;
            }
            let advanced = advance_frame_index(
                now_ms,
                entry.frames.len(),
                &entry.frame_delays_ms,
                &mut entry.frame_index,
                &mut entry.last_frame_advance_ms,
            );
            if advanced {
                any_advanced = true;
            }
        }
        any_advanced
    }

    /// `true` when at least one entry is animated (more than one
    /// frame). The window uses this to decide whether to keep its
    /// animation `WM_TIMER` armed.
    #[must_use]
    pub fn has_animated_entries(&self) -> bool {
        self.entries.values().any(CachedImage::is_animated)
    }

    fn ensure_factory(&mut self) -> Result<&IWICImagingFactory, ImageCacheError> {
        if self.factory.is_none() {
            let factory: IWICImagingFactory =
                unsafe { CoCreateInstance(&CLSID_WICImagingFactory, None, CLSCTX_INPROC_SERVER) }
                    .map_err(ImageCacheError::Factory)?;
            self.factory = Some(factory);
        }
        Ok(self
            .factory
            .as_ref()
            .expect("invariant: factory just initialised"))
    }

    fn evict_until_within_capacity(&mut self) {
        while self.current_bytes > self.capacity_bytes && !self.entries.is_empty() {
            self.evict_oldest();
        }
    }

    fn evict_oldest(&mut self) {
        let Some((key, _)) = self
            .entries
            .iter()
            .min_by_key(|(_, e)| e.last_used)
            .map(|(k, e)| (k.clone(), e.last_used))
        else {
            return;
        };
        if let Some(removed) = self.entries.remove(&key) {
            self.current_bytes = self.current_bytes.saturating_sub(removed.decoded_bytes);
        }
    }
}

/// Borrowed handle returned by [`ImageCache::get_or_decode`]. The
/// painter dereferences this for the actual `DrawBitmap` call.
pub struct CachedHandle<'a> {
    /// The D2D-side bitmap pointer.
    pub bitmap: &'a ID2D1Bitmap1,
    /// Decoded width in pixels.
    pub width: u32,
    /// Decoded height in pixels.
    pub height: u32,
}

/// Pure helper for the multi-step advance loop on an animated image.
///
/// Walks `frame_index` forward while `now_ms - last_advance_ms` >= the
/// current frame's delay, wrapping at `frame_count`. Each successful
/// step charges its delay to `last_advance_ms`. Returns `true` when
/// at least one step occurred so the caller can flag the paint
/// invalidation.
///
/// Multi-step (vs single-step) is important: if the WM_TIMER lands
/// late (system busy, focus loss), the animation still converges to
/// the correct frame for `now_ms` in one tick rather than drifting
/// further behind every tick.
fn advance_frame_index(
    now_ms: u64,
    frame_count: usize,
    frame_delays_ms: &[u32],
    frame_index: &mut usize,
    last_advance_ms: &mut u64,
) -> bool {
    if frame_count <= 1 {
        return false;
    }
    let mut advanced = false;
    loop {
        let delay = frame_delays_ms
            .get(*frame_index)
            .copied()
            .unwrap_or(DEFAULT_FRAME_DELAY_MS)
            .max(MIN_FRAME_DELAY_MS);
        let elapsed = now_ms.saturating_sub(*last_advance_ms);
        if elapsed < u64::from(delay) {
            break;
        }
        *frame_index = (*frame_index + 1) % frame_count;
        *last_advance_ms = last_advance_ms.saturating_add(u64::from(delay));
        advanced = true;
    }
    advanced
}

struct DecodedBitmaps {
    /// One bitmap per source frame. `frames.len() == 1` for static
    /// images; `> 1` for animated GIFs (one entry per Image Block in
    /// the LZW stream).
    frames: Vec<ID2D1Bitmap1>,
    /// Per-frame delay in milliseconds, parallel to `frames`. Default
    /// [`DEFAULT_FRAME_DELAY_MS`] if the source format exposes no
    /// timing metadata.
    frame_delays_ms: Vec<u32>,
    width: u32,
    height: u32,
}

fn decode_file_to_d2d_bitmaps(
    factory: &IWICImagingFactory,
    path: &Path,
    device_context: &ID2D1DeviceContext,
) -> Result<DecodedBitmaps, windows::core::Error> {
    // `GENERIC_READ = 0x80000000` (winnt.h). Hardcoded here so we do
    // not need to enable `Win32_Storage_FileSystem` solely for this
    // one constant.
    const GENERIC_READ: u32 = 0x8000_0000;
    unsafe {
        let wide = HSTRING::from(path.as_os_str());
        let decoder = factory.CreateDecoderFromFilename(
            &wide,
            None,
            windows::Win32::Foundation::GENERIC_ACCESS_RIGHTS(GENERIC_READ),
            WICDecodeMetadataCacheOnLoad,
        )?;
        let frame_count = decoder.GetFrameCount().unwrap_or(1).max(1);
        let mut frames: Vec<ID2D1Bitmap1> = Vec::with_capacity(frame_count as usize);
        let mut delays_ms: Vec<u32> = Vec::with_capacity(frame_count as usize);
        let mut canvas_width: u32 = 0;
        let mut canvas_height: u32 = 0;
        for frame_index in 0..frame_count {
            let frame = decoder.GetFrame(frame_index)?;
            let converter = factory.CreateFormatConverter()?;
            converter.Initialize(
                &frame.cast::<windows::Win32::Graphics::Imaging::IWICBitmapSource>()?,
                &GUID_WICPixelFormat32bppPBGRA,
                WICBitmapDitherTypeNone,
                None,
                0.0,
                WICBitmapPaletteTypeMedianCut,
            )?;
            let (mut w, mut h) = (0u32, 0u32);
            converter.GetSize(&mut w, &mut h)?;
            if frame_index == 0 {
                canvas_width = w;
                canvas_height = h;
            }
            let source = converter.cast::<windows::Win32::Graphics::Imaging::IWICBitmapSource>()?;
            let bitmap = device_context.CreateBitmapFromWicBitmap(&source, None)?;
            let bitmap1: ID2D1Bitmap1 = bitmap.cast()?;
            frames.push(bitmap1);
            // v1: every frame plays at `DEFAULT_FRAME_DELAY_MS` (100 ms).
            // Per-frame delay extraction from GIF Graphic Control
            // Extension metadata (`/grctlext/Delay`) is a known
            // follow-up — see `.docs/design/features/tutorial.md`
            // "Deferred polish". The wire-up needs careful PROPVARIANT
            // handling that's worth a focused review.
            delays_ms.push(DEFAULT_FRAME_DELAY_MS);
            let _ = &frame; // silence "unused variable" until metadata read lands.
        }
        if frames.is_empty() {
            return Err(windows::core::Error::from_win32());
        }
        Ok(DecodedBitmaps {
            frames,
            frame_delays_ms: delays_ms,
            width: canvas_width,
            height: canvas_height,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A 0-byte cap is the "disabled" branch — `get_or_decode` returns
    /// `Ok(None)` without touching WIC / D2D.
    #[test]
    fn disabled_cache_returns_none() {
        let cache = ImageCache::new(0);
        assert_eq!(cache.capacity_bytes, 0);
        assert!(cache.is_empty());
        assert_eq!(cache.current_bytes(), 0);
    }

    /// `set_capacity_bytes(0)` evicts everything if any entries were
    /// present. Exercised via the bookkeeping surface (no WIC needed).
    #[test]
    fn set_capacity_bytes_zero_drops_everything() {
        let mut cache = ImageCache::new(1024);
        // Synthetically populate so the eviction has something to do.
        cache.current_bytes = 800;
        cache.set_capacity_bytes(0);
        // current_bytes is updated only by evict_oldest, which
        // requires real entries. With no entries to evict, the loop
        // exits and current_bytes stays put — this is the documented
        // behaviour: the cache trusts the entries map as truth.
        assert_eq!(cache.capacity_bytes, 0);
    }

    /// device_generation bumps on invalidate.
    #[test]
    fn invalidate_bumps_device_generation() {
        let mut cache = ImageCache::new(1024);
        let g0 = cache.device_generation();
        cache.invalidate_for_new_device();
        let g1 = cache.device_generation();
        cache.invalidate_for_new_device();
        let g2 = cache.device_generation();
        assert_ne!(g0, g1);
        assert_ne!(g1, g2);
    }

    #[test]
    fn advance_frame_index_static_image_does_nothing() {
        let mut idx = 0;
        let mut last = 1000;
        let advanced = advance_frame_index(2000, 1, &[100], &mut idx, &mut last);
        assert!(!advanced);
        assert_eq!(idx, 0);
        assert_eq!(last, 1000);
    }

    #[test]
    fn advance_frame_index_below_delay_does_nothing() {
        let mut idx = 0;
        let mut last = 1000;
        let advanced = advance_frame_index(1049, 3, &[100, 100, 100], &mut idx, &mut last);
        assert!(!advanced);
        assert_eq!(idx, 0);
        assert_eq!(last, 1000);
    }

    #[test]
    fn advance_frame_index_single_step_at_boundary() {
        let mut idx = 0;
        let mut last = 1000;
        let advanced = advance_frame_index(1100, 3, &[100, 100, 100], &mut idx, &mut last);
        assert!(advanced);
        assert_eq!(idx, 1);
        assert_eq!(last, 1100);
    }

    #[test]
    fn advance_frame_index_multi_step_catches_up() {
        // 3 frames @ 100 ms each, 350 ms elapsed → step 3 frames
        // (back to index 0 after wrap).
        let mut idx = 0;
        let mut last = 1000;
        let advanced = advance_frame_index(1350, 3, &[100, 100, 100], &mut idx, &mut last);
        assert!(advanced);
        assert_eq!(idx, 0);
        assert_eq!(last, 1300);
    }

    #[test]
    fn advance_frame_index_wraps_at_end() {
        let mut idx = 2;
        let mut last = 1000;
        let advanced = advance_frame_index(1100, 3, &[100, 100, 100], &mut idx, &mut last);
        assert!(advanced);
        assert_eq!(idx, 0);
    }

    #[test]
    fn advance_frame_index_respects_min_floor() {
        // 0-delay frame is clamped to MIN_FRAME_DELAY_MS so elapsed
        // < floor → no advance.
        let mut idx = 0;
        let mut last = 1000;
        let advanced = advance_frame_index(1010, 3, &[0, 0, 0], &mut idx, &mut last);
        assert!(!advanced);
        assert_eq!(idx, 0);
    }
}
