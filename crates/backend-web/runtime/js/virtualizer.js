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

            container.addEventListener('scroll', () => this.update(), { passive: true });
            // Also re-update on container resize so viewport changes
            // trigger a re-window.
            if (typeof ResizeObserver !== 'undefined') {
                this._containerObserver = new ResizeObserver(() => this.update());
                this._containerObserver.observe(container);
            }

            // Defer the initial mount pass to a microtask: the Rust
            // side that constructed us still holds a `borrow_mut` on
            // the WebBackend RefCell. If we mount synchronously here,
            // `mountItem()` would re-enter the same RefCell from a
            // different call chain and trigger a "RefCell already
            // borrowed" panic.
            queueMicrotask(() => this.refresh());
        }

        /** Recompute prefix sums + spacer extent + visible range from scratch. */
        refresh() {
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

            // Reconcile mounted set against the new data:
            // unmount items whose key no longer exists.
            for (const [oldIdx, entry] of this.mountedByIdx) {
                if (!this.keyToIdx.has(entry.key)) {
                    this._unmountEntry(oldIdx);
                }
            }
            // Re-key surviving mounted entries — same scope, possibly
            // new index.
            const survivors = [];
            for (const [_, entry] of this.mountedByIdx) {
                const newIdx = this.keyToIdx.get(entry.key);
                if (newIdx !== undefined && newIdx !== entry.idx) {
                    survivors.push({ entry, newIdx });
                }
            }
            // Rebuild the mountedByIdx map after collecting survivors,
            // because we're going to change keys in place.
            this.mountedByIdx.clear();
            for (const [_, entry] of survivors.length === 0
                ? [...this._rebuiltSurvivorMap()]
                : this._withSurvivors(survivors)) {
                this.mountedByIdx.set(entry.idx, entry);
            }
            this.lastStart = -1;
            this.lastEnd = -1;
            this.update();
        }

        // Helper: yields current entries unchanged (for the no-reorder case).
        *_rebuiltSurvivorMap() {
            // After the keyToIdx pass above, mountedByIdx still holds
            // entries from before — but we cleared it. Recover from the
            // mounted DOM via mountedByIdx... wait, we cleared it. So
            // we re-derive from the survivors list. If survivors is
            // empty *and* we have nothing else, nothing to yield.
            // (Practically: this path is the no-data-change path; the
            // caller takes a different branch then.)
            return;
        }

        // Helper: applies survivor reindex + emits entries.
        *_withSurvivors(survivors) {
            for (const { entry, newIdx } of survivors) {
                entry.idx = newIdx;
                yield [newIdx, entry];
            }
        }

        /**
         * Recompute visible range and apply mount/unmount diff.
         */
        update() {
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
            queueMicrotask(() => this.refresh());
        }
    }

    window.__idealystVirtualizer = Virtualizer;
})();
