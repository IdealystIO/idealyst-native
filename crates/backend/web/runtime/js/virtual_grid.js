// VirtualGrid — JS-side scroll handler + visible-rect diff for the web
// backend's `virtual_grid` primitive. Sibling of `virtualizer.js`, and
// owned by the same contract: this class owns the scroll listener so
// the wasm-bindgen boundary is crossed only when cells enter or leave
// the window, never on every scroll tick.
//
// # What's different from the 1-D shim
//
// `virtualizer.js` windows one axis and treats the cross axis as
// LANES that divide the viewport. This one windows BOTH axes: columns
// have author-chosen widths whose sum can exceed the viewport, so the
// container scrolls in two directions and the mounted set is the
// cross-product of two ranges.
//
// The range arithmetic itself is NOT here — it lives in Rust
// (`runtime_shared::primitives::virtual_grid::GridMetrics`), which is
// shared by every backend and tested once. Rust hands this class the
// resolved window; JS owns only the DOM (positioning, recycling, and
// the scroll/resize listeners). That split is deliberate: the 1-D
// engine's window math is re-derived in four places, which is exactly
// how implementations drift apart.
//
// Type sketch (JSDoc — keeps editor type-aware without a TS build):
//
// /**
//  * @typedef {Object} GridCallbacks
//  * @property {(scrollX: number, scrollY: number, vpW: number, vpH: number)
//  *            => {colStart:number,colEnd:number,rowStart:number,rowEnd:number,
//  *                contentW:number,contentH:number}} window
//  *   Ask Rust for the visible window + content size at this offset.
//  * @property {(col: number, row: number) => number} cellKey
//  * @property {(col: number, row: number) => [number, number]} cellOrigin
//  * @property {(col: number, row: number) => [Element, number, number, number, number, number]}
//  *   mountCell → [node, scopeId, x, y, w, h]
//  * @property {(scopeId: number) => void} releaseCell
//  * @property {(x: number, y: number) => void} [onScroll]
//  */

