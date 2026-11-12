// VirtualGrid — JS-side scroll handler + visible-rect diff for the web
// backend's `virtual_grid` primitive. Sibling of `virtualizer.js`, and
// owned by the same contract: this class owns the scroll listener so
// the wasm→JS boundary is crossed only when cells enter or leave
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
//  *                top:number,bottom:number,left:number,right:number,
//  *                colCount:number,rowCount:number,
//  *                contentW:number,contentH:number}} window
//  *   Ask Rust for the scrolling window (clipped to the scrolling band of
//  *   each axis), the frozen bands, and the content size at this offset.
//  * @property {(col: number, row: number) => number} cellKey
//  * @property {(col: number, row: number) => [number, number, number, number]} cellRect
//  * @property {(col: number, row: number) => [number, number]} scrollTarget
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
            // A one-cell grid the size of the content, so each frozen
            // layer can sit at a corner of it (`justify-self` / `align-self`
            // start or end) and stick from there. Scrolling cells are
            // absolutely positioned and take no part in the grid.
            this.spacer.style.display = 'grid';
            this.spacer.style.gridTemplateColumns = '100%';
            this.spacer.style.gridTemplateRows = '100%';
            container.appendChild(this.spacer);

            /** Sticky layers for frozen cells, by band pair (`colBand +
             *  rowBand`, each 'L' leading / 'S' scrolling / 'T' trailing).
             *  'SS' is the spacer itself. @type {Map<string, HTMLElement>} */
            this.layers = new Map();

            /** Cell reuse (`setRecycling`): parked cells by
             *  `reuseClass + '|' + layerKey`, and the Rust callbacks.
             *  @type {Map<string, Array<{node: Element, scopeId: number, col: number, row: number, layer: string, key: number}>>} */
            this.pool = new Map();
            this.recycle = null;

            /** A row kept mounted outside the window (`setKeptRow`), or -1. */
            this.keepRow = -1;

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

        /** Reuse cells from now on: a cell leaving the window is parked —
         *  node kept — and a cell entering takes a parked one of the same
         *  reuse class and layer, rebound to its new position
         *  (`rebind(scopeId, col, row)` writes the cell's position signal;
         *  the framework's flush runs as a microtask, before the browser
         *  paints) and re-placed. */
        setRecycling(reuseClass, rebind) {
            this.recycle = { reuseClass, rebind };
        }

        /** Keep row `row`'s cells mounted however far the grid scrolls
         *  from it (`-1`: none) — `VirtualGridHandle::keep_row_mounted`,
         *  the same rule as Rust's `PaneWindow::keeping_row`. A cell that
         *  leaves the window is parked and re-inserted elsewhere in the
         *  DOM, which drops its pointer capture: a drag that started in it
         *  would stop hearing the pointer. Re-windows on a microtask, as
         *  the first fill does: the caller is framework code, and the
         *  re-window mounts and releases cells, which re-enter it. */
        setKeptRow(row) {
            if (this._released || this.keepRow === row) return;
            this.keepRow = row;
            queueMicrotask(() => {
                if (this._released) return;
                this.lastWindow = null;
                this.update();
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
                prev.rowStart === w.rowStart && prev.rowEnd === w.rowEnd &&
                prev.top === w.top && prev.bottom === w.bottom &&
                prev.left === w.left && prev.right === w.right &&
                prev.colCount === w.colCount && prev.rowCount === w.rowCount
            ) {
                return; // window unchanged — nothing to mount or drop
            }
            this.lastWindow = w;

            // The mounted set is (rows) x (cols): each axis is its frozen
            // leading band, the scrolling window, and its frozen trailing
            // band. Rust clips the scrolling window to the scrolling band,
            // so no index is in two bands. An empty scrolling window is
            // `start > end` (Rust's `GridWindow::EMPTY`).
            const rows = this._axis(w.top, w.rowStart, w.rowEnd, w.bottom, w.rowCount);
            const kept = this.keepRow;
            if (kept >= 0 && kept < w.rowCount && !rows.includes(kept)) {
                rows.push(kept);
                rows.sort((a, b) => a - b);
            }
            const cols = this._axis(w.left, w.colStart, w.colEnd, w.right, w.colCount);
            const rowSet = new Set(rows);
            const colSet = new Set(cols);

            // Unmount cells outside the new set. Snapshot the keys
            // first: `_unmount` mutates the map. A cell whose band
            // changed (the frozen panes or a count moved) is remounted in
            // its new layer.
            for (const slot of Array.from(this.mounted.keys())) {
                const e = this.mounted.get(slot);
                if (
                    !colSet.has(e.col) || !rowSet.has(e.row) ||
                    e.layer !== this._layerKey(e.col, e.row, w)
                ) {
                    if (this.recycle) {
                        // Parked under the class of the position it LEFT —
                        // the structure it was built with.
                        this.mounted.delete(slot);
                        const k = this.recycle.reuseClass(e.col, e.row) + '|' + e.layer;
                        let list = this.pool.get(k);
                        if (!list) this.pool.set(k, (list = []));
                        list.push(e);
                    } else {
                        this._unmount(slot);
                    }
                }
            }

            // Mount cells inside the set that aren't already there. On a
            // data-changed refresh, also remount any surviving cell whose
            // key no longer matches — same content-identity contract as
            // the 1-D virtualizer's keyed reuse — and re-place the rest:
            // a width or height may have moved them.
            for (const row of rows) {
                for (const col of cols) {
                    const slot = this._slot(col, row);
                    const existing = this.mounted.get(slot);
                    if (existing) {
                        if (!revalidate) continue;
                        if (this.recycle) {
                            // A reusable cell follows its data reactively:
                            // a new key needs no rebuild, only a re-place.
                            existing.key = this.cb.cellKey(col, row);
                            const r = this.cb.cellRect(col, row);
                            if (r) this._place(existing.node, r[0], r[1], r[2], r[3], col, row, w);
                            continue;
                        }
                        if (this.cb.cellKey(col, row) === existing.key) {
                            const r = this.cb.cellRect(col, row);
                            if (r) this._place(existing.node, r[0], r[1], r[2], r[3], col, row, w);
                            continue;
                        }
                        this._unmount(slot);
                    }
                    if (this.recycle && this._reuse(col, row, slot, w)) continue;
                    this._mount(col, row, slot, w);
                }
            }
            // Parked cells nothing claimed: release them.
            for (const list of this.pool.values()) {
                for (const e of list) this._release(e);
            }
            this.pool.clear();
        }

        /** Take a parked cell of this position's class and layer, point it
         *  at `(col, row)` and place it there. False when none is parked. */
        _reuse(col, row, slot, w) {
            const layerKey = this._layerKey(col, row, w);
            const list = this.pool.get(this.recycle.reuseClass(col, row) + '|' + layerKey);
            if (!list || list.length === 0) return false;
            const e = list.pop();
            this.recycle.rebind(e.scopeId, col, row);
            const r = this.cb.cellRect(col, row);
            if (r) this._place(e.node, r[0], r[1], r[2], r[3], col, row, w);
            e.col = col;
            e.row = row;
            e.key = this.cb.cellKey(col, row);
            // Back into row-major order within its layer (hit-testing of
            // equal-z siblings follows DOM order).
            this._insert(e.node, this._layer(layerKey), layerKey, col, row);
            this.mounted.set(slot, e);
            return true;
        }

        /** Detach a cell and drop its framework scope. */
        _release(e) {
            if (e.node.parentNode) e.node.parentNode.removeChild(e.node);
            this.cb.releaseCell(e.scopeId);
        }

        /** One axis's mounted indices, ascending: `lead` frozen, the
         *  scrolling `start..=end`, `trail` frozen at the end of `count`. */
        _axis(lead, start, end, trail, count) {
            const out = [];
            for (let i = 0; i < lead; i++) out.push(i);
            for (let i = start; i <= end; i++) out.push(i);
            for (let i = count - trail; i < count; i++) out.push(i);
            return out;
        }

        /** 'L' / 'S' / 'T' for index `i` of an axis. */
        _band(i, lead, trail, count) {
            if (i < lead) return 'L';
            if (i >= count - trail) return 'T';
            return 'S';
        }

        _layerKey(col, row, w) {
            return this._band(col, w.left, w.right, w.colCount) +
                this._band(row, w.top, w.bottom, w.rowCount);
        }

        /** The layer a band pair's cells go in. A frozen layer is a
         *  zero-size `position: sticky` box at the matching corner of the
         *  content, so the browser holds its cells at the scrollport's
         *  edge (CSS sticky — `GridMetrics::display_origin`'s rule). Edge
         *  strips draw above scrolling cells, corners above both. */
        _layer(key) {
            if (key === 'SS') return this.spacer;
            let el = this.layers.get(key);
            if (el) return el;
            el = document.createElement('div');
            const [cb, rb] = key;
            el.style.position = 'sticky';
            el.style.width = '0px';
            el.style.height = '0px';
            el.style.gridArea = '1 / 1';
            el.style.justifySelf = cb === 'T' ? 'end' : 'start';
            el.style.alignSelf = rb === 'T' ? 'end' : 'start';
            if (cb === 'L') el.style.left = '0px';
            if (cb === 'T') el.style.right = '0px';
            if (rb === 'L') el.style.top = '0px';
            if (rb === 'T') el.style.bottom = '0px';
            el.style.zIndex = cb !== 'S' && rb !== 'S' ? '3' : '2';
            el.setAttribute('data-virtual-grid-layer', key);
            this.spacer.appendChild(el);
            this.layers.set(key, el);
            return el;
        }

        /** Position a cell node in its layer. A trailing layer sits at
         *  the content's far edge, so its cells are offset back from it. */
        _place(node, x, y, cw, ch, col, row, w) {
            const key = this._layerKey(col, row, w);
            const dx = key[0] === 'T' ? w.contentW : 0;
            const dy = key[1] === 'T' ? w.contentH : 0;
            node.style.left = (x - dx) + 'px';
            node.style.top = (y - dy) + 'px';
            node.style.width = cw + 'px';
            node.style.height = ch + 'px';
        }

        _mount(col, row, slot, w) {
            const res = this.cb.mountCell(col, row);
            if (!res) return;
            const [node, scopeId, x, y, cw, ch] = res;
            const layerKey = this._layerKey(col, row, w);
            const layer = this._layer(layerKey);
            node.style.position = 'absolute';
            this._place(node, x, y, cw, ch, col, row, w);
            // `box-sizing: border-box` so an author's padding/border
            // stays inside the cell's allotted column width instead of
            // pushing it over its neighbour. The grid owns cell
            // geometry; the author owns what's drawn in it.
            node.style.boxSizing = 'border-box';
            // Row-major DOM order — before the next mounted cell in
            // (row, col) order, else last — so equal-z cells stack and
            // hit-test by position, not by the order they scrolled in.
            this._insert(node, layer, layerKey, col, row);
            this.mounted.set(slot, {
                node,
                scopeId,
                col,
                row,
                layer: layerKey,
                key: this.cb.cellKey(col, row),
            });
        }

        /** Put `node` in `layer` at its row-major place. Within the cell's
         *  own layer only; frozen layers are appended to the spacer after
         *  the scrolling cells and keep their place (`next` is always a
         *  sibling of `node` in `layer`). */
        _insert(node, layer, layerKey, col, row) {
            let next = null;
            for (const e of this.mounted.values()) {
                if (e.layer !== layerKey || e.node === node) continue;
                const after = e.row > row || (e.row === row && e.col > col);
                const closer = next === null || e.row < next.row || (e.row === next.row && e.col < next.col);
                if (after && closer) next = e;
            }
            if (next) layer.insertBefore(node, next.node);
            else if (layer === this.spacer && this.layers.size > 0) {
                // Scrolling cells stay before the sticky layers.
                this.spacer.insertBefore(node, this.spacer.querySelector(':scope > [data-virtual-grid-layer]'));
            } else layer.appendChild(node);
        }

        _unmount(slot) {
            const e = this.mounted.get(slot);
            if (!e) return;
            if (e.node.parentNode) {
                e.node.parentNode.removeChild(e.node);
            }
            this.cb.releaseCell(e.scopeId);
            this.mounted.delete(slot);
        }

        /** Scroll so cell `(col, row)` sits at the scrolling area's
         *  leading corner, just past any frozen header and columns. The
         *  offset comes from Rust's live metrics via `scrollTarget`, so a
         *  column-width change since mount is reflected. */
        scrollToCell(col, row) {
            if (this._released) return;
            const o = this.cb.scrollTarget(col, row);
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
            for (const list of this.pool.values()) {
                for (const e of list) this._release(e);
            }
            this.pool.clear();
            this.layers.clear();
            if (this.spacer && this.spacer.parentNode === this.container) {
                this.container.removeChild(this.spacer);
            }
        }
    }

    window.__idealystVirtualGrid = VirtualGrid;
})();
