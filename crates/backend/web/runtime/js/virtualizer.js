// Virtualizer — JS-side scroll handler + visible-range diff for the
// web backend's `Primitive::Virtualizer`. Owns the scroll listener
// so the wasm-bindgen boundary is crossed only when items
// enter/leave the visible window, not on every scroll tick.
//
// The framework's `backend-web::create_virtualizer` injects this
// file's contents into a `<script>` tag on first use, then constructs
// an instance and hands it the Rust callbacks via wasm-bindgen-
// wrapped closures.
//
// Type sketch (JSDoc — keeps editor type-aware without a TS build):
//
// /**
//  * @typedef {Object} VirtualizerCallbacks
//  * @property {() => number} itemCount
//  * @property {(idx: number) => number} itemKey
//  * @property {(idx: number) => number} itemSize
//  * @property {(idx: number) => [Element, number]} mountItem
//  *   Returns [DOM node, scopeId]. Scope id is opaque to JS.
//  * @property {(scopeId: number) => void} releaseItem
//  * @property {(scopeId: number, size: number) => void} setMeasuredSize
//  *   Backend->framework: notify a measured size change.
//  * @property {boolean} measureSizes
//  * @property {number} overscan
//  * @property {boolean} horizontal
//  */

(function () {
    if (typeof window === 'undefined') return;
    if (window.__idealystVirtualizer) return; // idempotent inject

    class Virtualizer {
        /**
         * @param {HTMLElement} container — the outer scroller
         * @param {Object} cb — callbacks bundle (see typedef above)
         */
        constructor(container, cb) {
            this.container = container;
            this.cb = cb;
            this.horizontal = !!cb.horizontal;

            // Outer scroller (the container) holds an inner spacer
            // sized to the total content extent so the scrollbar is
            // accurate. Mounted items absolute-positioned within.
            container.style.overflow = this.horizontal ? 'auto hidden' : 'hidden auto';
            container.style.position = container.style.position || 'relative';

            this.spacer = document.createElement('div');
            this.spacer.style.position = 'relative';
            this.spacer.style.width = this.horizontal ? '0px' : '100%';
            this.spacer.style.height = this.horizontal ? '100%' : '0px';
            // The container might be a flex parent (the framework's
            // default class sets `display: flex; flex-direction: column`
            // on every styled view). Flex children won't honor an
            // explicit main-axis size if `flex-shrink` is the default
            // 1 — the spacer would collapse to fit the viewport
            // instead of forcing a scrollable extent. Pin shrink to 0
            // and basis to auto so the spacer's explicit width/height
            // wins.
            this.spacer.style.flexShrink = '0';
            this.spacer.style.flexBasis = 'auto';
            container.appendChild(this.spacer);

            /** @type {Map<number, {node: Element, scopeId: number, idx: number, key: number, size: number, observer?: ResizeObserver}>} */
            this.mountedByIdx = new Map();
            /** @type {Map<number, number>} Map key -> mountedByIdx idx, for keyed-diff preservation. */
            this.keyToIdx = new Map();

            this.lastStart = -1;
            this.lastEnd = -1;
            this.totalSize = 0;
            this.prefixSize = []; // prefixSize[i] = cumulative size of items [0, i)

            // Store the scroll handler as a named property so
            // `release()` can detach it. An anonymous arrow would
            // be impossible to removeEventListener.
            this._scrollHandler = () => {
                if (this._released) return;
                this.update();
            };
            container.addEventListener('scroll', this._scrollHandler, { passive: true });
            // Also re-update on container resize so viewport changes
            // trigger a re-window.
            if (typeof ResizeObserver !== 'undefined') {
                this._containerObserver = new ResizeObserver(() => {
                    if (this._released) return;
                    this.update();
                });
                this._containerObserver.observe(container);
            }
            this._released = false;

            // Defer the initial mount pass to a microtask: the Rust
            // side that constructed us still holds a `borrow_mut` on
            // the WebBackend RefCell. If we mount synchronously here,
            // `mountItem()` would re-enter the same RefCell from a
            // different call chain and trigger a "RefCell already
            // borrowed" panic.
            queueMicrotask(() => {
                if (this._released) return;
                this.refresh();
            });
        }

        /** Recompute prefix sums + spacer extent + visible range from scratch. */
        refresh() {
            if (this._released) return;
            const n = this.cb.itemCount();
            this.prefixSize = new Array(n + 1);
            this.prefixSize[0] = 0;
            for (let i = 0; i < n; i++) {
                this.prefixSize[i + 1] = this.prefixSize[i] + this.cb.itemSize(i);
            }
            this.totalSize = this.prefixSize[n];
            if (this.horizontal) {
                this.spacer.style.width = this.totalSize + 'px';
            } else {
                this.spacer.style.height = this.totalSize + 'px';
            }

            // Rebuild keyToIdx from current data.
            this.keyToIdx.clear();
            for (let i = 0; i < n; i++) {
                this.keyToIdx.set(this.cb.itemKey(i), i);
            }

            // Reconcile mounted set against the new data in two
            // passes. Phase 1: unmount any entry whose key is gone
            // (the underlying item was removed) — this is the only
            // place that drops DOM + per-item Rust scopes.
            for (const [oldIdx, entry] of this.mountedByIdx) {
                if (!this.keyToIdx.has(entry.key)) {
                    this._unmountEntry(oldIdx);
                }
            }
            // Phase 2: re-key every survivor by its NEW idx. Two
            // groups:
            //   - newIdx === entry.idx → item stayed in place; keep
            //     the existing map entry untouched.
            //   - newIdx !== entry.idx → item moved (reorder); we
            //     need to move it to a new map slot AND update its
            //     `entry.idx` field so subsequent `update()` calls
            //     position it correctly.
            //
            // The pre-fix version collected ONLY the moved entries
            // into `survivors`, then `mountedByIdx.clear()` wiped
            // every still-in-place entry from the map. The next
            // `update()` couldn't find them and called `mountItem`
            // again — even though the DOM nodes were still attached
            // — producing the doubled-render bug.
            //
            // Snapshot first so we can iterate while mutating the
            // map. `entries()` returns a live iterator on Map, but
            // we'd be deleting + inserting under the same iteration.
            const snapshot = Array.from(this.mountedByIdx.entries());
            const moved = [];
            for (const [oldIdx, entry] of snapshot) {
                const newIdx = this.keyToIdx.get(entry.key);
                if (newIdx === undefined) {
                    // Already unmounted in phase 1. Nothing to do.
                    continue;
                }
                if (newIdx !== oldIdx) {
                    this.mountedByIdx.delete(oldIdx);
                    moved.push({ entry, newIdx });
                }
            }
            for (const { entry, newIdx } of moved) {
                entry.idx = newIdx;
                this.mountedByIdx.set(newIdx, entry);
            }
            this.lastStart = -1;
            this.lastEnd = -1;
            this.update();
        }

        /**
         * Recompute visible range and apply mount/unmount diff.
         */
        update() {
            if (this._released) return;
            const scroll = this.horizontal ? this.container.scrollLeft : this.container.scrollTop;
            const viewport = this.horizontal ? this.container.clientWidth : this.container.clientHeight;
            const buffer = viewport * (this.cb.overscan || 1.0);

            const startOffset = scroll - buffer;
            const endOffset = scroll + viewport + buffer;

            // Binary-search the prefix sums to find first/last
            // indices whose [top, top + size) overlaps the buffer
            // range.
            const n = this.cb.itemCount();
            const start = this._findIndexAtOffset(Math.max(0, startOffset));
            const end = Math.min(
                n - 1,
                this._findIndexAtOffset(Math.max(0, endOffset))
            );

            if (start === this.lastStart && end === this.lastEnd) return;
            this.lastStart = start;
            this.lastEnd = end;

            // Unmount items outside [start, end].
            for (const [idx, _] of this.mountedByIdx) {
                if (idx < start || idx > end) {
                    this._unmountEntry(idx);
                }
            }

            // Mount items inside [start, end] not already mounted.
            for (let idx = start; idx <= end; idx++) {
                if (this.mountedByIdx.has(idx)) continue;
                this._mountIndex(idx);
            }

            // Reposition all currently-mounted entries (cheap; they
            // already exist, we just set top/left).
            for (const [idx, entry] of this.mountedByIdx) {
                this._positionEntry(idx, entry);
            }
        }

        _mountIndex(idx) {
            const [node, scopeId] = this.cb.mountItem(idx);
            const key = this.cb.itemKey(idx);
            const size = this.prefixSize[idx + 1] - this.prefixSize[idx];
            node.style.position = 'absolute';
            if (this.horizontal) {
                node.style.top = '0';
                node.style.height = '100%';
            } else {
                node.style.left = '0';
                node.style.width = '100%';
            }
            this.spacer.appendChild(node);
            const entry = { node, scopeId, idx, key, size };
            this.mountedByIdx.set(idx, entry);
            this.keyToIdx.set(key, idx);

            // If we measure sizes, install a ResizeObserver. On
            // rendered-size change, push the new value back to Rust
            // and ourselves, then refresh layout.
            if (this.cb.measureSizes && typeof ResizeObserver !== 'undefined') {
                const obs = new ResizeObserver(() => {
                    const rect = node.getBoundingClientRect();
                    const newSize = this.horizontal ? rect.width : rect.height;
                    if (Math.abs(newSize - entry.size) < 0.5) return;
                    entry.size = newSize;
                    this.cb.setMeasuredSize(scopeId, newSize);
                    this.refresh();
                });
                obs.observe(node);
                entry.observer = obs;
            }
        }

        _unmountEntry(idx) {
            const entry = this.mountedByIdx.get(idx);
            if (!entry) return;
            if (entry.observer) entry.observer.disconnect();
            if (entry.node.parentNode === this.spacer) {
                this.spacer.removeChild(entry.node);
            }
            this.cb.releaseItem(entry.scopeId);
            this.mountedByIdx.delete(idx);
        }

        _positionEntry(idx, entry) {
            const off = this.prefixSize[idx] + 'px';
            if (this.horizontal) {
                entry.node.style.left = off;
            } else {
                entry.node.style.top = off;
            }
        }

        /**
         * Find the first index `i` such that prefixSize[i+1] > offset.
         * Binary search. Clamps to [0, n-1].
         */
        _findIndexAtOffset(offset) {
            let lo = 0;
            let hi = this.prefixSize.length - 1;
            while (lo < hi) {
                const mid = (lo + hi) >> 1;
                if (this.prefixSize[mid + 1] > offset) {
                    hi = mid;
                } else {
                    lo = mid + 1;
                }
            }
            return lo;
        }

        /** Called by Rust when data changes (item_count effect fires).
         * Deferred to a microtask so we don't re-enter the
         * `WebBackend` RefCell while the framework still holds
         * `borrow_mut` from the call site that fired the data-change
         * effect. */
        dataChanged() {
            if (this._released) return;
            queueMicrotask(() => {
                if (this._released) return;
                this.refresh();
            });
        }

        /** Called by Rust from `release_virtualizer` when the
         * surrounding scope (a `when` branch, a `switch` arm, the
         * containing `Owner`) drops. Detaches every DOM listener,
         * disconnects all observers, unmounts everything currently
         * mounted, and flips `_released` so any late-firing handlers
         * (queued scroll/resize events, layout-change callbacks)
         * short-circuit instead of calling back into Rust closures
         * that may already have had their captured Signal scope
         * dropped.
         *
         * After `release()` the instance is inert; callers should
         * drop their references so JS can GC the instance + its
         * attached `_rust_cb_*` Closure wrappers. */
        release() {
            // Idempotent: a separate `_releasedFully` flag tracks
            // whether the full teardown has run. The Rust side may
            // set `_released = true` synchronously before invoking
            // `release()` from a microtask (see `release` in the
            // Rust virtualizer module). The `_released` flag is the
            // event-guard for queued listeners; this flag prevents
            // double-running the unmount loop.
            if (this._releasedFully) return;
            this._releasedFully = true;
            console.log('[virt] release() called, setting _released=true');
            this._released = true;
            // 1. Stop listening for new scroll/resize events.
            this.container.removeEventListener('scroll', this._scrollHandler);
            if (this._containerObserver) {
                this._containerObserver.disconnect();
                this._containerObserver = null;
            }
            // 2. Unmount everything currently in the window. This
            //    detaches each item's ResizeObserver too.
            //    `_unmountEntry` calls `releaseItem(scopeId)` back
            //    into Rust to drop the per-item Scope. By this
            //    point the framework's outer `borrow_mut()` has
            //    been released (the Rust caller microtask-defers
            //    this release call), so per-item Scope drops are
            //    free to re-borrow the backend via
            //    `on_node_unstyled` / similar paths.
            for (const idx of Array.from(this.mountedByIdx.keys())) {
                this._unmountEntry(idx);
            }
            // 3. Clear the spacer so the DOM tree is empty too.
            if (this.spacer && this.spacer.parentNode === this.container) {
                this.container.removeChild(this.spacer);
            }
        }
    }

    window.__idealystVirtualizer = Virtualizer;
})();