(function () {
    if (typeof window === 'undefined') return;
    if (window.__idealystVirtualGrid) return; // idempotent inject

    class VirtualGrid {
        /**
         * @param {HTMLElement} container — the outer two-axis scroller
         * @param {Object} cb — callbacks bundle (see typedef above)
         */
        constructor(container, cb) {
            this.container = container;
            this.cb = cb;

            // Two-axis scroller. `virtualizer.js` pins the off-axis to
            // `hidden`; here both axes genuinely scroll, which is the
            // whole primitive.
            container.style.overflow = 'auto';
            container.style.position = container.style.position || 'relative';

            // Spacer carries the full content extent so both scrollbars
            // are accurate; cells are absolutely positioned inside it.
            this.spacer = document.createElement('div');
            this.spacer.style.position = 'relative';
            // The container is usually a flex parent (the framework's
            // default class sets `display: flex; flex-direction: column`
            // on every styled view), and a flex child with the default
            // `flex-shrink: 1` collapses to the viewport instead of
            // forcing a scrollable extent — the same trap the 1-D
            // spacer documents.
            this.spacer.style.flexShrink = '0';
            this.spacer.style.flexBasis = 'auto';
            container.appendChild(this.spacer);

            /** @type {Map<string, {node: Element, scopeId: number, col: number, row: number, key: number}>} */
            this.mounted = new Map();

            this.lastWindow = null;

            this._scrollHandler = () => {
                if (this._released) return;
                this.update();
                if (this.cb.onScroll) {
                    this.cb.onScroll(this.container.scrollLeft, this.container.scrollTop);
                }
            };
            container.addEventListener('scroll', this._scrollHandler, { passive: true });

            if (typeof ResizeObserver !== 'undefined') {
                this._containerObserver = new ResizeObserver(() => {
                    if (this._released) return;
                    // A resize changes the viewport on both axes, so the
                    // window can grow or shrink in either direction.
                    // Force a re-diff rather than trusting the cached
                    // window, which was computed against the old size.
                    this.lastWindow = null;
                    this.update();
                });
                this._containerObserver.observe(container);
            }
            this._released = false;

            // Defer the initial fill to a microtask: the Rust side that
            // constructed us still holds a `borrow_mut` on the
            // WebBackend RefCell, and `mountCell` re-enters it.
            queueMicrotask(() => {
                if (this._released) return;
                this.refresh();
            });
        }

        /** Map key for a mounted cell. Cells are addressed by position,
         *  not by identity: the same `(col, row)` slot is what the
         *  window diff adds and removes. */
        _slot(col, row) {
            return col + ':' + row;
        }

        /** Counts or sizes changed — drop the cached window so the next
         *  update re-diffs from scratch, and re-check the key of every
         *  surviving cell: a data change can rewrite a cell's content
         *  while leaving the window rectangle identical, and the slot
         *  diff alone would keep the stale subtree mounted. Scroll
         *  updates skip the key pass — content can only change through
         *  `dataChanged`, so per-scroll key queries would be pure
         *  wasm-crossing overhead. */
        refresh() {
            if (this._released) return;
            this.lastWindow = null;
            this._revalidateKeys = true;
            this.update();
        }

        update() {
            if (this._released) return;
            const w = this.cb.window(
                this.container.scrollLeft,
                this.container.scrollTop,
                this.container.clientWidth,
                this.container.clientHeight,
            );
            if (!w) return;

            // Content extent first: the scrollbars must reflect the new
            // size before we position anything against it, or a shrink
            // leaves the scroller briefly scrolled past its own content.
            this.spacer.style.width = w.contentW + 'px';
            this.spacer.style.height = w.contentH + 'px';

            const revalidate = this._revalidateKeys;
            this._revalidateKeys = false;

            const prev = this.lastWindow;
            if (
                !revalidate &&
                prev &&
                prev.colStart === w.colStart && prev.colEnd === w.colEnd &&
                prev.rowStart === w.rowStart && prev.rowEnd === w.rowEnd
            ) {
                return; // window unchanged — nothing to mount or drop
            }
            this.lastWindow = w;

            // Unmount cells outside the new window. Snapshot the keys
            // first: `_unmount` mutates the map.
            for (const slot of Array.from(this.mounted.keys())) {
                const e = this.mounted.get(slot);
                if (
                    e.col < w.colStart || e.col > w.colEnd ||
                    e.row < w.rowStart || e.row > w.rowEnd
                ) {
                    this._unmount(slot);
                }
            }

            // Mount cells inside the window that aren't already there.
            // On a data-changed refresh, also remount any surviving cell
            // whose key no longer matches — same content-identity
            // contract as the 1-D virtualizer's keyed reuse.
            // An empty window is `colStart > colEnd` (Rust's
            // `GridWindow::EMPTY`), so these loops correctly do nothing
            // rather than mounting a phantom cell at the origin.
            for (let row = w.rowStart; row <= w.rowEnd; row++) {
                for (let col = w.colStart; col <= w.colEnd; col++) {
                    const slot = this._slot(col, row);
                    const existing = this.mounted.get(slot);
                    if (existing) {
                        if (!revalidate) continue;
                        if (this.cb.cellKey(col, row) === existing.key) continue;
                        this._unmount(slot);
                    }
                    this._mount(col, row, slot);
                }
            }
        }

        _mount(col, row, slot) {
            const res = this.cb.mountCell(col, row);
            if (!res) return;
            const [node, scopeId, x, y, cw, ch] = res;
            node.style.position = 'absolute';
            node.style.left = x + 'px';
            node.style.top = y + 'px';
            node.style.width = cw + 'px';
            node.style.height = ch + 'px';
            // `box-sizing: border-box` so an author's padding/border
            // stays inside the cell's allotted column width instead of
            // pushing it over its neighbour. The grid owns cell
            // geometry; the author owns what's drawn in it.
            node.style.boxSizing = 'border-box';
            this.spacer.appendChild(node);
            this.mounted.set(slot, {
                node,
                scopeId,
                col,
                row,
                key: this.cb.cellKey(col, row),
            });
        }

        _unmount(slot) {
            const e = this.mounted.get(slot);
            if (!e) return;
            if (e.node.parentNode === this.spacer) {
                this.spacer.removeChild(e.node);
            }
            this.cb.releaseCell(e.scopeId);
            this.mounted.delete(slot);
        }

        /** Scroll so cell `(col, row)` sits at the leading corner.
         *  The origin comes from Rust's live metrics via `cellOrigin`,
         *  so a column-width change since mount is reflected. */
        scrollToCell(col, row) {
            if (this._released) return;
            const o = this.cb.cellOrigin(col, row);
            if (!o) return;
            this.container.scrollLeft = o[0];
            this.container.scrollTop = o[1];
        }

        /** Called by Rust when counts or sizes change. Microtask-
         *  deferred for the same reason the constructor's initial fill
         *  is: the caller still holds `borrow_mut` on the backend. */
        dataChanged() {
            if (this._released) return;
            queueMicrotask(() => {
                if (this._released) return;
                this.refresh();
            });
        }

        /** Called by Rust from `release_virtual_grid`. Detaches every
         *  listener, unmounts everything, and flips `_released` so any
         *  queued handler short-circuits instead of calling back into
         *  Rust closures whose captured scopes may already be freed. */
        release() {
            if (this._releasedFully) return;
            this._releasedFully = true;
            this._released = true;
            this.container.removeEventListener('scroll', this._scrollHandler);
            if (this._containerObserver) {
                this._containerObserver.disconnect();
                this._containerObserver = null;
            }
            for (const slot of Array.from(this.mounted.keys())) {
                this._unmount(slot);
            }
            if (this.spacer && this.spacer.parentNode === this.container) {
                this.container.removeChild(this.spacer);
            }
        }
    }

    window.__idealystVirtualGrid = VirtualGrid;
})();
