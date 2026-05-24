//! ASCII frame compositor. Walks the laid-out node tree and writes
//! into a 2D grid of [`Cell`]s. The host turns the grid into ANSI-
//! escaped bytes and dumps it to stdout.

use runtime_core::color::Rgba;
use runtime_core::{GradientKind, Length, RadialExtent};

use crate::node::{NodeData, NodeKind, ResolvedGradient};
use crate::TerminalBackend;

/// Resolve `Length::Px` / `Length::Percent` / `Length::Auto` against
/// `basis` (the node's laid-out size on the matching axis). Used to
/// realise static `transform: [translate(...)]` whose percent values
/// reference the node's OWN size.
pub(crate) fn resolve_length_against(l: &Length, basis: f32) -> f32 {
    match l {
        Length::Px(v) => *v,
        Length::Percent(v) => basis * v / 100.0,
        Length::Auto => 0.0,
    }
}

/// One terminal cell. `glyph` is the visible char; `fg` / `bg` are
/// optional foreground / background colors (None = terminal default).
#[derive(Copy, Clone, Debug, PartialEq)]
pub struct Cell {
    pub glyph: char,
    pub fg: Option<Rgba>,
    pub bg: Option<Rgba>,
}

impl Default for Cell {
    fn default() -> Self {
        Self {
            glyph: ' ',
            fg: None,
            bg: None,
        }
    }
}

/// Row-major 2D grid of cells. `cells[row * cols + col]`.
pub struct Grid {
    pub cols: u16,
    pub rows: u16,
    pub cells: Vec<Cell>,
}

impl Grid {
    pub fn new(cols: u16, rows: u16) -> Self {
        Self {
            cols,
            rows,
            cells: vec![Cell::default(); cols as usize * rows as usize],
        }
    }

    pub fn cell_mut(&mut self, col: u16, row: u16) -> Option<&mut Cell> {
        if col >= self.cols || row >= self.rows {
            return None;
        }
        let idx = row as usize * self.cols as usize + col as usize;
        self.cells.get_mut(idx)
    }

    pub fn cell(&self, col: u16, row: u16) -> Option<&Cell> {
        if col >= self.cols || row >= self.rows {
            return None;
        }
        let idx = row as usize * self.cols as usize + col as usize;
        self.cells.get(idx)
    }
}

impl TerminalBackend {
    /// Run flex layout and compose the result into a fresh
    /// [`Grid`]. Called by the host once per frame.
    pub fn render_to_grid(&mut self) -> Grid {
        let (cols, rows) = self.viewport;
        let (cw, ch) = self.cell_size;
        let mut grid = Grid::new(cols, rows);

        let Some(root_id) = self.find_root() else { return grid };
        let root_layout = match self.nodes.get(&root_id) {
            Some(d) => d.layout,
            None => return grid,
        };
        // Taffy operates in layout px. Tell it the viewport is the
        // cell count multiplied by the per-cell px factor, then we'll
        // divide frame coords by the same factor at paint time to
        // land back in cells.
        self.layout.compute(root_layout, cols as f32 * cw, rows as f32 * ch);

        self.paint_node(root_id, 0.0, 0.0, 1.0, &mut grid);
        grid
    }

