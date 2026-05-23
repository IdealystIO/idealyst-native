//! ASCII frame compositor. Walks the laid-out node tree and writes
//! into a 2D grid of [`Cell`]s. The host turns the grid into ANSI-
//! escaped bytes and dumps it to stdout.

use framework_core::color::Rgba;

use crate::node::{NodeData, NodeKind};
use crate::TerminalBackend;

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
        let mut grid = Grid::new(cols, rows);

        let Some(root_id) = self.find_root() else { return grid };
        let root_layout = match self.nodes.get(&root_id) {
            Some(d) => d.layout,
            None => return grid,
        };
        self.layout.compute(root_layout, cols as f32, rows as f32);

        self.paint_node(root_id, 0.0, 0.0, &mut grid);
        grid
    }

    fn paint_node(&self, id: u32, parent_x: f32, parent_y: f32, grid: &mut Grid) {
        let Some(data) = self.nodes.get(&id) else { return };
        if data.opacity <= 0.0 {
            // Fully transparent — skip the subtree entirely. Children
            // inherit opacity multiplicatively, so 0 here = 0 everywhere
            // below.
            return;
        }
        let frame = self.layout.frame_of(data.layout);
        // Animation-time translates ride on top of the laid-out frame.
        let x = parent_x + frame.x + data.translate_x;
        let y = parent_y + frame.y + data.translate_y;
        let w = frame.width;
        let h = frame.height;
        if w <= 0.0 || h <= 0.0 {
            return;
        }

        // Animated bg/fg override the static style colors when set.
        let effective_bg = data.animated_bg.or(data.bg);
        let effective_fg = data.animated_fg.or(data.fg);

        // 1. Paint the background fill across this node's frame.
        if let Some(mut bg) = effective_bg {
            // Multiply the bg's alpha by opacity. Since terminal
            // cells can't actually composite transparent colors, we
            // skip the paint entirely when alpha drops to zero and
            // let whatever was painted underneath show through.
            bg.a = ((bg.a as f32) * data.opacity).round() as u8;
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
            color.a = ((color.a as f32) * data.opacity).round() as u8;
            paint_border(grid, x, y, w, h, color, effective_bg);
        }

        // 3. Paint content. Views and Pressables don't carry content
        // directly — only their children do.
        match data.kind {
            NodeKind::Text | NodeKind::Button => {
                let mut fg = effective_fg.unwrap_or(default_fg(data));
                fg.a = ((fg.a as f32) * data.opacity).round() as u8;
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
        // background, so order matters.
        for &cid in &data.children {
            self.paint_node(cid, x, y, grid);
        }
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
    let read = |t: &Option<framework_core::Tokenized<f32>>| -> f32 {
        t.as_ref().map(|t| *t.value()).unwrap_or(0.0)
    };
    read(&style.border_top_width) > 0.0
        || read(&style.border_right_width) > 0.0
        || read(&style.border_bottom_width) > 0.0
        || read(&style.border_left_width) > 0.0
}

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
