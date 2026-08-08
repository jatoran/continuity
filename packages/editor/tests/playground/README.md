# Mobile playground

A LAN/Tailscale-reachable page that embeds `<continuity-editor>` so a real phone
can drive it. Headless Chromium cannot synthesize platform touch selection (a
long press emits `pointercancel` and produces no selection), so touch behaviour
that involves the OS is only verifiable on a device.

It serves two copies of the package side by side — `fixed/` (working tree) and
`baseline/` (a reference build) — so a defect can be A/B'd on the same phone in
two page loads rather than argued about.

## Run

```sh
# 1. Build/pack the package so a consumer install exists.
cargo xtask browser-check          # or reuse target/wasm-sdk/browser-consumer

# 2. Stage the two builds.
PLAYGROUND=target/wasm-sdk/mobile-playground
SOURCE=target/wasm-sdk/browser-consumer/node_modules/@continuity-editor/editor
mkdir -p "$PLAYGROUND"
cp -r "$SOURCE" "$PLAYGROUND/fixed"
cp -r "$SOURCE" "$PLAYGROUND/baseline"
cp packages/editor/src/*.js "$PLAYGROUND/fixed/src/"        # working tree
git show HEAD:packages/editor/src/component.js > "$PLAYGROUND/baseline/src/component.js"
# ...repeat for each file the change touches; drop any file the baseline lacks.
cp packages/editor/tests/playground/{index.html,app.mjs,server.mjs} "$PLAYGROUND/"

# 3. Serve on every interface.
node "$PLAYGROUND/server.mjs" "$PLAYGROUND" 8787
```

The server prints one URL per non-internal IPv4 interface. Append
`?build=baseline` (or press **swap build**) to load the unfixed copy.

## Reaching it from a phone

Windows Firewall blocks inbound by default and ships no rule for `node`, so the
LAN address usually fails. A Tailscale address works without any firewall
change, because Tailscale permits inbound on its own interface — prefer it.

## What the page shows

A diagnostics footer reports, live: engine caret offset vs textarea offset (they
must agree — a `DRIFT` means the drawn caret is lying about where typing lands),
the current selection, scroll position against the textarea's extent and the
projection height, the applied scroll-extent padding plus transform residual,
and whether the document's tail is reachable once scrolled to the floor.

Corpora are chosen to hit the layouts where the projection and the invisible
textarea diverge: long headings (which wrap into more projected rows than source
rows), heavily folded inline markup, wrapped list items with hanging indent, and
a 600-line paste.

## Physical tap versus scroll acceptance

`cargo xtask browser-check` verifies projected caret placement and focus-event
ordering with touch/coarse-pointer emulation. It cannot observe the OS keyboard,
keyboard flash, native inertial scrolling, or whether Android/iOS preserves
user activation through the real synthesized click. Those require this page on
physical devices.

Record the browser/device/OS build and check:

| Gesture | Android Chrome | iPhone/iPad Safari |
|---|---|---|
| Start unfocused; swipe vertically through projected text | Scroll remains native/inertial; Gboard never opens or flashes | Scroll remains native/inertial; iOS keyboard never opens or flashes |
| Tap a projected heading, folded inline marker, and wrapped list continuation | Visible caret lands under the finger; Gboard opens on the first tap | Visible caret lands under the finger; iOS keyboard opens on the first tap |
| Dismiss keyboard, then tap again | Caret moves and keyboard reopens | Caret moves and keyboard reopens |
| Tap during active composition | Composed text commits once; caret lands under finger; typing continues there | Composed text commits once; caret lands under finger; typing continues there |
| Long-press, drag, adjust handles, and use selection actions | Projection-owned selection remains correct | Projection-owned selection remains correct |
| Tap link, task checkbox, command rail, and code-copy control | Existing action behavior remains intact | Existing action behavior remains intact |

Physical Android/iOS execution is unavailable in the automated workspace. A
release that claims either lane must attach the completed matrix as manual
acceptance evidence; do not infer it from the headless pass.