    fn paint_node(
        &self,
        id: u32,
        parent_x: f32,
        parent_y: f32,
        parent_opacity: f32,
        grid: &mut Grid,
    ) {
        let Some(data) = self.nodes.get(&id) else { return };
        // Effective opacity composes multiplicatively down the tree —
        // a vignette wrapper at `opacity: 0.0` hides every band
        // beneath it without each band needing its own zero.
        // `animated_opacity` wins over the static slot when present
        // (mirrors `animated_bg` vs `bg`).
        let own_opacity = data.animated_opacity.unwrap_or(data.opacity);
        let effective_opacity = parent_opacity * own_opacity;
        if effective_opacity <= 0.0 {
            return;
        }
        let frame = self.layout.frame_of(data.layout);
        let (cw, ch) = self.cell_size;
        // Static `transform: [translate(...)]` resolves against the
        // node's own laid-out size (`Length::Percent` semantics).
        // The animation-driven translate composes additively on top.
        let static_tx = data
            .static_translate_x
            .as_ref()
            .map(|l| resolve_length_against(l, frame.width))
            .unwrap_or(0.0);
        let static_ty = data
            .static_translate_y
            .as_ref()
            .map(|l| resolve_length_against(l, frame.height))
            .unwrap_or(0.0);
        let total_tx = data.translate_x + static_tx;
        let total_ty = data.translate_y + static_ty;
        // Convert frame + translate from layout px to cell space.
        // `parent_x` / `parent_y` are already in cells (paint recurses
        // through `paint_node` with cell-space coords).
        let x = parent_x + (frame.x + total_tx) / cw;
        let y = parent_y + (frame.y + total_ty) / ch;
        let w = frame.width / cw;
        let h = frame.height / ch;
        // Inline `cell_size` into a local alias the rest of the
        // method can keep referencing — used for gradient sampling
        // which has to operate in layout-px space to keep radial
        // shapes round despite the cell aspect ratio.
        let _ = cw;
        let _ = ch;
        if w <= 0.0 || h <= 0.0 {
            return;
        }

        // Animated bg/fg override the static style colors when set.
        let effective_bg = data.animated_bg.or(data.bg);
        let effective_fg = data.animated_fg.or(data.fg);

        // 1. Paint the background. A `background_gradient` wins over
        //    `background` — same precedence as iOS / web. Gradient
        //    sampling reads in layout-px space so radial shapes
        //    stay circular even when the cell aspect ratio (~2:1
        //    height-to-width) would otherwise squash them.
        if let Some(gradient) = data.gradient.as_ref() {
            paint_gradient(grid, x, y, w, h, effective_opacity, gradient, (cw, ch));
        } else if let Some(mut bg) = effective_bg {
            bg.a = ((bg.a as f32) * effective_opacity).round() as u8;
            if bg.a > 0 {
                paint_rect_bg(grid, x, y, w, h, bg);
            }
        }

        // 2. Paint a 1-cell border if the style declares any non-zero
        // border width. We collapse all four sides into one simple
        // box-drawing border for the ASCII medium — finer per-side
        // control isn't useful at character resolution.
        if border_requested(data) {
            let mut color = effective_fg.unwrap_or(Rgba::new(180, 180, 180, 255));
            color.a = ((color.a as f32) * effective_opacity).round() as u8;
            paint_border(grid, x, y, w, h, color, effective_bg);
        }

        // 3. Paint content. Views and Pressables don't carry content
        // directly — only their children do.
        match data.kind {
            NodeKind::Text | NodeKind::Button => {
                let mut fg = effective_fg.unwrap_or(default_fg(data));
                fg.a = ((fg.a as f32) * effective_opacity).round() as u8;
                paint_text(grid, &data.content, x, y, w, h, fg, effective_bg);
            }
            NodeKind::Toggle => {
                let fg = effective_fg.unwrap_or(Rgba::new(220, 220, 220, 255));
                let on_color = Rgba::new(127, 232, 214, 255);
                let glyph = if data.toggle_value { '●' } else { ' ' };
                let label = format!("[{}{}{}]", ' ', glyph, ' ');
                let color = if data.toggle_value { on_color } else { fg };
                paint_text(grid, &label, x, y, w, h, color, effective_bg);
            }
            NodeKind::TextInput => {
                let focused = self.focused_id == Some(id);
                let fg = effective_fg.unwrap_or(Rgba::new(220, 220, 220, 255));
                let placeholder_fg = Rgba::new(110, 116, 134, 255);
                let cursor_color = Rgba::new(255, 210, 139, 255);
                if let Some(input) = data.input.as_ref() {
                    // Choose what to display.
                    let (display, color, is_placeholder) = if input.value.is_empty()
                    {
                        (
                            input.placeholder.clone().unwrap_or_default(),
                            placeholder_fg,
                            true,
                        )
                    } else {
                        (input.value.clone(), fg, false)
                    };
                    paint_text(grid, &display, x, y, w, h, color, effective_bg);
                    // Cursor. Only draw when focused. We always paint
                    // it at `cursor` cells from the left edge — for
                    // inputs longer than the visible width this is a
                    // simplification: a real implementation would
                    // scroll. The intrinsic-size we set on creation
                    // means most demo cases stay on-screen.
                    if focused {
                        let cursor_col = (x + input.cursor as f32).floor() as i32;
                        let cursor_row = y.floor() as i32;
                        if let (Ok(c), Ok(r)) =
                            (u16::try_from(cursor_col), u16::try_from(cursor_row))
                        {
                            if let Some(cell) = grid.cell_mut(c, r) {
                                // Cursor cell: swap bg for cursor color
                                // and recolor the glyph on top.
                                cell.bg = Some(cursor_color);
                                // If we drew over a placeholder, blank
                                // the glyph so the cursor sits on an
                                // empty cell. Otherwise keep the glyph
                                // visible (caret-on-char style).
                                if is_placeholder {
                                    cell.glyph = ' ';
                                }
                                // Force the glyph's fg to readable
                                // contrast against the warm cursor.
                                cell.fg = Some(Rgba::new(10, 12, 17, 255));
                            }
                        }
                    }
                }
            }
            NodeKind::ActivityIndicator => {
                // Braille spinner — 10-step cycle. `anim_phase` is
                // bumped once per rAF tick, so the loop advances at
                // the host's `target_fps`.
                let frames = [
                    '⠋', '⠙', '⠹', '⠸', '⠼', '⠴', '⠦', '⠧', '⠇', '⠏',
                ];
                let idx = (data.anim_phase as usize) % frames.len();
                let mut s = String::new();
                s.push(frames[idx]);
                let fg = effective_fg.unwrap_or(Rgba::new(127, 232, 214, 255));
                // Center horizontally within the node's frame.
                let cx = x + (w - 1.0) / 2.0;
                paint_text(grid, &s, cx.floor(), y, 1.0, 1.0, fg, effective_bg);
            }
            NodeKind::View | NodeKind::Pressable => {}
        }

        // 4. Recurse into children. Children paint OVER the parent's
        // background; siblings with higher `z_index` paint over
        // siblings with lower. Tree-insertion order is the
        // tiebreaker (matches every other backend's "siblings later
        // in the tree win when z-index is equal" posture).
        for cid in self.children_in_z_order(&data.children) {
            self.paint_node(cid, x, y, effective_opacity, grid);
        }
    }
}

