// VirtualGrid — JS-side scroll handler + visible-rect diff for the web
// backend's `virtual_grid` primitive. Sibling of `virtualizer.js`, and
// owned by the same contract: this class owns the scroll listener so
// the wasm→JS boundary is crossed only when cells enter or leave
// the window, never on every scroll tick.
//
// # Rows
//
// Each mounted row is one element holding that row's mounted cells, in
// column order, so a row is a real DOM container: `getByRole('row')` and
// `row.getByRole('cell')` (Playwright scopes by DOM descent, not
// `aria-owns`) and `filter({ hasText })` see every cell of it, the frozen
// ones included. Frozen panes are CSS sticky on the row and cell level:
//
// - a scrolling row is absolutely placed in the spacer at its `y`; a
//   frozen row sits in flow in the top or bottom BAND, a sticky strip at
//   that edge of the content (`GridMetrics::display_origin`'s rule);
// - in a row, a frozen LEADING column's cell is in flow at its own `x`
//   (the leading columns are contiguous from 0) and sticks at `left: x`;
//   a TRAILING one is in flow after the row's filler, at the far edge,
//   and sticks at `right`; a scrolling column's cell is absolutely placed
//   at its `x`.
//
// The browser holds frozen cells at the edge on the compositor, with no
// frame of lag; moving them from the scroll listener would trail.
//
// With `setTable(true)` (the grid's role is `table`) the rows carry
// `role=row` + `aria-rowindex`, the cells `role=columnheader` (frozen top
// rows) or `cell` + `aria-colindex`, and the container `aria-rowcount` /
// `aria-colcount` for the rows that are not mounted.
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

    /** The kept (held) row's `z-index`: over a frozen cell's 1, under the
     *  frozen bands' 3. */
    const KEPT_ROW_Z = '2';

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
            // A column the size of the content: the frozen top band, the
            // scrolling rows (absolutely placed, out of flow), a filler,
            // the frozen bottom band. The bands stick to the scrollport's
            // top and bottom edges.
            this.spacer.style.display = 'flex';
            this.spacer.style.flexDirection = 'column';
            container.appendChild(this.spacer);
            this.topBand = this._makeBand('L');
            this.filler = document.createElement('div');
            this.filler.style.flex = '1 1 auto';
            this.bottomBand = this._makeBand('T');
            this.spacer.appendChild(this.topBand);
            this.spacer.appendChild(this.filler);
            this.spacer.appendChild(this.bottomBand);

            /** Mounted rows by index: the row element, its band ('L' top
             *  frozen / 'S' scrolling / 'T' bottom frozen), its filler
             *  (between the scrolling and the trailing frozen cells) and
             *  its mounted cells by column.
             *  @type {Map<number, {el: HTMLElement, band: string, filler: HTMLElement, cells: Map<number, Object>}>} */
            this.rows = new Map();
            this.contentW = 0;
            /** The pending task that may turn sideways scrolling back
             *  on (`_fitSideways`), or 0. */
            this._widenTimer = 0;
            /** Table semantics (`setTable`). */
            this.table = false;

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

        /** Give the grid table semantics: rows and cells carry their
         *  roles and indices (see the file's header). Called by Rust right
         *  after construction, before the first fill. */
        setTable(on) {
            this.table = !!on;
            for (const b of [this.topBand, this.bottomBand]) {
                if (this.table) b.setAttribute('role', 'rowgroup');
                else b.removeAttribute('role');
            }
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
         *  re-window mounts and releases cells, which re-enter it.
         *
         *  The kept row is the one in the user's hand, so it also STACKS
         *  above the other scrolling rows (and under the frozen bands,
         *  which it passes beneath). On the row element, not its cells: a
         *  frozen cell's `z-index` is set inline by `_place`, so a cell's
         *  own class cannot lift it over another row's frozen cells, and
         *  a held row dragged down slid under them. */
        setKeptRow(row) {
            if (this._released || this.keepRow === row) return;
            const was = this.rows.get(this.keepRow);
            if (was) was.el.style.zIndex = '';
            this.keepRow = row;
            const now = this.rows.get(row);
            if (now && now.band === 'S') now.el.style.zIndex = KEPT_ROW_Z;
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
            if (w.contentW !== this.contentW) {
                this.contentW = w.contentW;
                for (const r of this.rows.values()) r.el.style.width = w.contentW + 'px';
            }
            this._fitSideways(w);
            if (this.table) {
                this.container.setAttribute('aria-rowcount', String(w.rowCount));
                this.container.setAttribute('aria-colcount', String(w.colCount));
            }

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
                        // the structure it was built with. Its node stays
                        // in the old row until it is reused or released.
                        this.mounted.delete(slot);
                        this._leaveRow(e);
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
            // Rows left with no mounted cell go.
            for (const [row, r] of Array.from(this.rows)) {
                if (r.cells.size === 0) this._dropRow(row);
            }
        }

        /** Take a parked cell of this position's class and layer, point it
         *  at `(col, row)` and place it there. False when none is parked. */
        _reuse(col, row, slot, w) {
            const layerKey = this._layerKey(col, row, w);
            const list = this.pool.get(this.recycle.reuseClass(col, row) + '|' + layerKey);
            if (!list || list.length === 0) return false;
            const e = list.pop();
            this.recycle.rebind(e.scopeId, col, row);
            e.col = col;
            e.row = row;
            e.key = this.cb.cellKey(col, row);
            const rowRec = this._rowFor(row, w);
            const r = this.cb.cellRect(col, row);
            if (r) this._place(e.node, r[0], r[1], r[2], r[3], col, row, w);
            // Into its new row, in column order.
            this._insert(e, rowRec, w);
            this._cellRole(e.node, col, row, w);
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

        /** A frozen row band: a sticky strip at the content's top ('L')
         *  or bottom ('T') edge, its rows in flow inside it, drawn above
         *  the scrolling rows. */
        _makeBand(key) {
            const el = document.createElement('div');
            el.style.position = 'sticky';
            el.style.flex = 'none';
            el.style.zIndex = '3';
            if (key === 'L') el.style.top = '0px';
            else el.style.bottom = '0px';
            el.setAttribute('data-virtual-grid-band', key);
            return el;
        }

        /** Row `row`'s record, created (and put in its band, in row
         *  order) if it has none. A row whose band changed since (the
         *  frozen counts moved) is rebuilt: its cells were all moved out
         *  by the window diff, which compares bands too. */
        _rowFor(row, w) {
            const band = this._band(row, w.top, w.bottom, w.rowCount);
            let r = this.rows.get(row);
            if (r && r.band === band) return r;
            if (r) this._dropRow(row);
            const el = document.createElement('div');
            el.style.display = 'flex';
            el.style.flexDirection = 'row';
            el.style.flex = 'none';
            el.style.width = this.contentW + 'px';
            if (band === 'S') {
                el.style.position = 'absolute';
                el.style.left = '0px';
                if (row === this.keepRow) el.style.zIndex = KEPT_ROW_Z;
            } else {
                el.style.position = 'relative';
            }
            el.setAttribute('data-virtual-grid-row', String(row));
            if (this.table) {
                el.setAttribute('role', 'row');
                el.setAttribute('aria-rowindex', String(row + 1));
            }
            const filler = document.createElement('div');
            filler.style.flex = '1 1 auto';
            filler.style.minWidth = '0px';
            el.appendChild(filler);
            r = { el, band, filler, cells: new Map() };
            // In row order among its band's rows: rows sit in DOM order,
            // the header first, for `getByRole('row').nth(i)`.
            let next = null;
            for (const [i, o] of this.rows) {
                if (o.band === band && i > row && (next === null || i < next[0])) next = [i, o];
            }
            const parent = band === 'L' ? this.topBand : band === 'T' ? this.bottomBand : this.spacer;
            const fallback = band === 'S' ? this.filler : null;
            parent.insertBefore(el, next ? next[1].el : fallback);
            this.rows.set(row, r);
            return r;
        }

        _dropRow(row) {
            const r = this.rows.get(row);
            if (!r) return;
            if (r.el.parentNode) r.el.parentNode.removeChild(r.el);
            this.rows.delete(row);
        }

        /** Take cell entry `e` out of its row's bookkeeping. */
        _leaveRow(e) {
            const r = this.rows.get(e.row);
            if (r && r.cells.get(e.col) === e) r.cells.delete(e.col);
        }

        /** Place a cell node in its row, and the row at the cell's `y`.
         *  A scrolling column's cell is absolutely placed at its `x`; a
         *  frozen one is in flow and sticks at that edge. */
        _place(node, x, y, cw, ch, col, row, w) {
            const r = this.rows.get(row);
            if (r) {
                if (r.band === 'S') r.el.style.top = y + 'px';
                r.el.style.height = ch + 'px';
            }
            node.style.width = cw + 'px';
            node.style.height = ch + 'px';
            node.style.flex = 'none';
            const band = this._band(col, w.left, w.right, w.colCount);
            if (band === 'S') {
                node.style.position = 'absolute';
                node.style.left = x + 'px';
                node.style.top = '0px';
                node.style.right = '';
                node.style.zIndex = '';
            } else {
                // Above the row's scrolling cells, which pass beneath it.
                node.style.position = 'sticky';
                node.style.top = '';
                node.style.zIndex = '1';
                if (band === 'L') {
                    node.style.left = x + 'px';
                    node.style.right = '';
                } else {
                    node.style.left = '';
                    node.style.right = (w.contentW - x - cw) + 'px';
                }
            }
        }

        /** Table semantics for a cell (`setTable`). */
        _cellRole(node, col, row, w) {
            if (!this.table) return;
            node.setAttribute('role', row < w.top ? 'columnheader' : 'cell');
            node.setAttribute('aria-colindex', String(col + 1));
        }

        _mount(col, row, slot, w) {
            const res = this.cb.mountCell(col, row);
            if (!res) return;
            const [node, scopeId, x, y, cw, ch] = res;
            const layerKey = this._layerKey(col, row, w);
            const rowRec = this._rowFor(row, w);
            this._place(node, x, y, cw, ch, col, row, w);
            // `box-sizing: border-box` so an author's padding/border
            // stays inside the cell's allotted column width instead of
            // pushing it over its neighbour. The grid owns cell
            // geometry; the author owns what's drawn in it.
            node.style.boxSizing = 'border-box';
            const e = {
                node,
                scopeId,
                col,
                row,
                layer: layerKey,
                key: this.cb.cellKey(col, row),
            };
            this._insert(e, rowRec, w);
            this._cellRole(node, col, row, w);
            this.mounted.set(slot, e);
        }

        /** Put cell entry `e` in row `r` at its column-order place: before
         *  the next mounted cell of a higher column, and the leading and
         *  scrolling cells before the row's filler, the trailing ones
         *  after it (so they sit in flow at the far edge). */
        _insert(e, r, w) {
            let next = null;
            for (const [c, o] of r.cells) {
                if (o === e || c <= e.col) continue;
                if (next === null || c < next.col) next = o;
            }
            const trailing = (c) => this._band(c, w.left, w.right, w.colCount) === 'T';
            let ref = next ? next.node : null;
            if (!trailing(e.col) && (ref === null || trailing(next.col))) ref = r.filler;
            r.el.insertBefore(e.node, ref);
            e.layer = this._layerKey(e.col, e.row, w);
            r.cells.set(e.col, e);
        }

        _unmount(slot) {
            const e = this.mounted.get(slot);
            if (!e) return;
            if (e.node.parentNode) {
                e.node.parentNode.removeChild(e.node);
            }
            this.cb.releaseCell(e.scopeId);
            this.mounted.delete(slot);
            this._leaveRow(e);
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

        /** Stop the grid scrolling sideways while its columns fit.
         *
         *  A grid whose columns share the scrollport's width (a table
         *  with a filling column) is sized from a MEASURED width: when
         *  its box narrows, the first layout still has the content at
         *  the width before, and with `overflow-x: auto` that layout
         *  carries a horizontal scrollbar until the resize observer has
         *  re-sized the columns. Animate the box and the bar shows on
         *  every step. So while the content fits, the axis is `hidden`:
         *  a layout that runs ahead of the columns has no bar to show.
         *
         *  Turning the axis back ON waits for a task. A resize is seen
         *  here before the grid's owner has re-sized its columns, so
         *  "does not fit" is true of every narrowing for a moment; by
         *  the task both have run, and only content that still does not
         *  fit gets its bar. */
        _fitSideways(w) {
            const c = this.container;
            const fits = this._fitsSideways(w);
            const hidden = c.style.overflowX === 'hidden';
            if (fits) {
                if (!hidden) c.style.overflowX = 'hidden';
            } else if (hidden && !this._widenTimer) {
                this._widenTimer = setTimeout(() => {
                    this._widenTimer = 0;
                    if (this._released) return;
                    const now = this.cb.window(c.scrollLeft, c.scrollTop, c.clientWidth, c.clientHeight);
                    if (now && !this._fitsSideways(now)) c.style.overflowX = 'auto';
                }, 0);
            }
        }

        /** Whether `w`'s content is no wider than the scrollport. Half
         *  a pixel of slack: the widths are sums of floats, and the
         *  browser itself scrolls nothing under a pixel. */
        _fitsSideways(w) {
            const c = this.container;
            const SLACK = 0.5;
            if (w.contentW <= c.clientWidth + SLACK) return true;
            // Two classic bars can each be there only because of the
            // other: the content fits the box without them, and each
            // bar's thickness is what pushes the other axis over. The
            // browser's own `auto` shows neither; say the same, or the
            // pair would stay once shown. Only looked at when the
            // content is within a bar of fitting.
            if (w.contentW > c.offsetWidth + SLACK) return false;
            const cs = getComputedStyle(c);
            const px = (v) => parseFloat(v) || 0;
            const barV = c.offsetWidth - c.clientWidth - px(cs.borderLeftWidth) - px(cs.borderRightWidth);
            const barH = c.offsetHeight - c.clientHeight - px(cs.borderTopWidth) - px(cs.borderBottomWidth);
            return barV > 0 && barH > 0
                && w.contentW <= c.clientWidth + barV + SLACK
                && w.contentH <= c.clientHeight + barH + SLACK;
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
            if (this._widenTimer) clearTimeout(this._widenTimer);
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
            this.rows.clear();
            if (this.spacer && this.spacer.parentNode === this.container) {
                this.container.removeChild(this.spacer);
            }
        }
    }

    window.__idealystVirtualGrid = VirtualGrid;
})();
