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
// # Lanes / grid
//
// The engine is lane-based. A *list* is one lane: each item owns a
// full cross-axis line. A *grid* is N lanes: item `i` lands in lane
// `i % L` of grid-row `i / L`. Range math runs over grid-rows (a
// prefix-sum of per-row main-axis extents), then expands the visible
// grid-rows to the item indices `[rowStart*L, (rowEnd+1)*L)`. The
// list path is exactly the grid path with `L === 1`, so there is no
// separate code path and no list-vs-grid branch in the hot loop.
//
// `lanesFixed` (a column count, 1 = list) and `lanesMinCross` (a
// responsive minimum lane size, CSS `auto-fill`-style) are mutually
// exclusive; the Rust side sets exactly one. `mainSpacing` is the gap
// between grid-rows along the scroll axis, `crossSpacing` the gap
// between lanes.
//
// Type sketch (JSDoc — keeps editor type-aware without a TS build):
//
// /**
//  * @typedef {Object} VirtualizerCallbacks
//  * @property {() => number} itemCount
//  * @property {(idx: number) => number} itemKey
//  * @property {(idx: number) => number} itemSize  Main-axis extent.
//  * @property {(idx: number) => [Element, number]} mountItem
//  *   Returns [DOM node, scopeId]. Scope id is opaque to JS.
//  * @property {(scopeId: number) => void} releaseItem
//  * @property {(scopeId: number, size: number) => void} setMeasuredSize
//  *   Backend->framework: notify a measured size change.
//  * @property {boolean} measureSizes
//  * @property {number} overscan
//  * @property {boolean} horizontal
//  * @property {number} [lanesFixed]   Fixed lane count (>=1).
//  * @property {number} [lanesMinCross] Responsive min lane cross size.
//  * @property {number} [mainSpacing]
//  * @property {number} [crossSpacing]
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
            this.mainSpacing = +cb.mainSpacing || 0;
            this.crossSpacing = +cb.crossSpacing || 0;
            // Lane config: exactly one of the two is meaningful.
            this.lanesFixed = cb.lanesFixed != null ? Math.max(1, cb.lanesFixed | 0) : null;
            this.lanesMinCross = cb.lanesMinCross != null ? +cb.lanesMinCross : null;
            // Resolved at layout time from the container cross extent.
            this.lanes = this.lanesFixed || 1;

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
            // prefixRow[r] = main-axis offset of grid-row r (cumulative
            // row extents + inter-row spacing). Length = rowCount + 1.
            this.prefixRow = [0];
            this.rowCount = 0;
            // Resolved cross-axis extent per lane, set in refresh().
            this.laneCross = 0;

            // Store the scroll handler as a named property so
            // `release()` can detach it. An anonymous arrow would
            // be impossible to removeEventListener.
            this._scrollHandler = () => {
                if (this._released) return;
                this.update();
            };
            container.addEventListener('scroll', this._scrollHandler, { passive: true });
            // Also re-update on container resize so viewport changes
            // trigger a re-window. A resize can also change the
            // resolved lane count (AutoFit) or lane width — if so we
            // need a full `refresh()`, not just a range `update()`.
            if (typeof ResizeObserver !== 'undefined') {
                this._containerObserver = new ResizeObserver(() => {
                    if (this._released) return;
                    const prevLanes = this.lanes;
                    const prevCross = this.laneCross;
                    if (this._resolveLanes() !== prevLanes) {
                        this.refresh();
                    } else if (Math.abs(this.laneCross - prevCross) > 0.5) {
                        // Lane count steady but each lane's width moved
                        // (container resized): reposition without a full
                        // remount by re-running the range pass.
                        this.update();
                    } else {
                        this.update();
                    }
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

        /** The container's cross-axis extent (perpendicular to scroll). */
        _crossExtent() {
            return this.horizontal ? this.container.clientHeight : this.container.clientWidth;
        }

        /**
         * Resolve the concrete lane count + per-lane cross extent from
         * the container's current cross size. Returns the lane count
         * (also stored on `this.lanes`); updates `this.laneCross`.
         */
        _resolveLanes() {
            const cross = this._crossExtent();
            let lanes;
            if (this.lanesFixed != null) {
                lanes = this.lanesFixed;
            } else if (this.lanesMinCross != null && this.lanesMinCross > 0 && cross > 0) {
                // Largest N with N*min + (N-1)*gap <= cross.
                lanes = Math.floor((cross + this.crossSpacing) / (this.lanesMinCross + this.crossSpacing));
                lanes = Math.max(1, lanes);
            } else {
                lanes = 1;
            }
            this.lanes = lanes;
            // Per-lane cross extent: divide the remaining space after
            // inter-lane gaps. Guard a zero/unknown container.
            this.laneCross = lanes > 0 && cross > 0
                ? (cross - (lanes - 1) * this.crossSpacing) / lanes
                : 0;
            return lanes;
        }

        /** Main-axis extent of grid-row `r` = max item size across its lanes. */
        _rowExtent(r, n) {
            const L = this.lanes;
            let ext = 0;
            for (let lane = 0; lane < L; lane++) {
                const idx = r * L + lane;
                if (idx >= n) break;
                const s = this.cb.itemSize(idx);
                if (s > ext) ext = s;
            }
            return ext;
        }

        /** Recompute lanes + prefix sums + spacer extent + visible range from scratch. */
        refresh() {
            if (this._released) return;
            this._resolveLanes();
            const L = this.lanes;
            const n = this.cb.itemCount();
            this.rowCount = L > 0 ? Math.ceil(n / L) : 0;

            // Prefix-sum over grid-rows. prefixRow[r+1] = prefixRow[r]
            // + rowExtent(r) + mainSpacing. Trailing spacing is trimmed
            // from the total content extent below.
            this.prefixRow = new Array(this.rowCount + 1);
            this.prefixRow[0] = 0;
            for (let r = 0; r < this.rowCount; r++) {
                this.prefixRow[r + 1] = this.prefixRow[r] + this._rowExtent(r, n) + this.mainSpacing;
            }
            this.totalSize = this.rowCount > 0
                ? this.prefixRow[this.rowCount] - this.mainSpacing
                : 0;
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
            const L = this.lanes;
            const scroll = this.horizontal ? this.container.scrollLeft : this.container.scrollTop;
            const viewport = this.horizontal ? this.container.clientWidth : this.container.clientHeight;
            const buffer = viewport * (this.cb.overscan || 1.0);

            const startOffset = scroll - buffer;
            const endOffset = scroll + viewport + buffer;

            // Binary-search prefix-row sums for the first/last grid-row
            // overlapping the buffered window, then expand to item
            // indices. A grid-row of L lanes covers L consecutive
            // item indices.
            const n = this.cb.itemCount();
            const rowStart = this._findRowAtOffset(Math.max(0, startOffset));
            const rowEnd = Math.min(
                this.rowCount - 1,
                this._findRowAtOffset(Math.max(0, endOffset))
            );
            const start = rowStart * L;
            const end = Math.min(n - 1, (rowEnd + 1) * L - 1);

            if (start === this.lastStart && end === this.lastEnd) {
                // Range steady, but lane geometry may have shifted on a
                // resize — reposition mounted entries cheaply.
                for (const [idx, entry] of this.mountedByIdx) {
                    this._positionEntry(idx, entry);
                }
                return;
            }
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
            // already exist, we just set top/left + cross size).
            for (const [idx, entry] of this.mountedByIdx) {
                this._positionEntry(idx, entry);
            }
        }

        _mountIndex(idx) {
            const [node, scopeId] = this.cb.mountItem(idx);
            const key = this.cb.itemKey(idx);
            const size = this.cb.itemSize(idx);
            node.style.position = 'absolute';
            this.spacer.appendChild(node);
            const entry = { node, scopeId, idx, key, size };
            this.mountedByIdx.set(idx, entry);
            this.keyToIdx.set(key, idx);
            this._positionEntry(idx, entry);

            // If we measure sizes, install a ResizeObserver. On
            // rendered-size change, push the new value back to Rust
            // and ourselves, then refresh layout. We observe the
            // main-axis dimension only — the cross axis is pinned to
            // the lane width.
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

        /**
         * Place an entry at its (main, cross) position. Main offset
         * comes from its grid-row's prefix sum; cross offset + size
         * from its lane. In single-lane (list) mode the item fills the
         * cross axis exactly as before.
         */
        _positionEntry(idx, entry) {
            const L = this.lanes;
            const row = (idx / L) | 0;
            const lane = idx - row * L;
            const main = this.prefixRow[row] || 0;
            const crossOff = lane * (this.laneCross + this.crossSpacing);
            const node = entry.node;
            if (this.horizontal) {
                node.style.left = main + 'px';
                if (L > 1) {
                    node.style.top = crossOff + 'px';
                    node.style.height = this.laneCross + 'px';
                } else {
                    node.style.top = '0';
                    node.style.height = '100%';
                }
            } else {
                node.style.top = main + 'px';
                if (L > 1) {
                    node.style.left = crossOff + 'px';
                    node.style.width = this.laneCross + 'px';
                } else {
                    node.style.left = '0';
                    node.style.width = '100%';
                }
            }
        }

        /**
         * Find the first grid-row `r` such that prefixRow[r+1] > offset.
         * Binary search. Clamps to [0, rowCount-1].
         */
        _findRowAtOffset(offset) {
            let lo = 0;
            let hi = this.prefixRow.length - 1;
            while (lo < hi) {
                const mid = (lo + hi) >> 1;
                if (this.prefixRow[mid + 1] > offset) {
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