impl TerminalBackend {
    /// Order a list of child ids by `(z_index ASC, original_index ASC)`.
    /// Stable sort over `original_index` keeps siblings with equal
    /// z in their tree order. Used by both the paint walker and the
    /// hit-tester — clicks should land on whatever paints visually
    /// on top.
    pub(crate) fn children_in_z_order(&self, children: &[u32]) -> Vec<u32> {
        let mut paired: Vec<(usize, u32, f32)> = children
            .iter()
            .enumerate()
            .map(|(idx, &id)| {
                let z = self.nodes.get(&id).map(|d| d.z_index).unwrap_or(0.0);
                (idx, id, z)
            })
            .collect();
        // Sort by z ascending — lowest paints first (back).
        paired.sort_by(|a, b| a.2.partial_cmp(&b.2).unwrap_or(std::cmp::Ordering::Equal));
        paired.into_iter().map(|(_, id, _)| id).collect()
    }
}

/// Default foreground color for a text-bearing node when the style
/// didn't set one. Buttons get a brighter shade so they stand out
/// from plain Text by default.
fn default_fg(data: &NodeData) -> Rgba {
    match data.kind {
        NodeKind::Button => Rgba::new(255, 255, 255, 255),
        _ => Rgba::new(220, 220, 220, 255),
    }
}

fn border_requested(data: &NodeData) -> bool {
    let Some(style) = &data.style else { return false };
    let read = |t: &Option<runtime_core::Tokenized<f32>>| -> f32 {
        t.as_ref().map(|t| *t.value()).unwrap_or(0.0)
    };
    read(&style.border_top_width) > 0.0
        || read(&style.border_right_width) > 0.0
        || read(&style.border_bottom_width) > 0.0
        || read(&style.border_left_width) > 0.0
}

/// Per-cell gradient sampler. Reads the node's frame in cells and
/// the active `cell_size` (px-per-cell, per-axis) so radial math
/// runs in layout-px space — that keeps a `Radial { ClosestSide }`
/// disc circular even though the terminal's cells are roughly 2:1
/// taller than wide.
fn paint_gradient(
    grid: &mut Grid,
    x: f32,
    y: f32,
    w: f32,
    h: f32,
    opacity: f32,
    gradient: &ResolvedGradient,
    cell_size: (f32, f32),
) {
    let (cw, ch) = cell_size;
    // Frame in layout-px space (matches GradientKind::Radial's
    // `center` and `extent` conventions, which the framework speaks).
    let frame_w_px = w * cw;
    let frame_h_px = h * ch;
    if frame_w_px <= 0.0 || frame_h_px <= 0.0 || gradient.stops.is_empty() {
        return;
    }

    // Build an effective stop array applying any animated overrides.
    // We materialise once before the per-cell loop so the inner loop
    // stays branch-free on the override path.
    let effective_stops: Vec<(f32, Rgba)> = gradient
        .stops
        .iter()
        .zip(gradient.animated_stops.iter())
        .map(|((off, base), ov)| (*off, ov.unwrap_or(*base)))
        .collect();

    let x0 = x.max(0.0).floor() as i32;
    let y0 = y.max(0.0).floor() as i32;
    let x1 = (x + w).ceil() as i32;
    let y1 = (y + h).ceil() as i32;

    match &gradient.kind {
        GradientKind::Radial {
            center,
            radius,
            extent,
        } => {
            let cx_px = center.0 * frame_w_px;
            let cy_px = center.1 * frame_h_px;
            let ref_dist = match extent {
                RadialExtent::ClosestSide => 0.5 * frame_w_px.min(frame_h_px),
                RadialExtent::FarthestCorner => {
                    let dx = cx_px.max(frame_w_px - cx_px);
                    let dy = cy_px.max(frame_h_px - cy_px);
                    (dx * dx + dy * dy).sqrt()
                }
            };
            let max_r = (ref_dist * radius).max(0.001);
            for row in y0..y1 {
                for col in x0..x1 {
                    // Cell center in layout-px, relative to the node's frame.
                    let local_x_px = (col as f32 + 0.5 - x) * cw - cx_px;
                    let local_y_px = (row as f32 + 0.5 - y) * ch - cy_px;
                    let d = (local_x_px * local_x_px + local_y_px * local_y_px).sqrt();
                    let t = (d / max_r).clamp(0.0, 1.0);
                    let color = sample_stops(&effective_stops, t, opacity);
                    write_cell_bg(grid, col, row, color);
                }
            }
        }
        GradientKind::Linear { angle_deg } => {
            // CSS convention: 0° = bottom→top, 90° = left→right,
            // 180° = top→bottom, 270° = right→left.
            let rad = angle_deg.to_radians();
            let dir_x = rad.sin();
            let dir_y = -rad.cos();
            // Project the frame's corners onto the gradient axis to
            // get the axis range (in layout-px).
            let corners_px = [
                (0.0, 0.0),
                (frame_w_px, 0.0),
                (0.0, frame_h_px),
                (frame_w_px, frame_h_px),
            ];
            let projected: Vec<f32> = corners_px
                .iter()
                .map(|(px, py)| px * dir_x + py * dir_y)
                .collect();
            let min_p = projected.iter().copied().fold(f32::INFINITY, f32::min);
            let max_p = projected
                .iter()
                .copied()
                .fold(f32::NEG_INFINITY, f32::max);
            let range = (max_p - min_p).max(0.001);
            for row in y0..y1 {
                for col in x0..x1 {
                    let local_x_px = (col as f32 + 0.5 - x) * cw;
                    let local_y_px = (row as f32 + 0.5 - y) * ch;
                    let p = local_x_px * dir_x + local_y_px * dir_y;
                    let t = ((p - min_p) / range).clamp(0.0, 1.0);
                    let color = sample_stops(&effective_stops, t, opacity);
                    write_cell_bg(grid, col, row, color);
                }
            }
        }
    }
}

/// Lerp two adjacent stops at parameter `t`. Multiplies the result's
/// alpha by `opacity` so the node-level opacity composes through
/// gradient stops as well as solid fills.
fn sample_stops(stops: &[(f32, Rgba)], t: f32, opacity: f32) -> Rgba {
    // Stops are author-ordered ascending by offset (the framework's
    // contract). Find the bracket.
    let last = stops.len() - 1;
    if t <= stops[0].0 {
        return apply_opacity(stops[0].1, opacity);
    }
    if t >= stops[last].0 {
        return apply_opacity(stops[last].1, opacity);
    }
    for win in stops.windows(2) {
        let (a_off, a_col) = win[0];
        let (b_off, b_col) = win[1];
        if t >= a_off && t <= b_off {
            let span = (b_off - a_off).max(0.0001);
            let u = (t - a_off) / span;
            let blended = Rgba {
                r: lerp_u8(a_col.r, b_col.r, u),
                g: lerp_u8(a_col.g, b_col.g, u),
                b: lerp_u8(a_col.b, b_col.b, u),
                a: lerp_u8(a_col.a, b_col.a, u),
            };
            return apply_opacity(blended, opacity);
        }
    }
    apply_opacity(stops[last].1, opacity)
}

fn lerp_u8(a: u8, b: u8, t: f32) -> u8 {
    let a = a as f32;
    let b = b as f32;
    (a + (b - a) * t).round().clamp(0.0, 255.0) as u8
}

fn apply_opacity(c: Rgba, opacity: f32) -> Rgba {
    Rgba {
        r: c.r,
        g: c.g,
        b: c.b,
        a: ((c.a as f32) * opacity).round().clamp(0.0, 255.0) as u8,
    }
}

/// Write a gradient cell's bg. Skips fully-transparent samples so
/// the underlying paint shows through (vignettes, planet halos).
///
/// When the sample is sufficiently opaque (≥ ~50% alpha), also
/// blank the cell's glyph + fg. This is what makes an "in front"
/// planet actually hide text underneath it — without it, the
/// planet's bg would render but the text glyph would still poke
/// through as a colored character. Below the threshold the glyph
/// stays (halo regions, vignette edges where text should remain
/// legible).
fn write_cell_bg(grid: &mut Grid, col: i32, row: i32, color: Rgba) {
    if color.a == 0 {
        return;
    }
    if let (Ok(c), Ok(r)) = (u16::try_from(col), u16::try_from(row)) {
        if let Some(cell) = grid.cell_mut(c, r) {
            // Alpha-composite against whatever's underneath. Cheap
            // sRGB-space blend — perceptually fine for ASCII.
            let prev = cell.bg.unwrap_or(Rgba::BLACK);
            let a = color.a as f32 / 255.0;
            let inv = 1.0 - a;
            cell.bg = Some(Rgba {
                r: (color.r as f32 * a + prev.r as f32 * inv).round() as u8,
                g: (color.g as f32 * a + prev.g as f32 * inv).round() as u8,
                b: (color.b as f32 * a + prev.b as f32 * inv).round() as u8,
                a: 255,
            });
            if color.a >= GLYPH_HIDE_ALPHA {
                cell.glyph = ' ';
                cell.fg = None;
            }
        }
    }
}

/// Alpha threshold above which a solid bg / gradient sample clears
/// the underlying glyph. This is what makes an "in front" sibling
/// (higher z-index) hide text behind it; below the threshold the
/// glyph survives so halos / vignettes / soft overlays remain
/// readable.
const GLYPH_HIDE_ALPHA: u8 = 128;

fn paint_rect_bg(grid: &mut Grid, x: f32, y: f32, w: f32, h: f32, bg: Rgba) {
    let x0 = x.max(0.0).floor() as i32;
    let y0 = y.max(0.0).floor() as i32;
    let x1 = (x + w).ceil() as i32;
    let y1 = (y + h).ceil() as i32;
    for row in y0..y1 {
        for col in x0..x1 {
            if let (Ok(c), Ok(r)) = (u16::try_from(col), u16::try_from(row)) {
                if let Some(cell) = grid.cell_mut(c, r) {
                    cell.bg = Some(bg);
                    if bg.a >= GLYPH_HIDE_ALPHA {
                        cell.glyph = ' ';
                        cell.fg = None;
                    }
                }
            }
        }
    }
}

fn paint_border(
    grid: &mut Grid,
    x: f32,
    y: f32,
    w: f32,
    h: f32,
    fg: Rgba,
    bg: Option<Rgba>,
) {
    let x0 = x.floor() as i32;
    let y0 = y.floor() as i32;
    let x1 = (x + w).ceil() as i32 - 1;
    let y1 = (y + h).ceil() as i32 - 1;
    if x1 <= x0 || y1 <= y0 {
        return;
    }

    let put = |grid: &mut Grid, col: i32, row: i32, ch: char| {
        if let (Ok(c), Ok(r)) = (u16::try_from(col), u16::try_from(row)) {
            if let Some(cell) = grid.cell_mut(c, r) {
                cell.glyph = ch;
                cell.fg = Some(fg);
                if let Some(b) = bg {
                    cell.bg = Some(b);
                }
            }
        }
    };

    // Horizontal edges
    for col in (x0 + 1)..x1 {
        put(grid, col, y0, '─');
        put(grid, col, y1, '─');
    }
    // Vertical edges
    for row in (y0 + 1)..y1 {
        put(grid, x0, row, '│');
        put(grid, x1, row, '│');
    }
    // Corners
    put(grid, x0, y0, '╭');
    put(grid, x1, y0, '╮');
    put(grid, x0, y1, '╰');
    put(grid, x1, y1, '╯');
}

/// Lay out `content` inside the rect `(x, y, w, h)`, wrapping at
/// whitespace. Honors `\n`. Truncates if more lines than `h` are
/// produced. Writes glyph + fg + bg into the matching cells.
fn paint_text(
    grid: &mut Grid,
    content: &str,
    x: f32,
    y: f32,
    w: f32,
    h: f32,
    fg: Rgba,
    bg: Option<Rgba>,
) {
    let x0 = x.floor() as i32;
    let y0 = y.floor() as i32;
    let max_cols = w.floor() as i32;
    let max_rows = h.ceil() as i32;
    if max_cols <= 0 || max_rows <= 0 {
        return;
    }

    let mut lines: Vec<String> = Vec::new();
    for paragraph in content.split('\n') {
        let words: Vec<&str> = paragraph.split_whitespace().collect();
        if words.is_empty() {
            lines.push(String::new());
            continue;
        }
        let mut line = String::new();
        for word in words {
            let wlen = word.chars().count() as i32;
            if line.is_empty() {
                if wlen > max_cols {
                    // Hard-break a too-long word.
                    let mut start = 0;
                    let chars: Vec<char> = word.chars().collect();
                    while start < chars.len() {
                        let end = (start + max_cols as usize).min(chars.len());
                        lines.push(chars[start..end].iter().collect());
                        start = end;
                    }
                } else {
                    line.push_str(word);
                }
            } else if line.chars().count() as i32 + 1 + wlen > max_cols {
                lines.push(std::mem::take(&mut line));
                line.push_str(word);
            } else {
                line.push(' ');
                line.push_str(word);
            }
        }
        if !line.is_empty() {
            lines.push(line);
        }
    }

    for (row_idx, line) in lines.iter().take(max_rows as usize).enumerate() {
        let row = y0 + row_idx as i32;
        for (col_idx, ch) in line.chars().take(max_cols as usize).enumerate() {
            let col = x0 + col_idx as i32;
            if let (Ok(c), Ok(r)) = (u16::try_from(col), u16::try_from(row)) {
                if let Some(cell) = grid.cell_mut(c, r) {
                    cell.glyph = ch;
                    cell.fg = Some(fg);
                    if let Some(b) = bg {
                        cell.bg = Some(b);
                    }
                }
            }
        }
    }
}
