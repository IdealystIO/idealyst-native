//! CommonMark/GFM source → [`MarkdownDoc`] block list.
//!
//! A small state machine over `pulldown-cmark`'s event stream. It
//! flattens the document into the renderer's one-node-per-block shape:
//! inline emphasis/strong/code/link/strike become per-run flags;
//! nested lists expand into sibling [`MdListItem`]s with a larger
//! `depth`; block quotes and list items flatten their inner paragraphs
//! into a single line of runs.
//!
//! Deliberately out of scope for v1 (documented in the README): images,
//! tables, task-list checkboxes, footnotes, and syntax highlighting of
//! code-block bodies. These either parse to nothing or degrade to plain
//! text rather than panicking.

use pulldown_cmark::{
    CodeBlockKind, Event, HeadingLevel, Options, Parser, Tag, TagEnd,
};

use crate::ir::{MarkdownDoc, MdBlock, MdListItem, MdRun, MdTheme};

/// Parse `source` into a resolved [`MarkdownDoc`] painted with `theme`.
pub fn parse(source: &str, theme: MdTheme) -> MarkdownDoc {
    let mut opts = Options::empty();
    opts.insert(Options::ENABLE_STRIKETHROUGH);
    opts.insert(Options::ENABLE_TABLES);
    opts.insert(Options::ENABLE_TASKLISTS);

    let mut st = Builder::default();
    for ev in Parser::new_ext(source, opts) {
        st.event(ev);
    }
    st.finish();
    MarkdownDoc { blocks: st.blocks, theme }
}

/// One open list level: whether it's ordered and the next number to emit.
struct ListFrame {
    ordered: bool,
    next_no: u64,
    depth: u8,
}

#[derive(Default)]
struct Builder {
    blocks: Vec<MdBlock>,

    // Inline style nesting counters / current link target.
    bold: u32,
    italic: u32,
    strike: u32,
    link: Option<String>,

    // The runs accumulator for whatever block is currently open.
    runs: Vec<MdRun>,

    // Block context.
    heading_level: Option<u8>,
    in_paragraph: bool,
    in_quote: bool,

    // List context: a stack of open levels + the flat item list of the
    // OUTERMOST list (nested levels append here with a larger depth).
    lists: Vec<ListFrame>,
    list_items: Vec<MdListItem>,
    list_ordered_root: bool,
    // `item_open` is the current item-being-built flag. A nested list
    // flushes the parent item before descending (CommonMark keeps the
    // parent `Item` open across the child `List`), so `End(Item)` only
    // pushes when the item wasn't already flushed.
    item_open: bool,
    item_runs: Vec<MdRun>,
    item_depth: u8,
    item_marker: String,

    // Code block accumulation.
    in_code: bool,
    code_text: String,
}

impl Builder {
    fn event(&mut self, ev: Event) {
        match ev {
            Event::Start(tag) => self.start(tag),
            Event::End(end) => self.end(end),
            Event::Text(t) => self.text(&t),
            Event::Code(t) => self.inline_code(&t),
            Event::SoftBreak => self.text(" "),
            Event::HardBreak => self.text("\n"),
            Event::Rule => self.blocks.push(MdBlock::Rule),
            // HTML, math, footnotes, task markers: ignored in v1.
            _ => {}
        }
    }

    fn start(&mut self, tag: Tag) {
        match tag {
            Tag::Paragraph => self.in_paragraph = true,
            Tag::Heading { level, .. } => {
                self.heading_level = Some(heading_num(level));
                self.runs.clear();
            }
            Tag::BlockQuote(_) => {
                self.in_quote = true;
                self.runs.clear();
            }
            Tag::CodeBlock(_kind) => {
                self.in_code = true;
                self.code_text.clear();
                // `_kind` carries the fence info string (language); v1
                // does not syntax-highlight, so it's intentionally unused.
                let _ = CodeBlockKind::Indented;
            }
            Tag::List(first) => {
                let ordered = first.is_some();
                let depth = self.lists.len() as u8;
                if self.lists.is_empty() {
                    self.list_items.clear();
                    self.list_ordered_root = ordered;
                } else if self.item_open {
                    // Nested list inside an open item: emit the parent's
                    // line now, then descend. The parent's `End(Item)`
                    // (which fires after this child list closes) won't
                    // re-push it.
                    self.flush_item();
                }
                self.lists.push(ListFrame {
                    ordered,
                    next_no: first.unwrap_or(1),
                    depth,
                });
            }
            Tag::Item => {
                self.item_open = true;
                self.item_runs.clear();
                let frame = self.lists.last_mut();
                if let Some(f) = frame {
                    self.item_depth = f.depth;
                    self.item_marker = if f.ordered {
                        let m = format!("{}.", f.next_no);
                        f.next_no += 1;
                        m
                    } else {
                        "•".to_string()
                    };
                }
            }
            Tag::Emphasis => self.italic += 1,
            Tag::Strong => self.bold += 1,
            Tag::Strikethrough => self.strike += 1,
            Tag::Link { dest_url, .. } => self.link = Some(dest_url.to_string()),
            // Image / Table / others: no inline contribution in v1.
            _ => {}
        }
    }

    fn end(&mut self, end: TagEnd) {
        match end {
            TagEnd::Paragraph => {
                self.in_paragraph = false;
                if self.item_open || self.in_quote {
                    // Inside a list item / quote: keep accumulating; a
                    // following paragraph is separated by a newline.
                    self.push_run(MdRun::plain("\n"));
                } else {
                    self.flush_paragraph();
                }
            }
            TagEnd::Heading(_) => {
                if let Some(level) = self.heading_level.take() {
                    let runs = std::mem::take(&mut self.runs);
                    self.blocks.push(MdBlock::Heading {
                        level,
                        runs: trim_runs(runs),
                    });
                }
            }
            TagEnd::BlockQuote(_) => {
                self.in_quote = false;
                let runs = std::mem::take(&mut self.runs);
                self.blocks.push(MdBlock::Quote { runs: trim_runs(runs) });
            }
            TagEnd::CodeBlock => {
                self.in_code = false;
                let text = std::mem::take(&mut self.code_text);
                let text = text.strip_suffix('\n').map(str::to_string).unwrap_or(text);
                self.blocks.push(MdBlock::CodeBlock { text });
            }
            TagEnd::Item => {
                // Only push if this item wasn't already flushed by a
                // nested list starting inside it.
                if self.item_open {
                    self.flush_item();
                }
            }
            TagEnd::List(_) => {
                self.lists.pop();
                if self.lists.is_empty() {
                    let items = std::mem::take(&mut self.list_items);
                    self.blocks.push(MdBlock::List {
                        ordered: self.list_ordered_root,
                        items,
                    });
                }
            }
            TagEnd::Emphasis => self.italic = self.italic.saturating_sub(1),
            TagEnd::Strong => self.bold = self.bold.saturating_sub(1),
            TagEnd::Strikethrough => self.strike = self.strike.saturating_sub(1),
            TagEnd::Link => self.link = None,
            _ => {}
        }
    }

    fn text(&mut self, t: &str) {
        if self.in_code {
            self.code_text.push_str(t);
            return;
        }
        self.push_run(self.styled(t.to_string(), false));
    }

    fn inline_code(&mut self, t: &str) {
        self.push_run(self.styled(t.to_string(), true));
    }

    /// Build a run carrying the current ambient inline styles.
    fn styled(&self, text: String, code: bool) -> MdRun {
        MdRun {
            text,
            bold: self.bold > 0,
            italic: self.italic > 0,
            code,
            strike: self.strike > 0,
            link: self.link.clone(),
        }
    }

    /// Route a run to whichever block is currently open.
    fn push_run(&mut self, run: MdRun) {
        if self.item_open {
            self.item_runs.push(run);
        } else {
            self.runs.push(run);
        }
    }

    /// Emit the in-progress list item and mark it closed.
    fn flush_item(&mut self) {
        self.item_open = false;
        let runs = std::mem::take(&mut self.item_runs);
        self.list_items.push(MdListItem {
            runs: trim_runs(runs),
            depth: self.item_depth,
            marker: std::mem::take(&mut self.item_marker),
        });
    }

    fn flush_paragraph(&mut self) {
        if self.heading_level.is_some() || self.in_quote {
            return;
        }
        let runs = std::mem::take(&mut self.runs);
        let runs = trim_runs(runs);
        if !runs.is_empty() {
            self.blocks.push(MdBlock::Paragraph { runs });
        }
    }

    fn finish(&mut self) {
        // Flush any dangling paragraph (a trailing line with no closing
        // event in malformed input).
        if !self.runs.is_empty() && self.heading_level.is_none() && !self.in_quote {
            let runs = std::mem::take(&mut self.runs);
            let runs = trim_runs(runs);
            if !runs.is_empty() {
                self.blocks.push(MdBlock::Paragraph { runs });
            }
        }
    }
}

fn heading_num(level: HeadingLevel) -> u8 {
    match level {
        HeadingLevel::H1 => 1,
        HeadingLevel::H2 => 2,
        HeadingLevel::H3 => 3,
        HeadingLevel::H4 => 4,
        HeadingLevel::H5 => 5,
        HeadingLevel::H6 => 6,
    }
}

/// Drop leading/trailing whitespace-only runs and trim the outer edges so
/// a block doesn't render with stray newlines from paragraph-join logic.
fn trim_runs(mut runs: Vec<MdRun>) -> Vec<MdRun> {
    while runs.first().map(|r| r.text.trim().is_empty()).unwrap_or(false) {
        runs.remove(0);
    }
    while runs.last().map(|r| r.text.trim().is_empty()).unwrap_or(false) {
        runs.pop();
    }
    if let Some(first) = runs.first_mut() {
        first.text = first.text.trim_start().to_string();
    }
    if let Some(last) = runs.last_mut() {
        last.text = last.text.trim_end().to_string();
    }
    runs
}

#[cfg(test)]
mod tests {
    use super::*;

    fn doc(src: &str) -> Vec<MdBlock> {
        parse(src, MdTheme::light()).blocks
    }

    #[test]
    fn heading_and_paragraph() {
        let b = doc("# Title\n\nHello world");
        assert_eq!(b.len(), 2);
        assert!(matches!(b[0], MdBlock::Heading { level: 1, .. }));
        assert!(matches!(b[1], MdBlock::Paragraph { .. }));
    }

    #[test]
    fn inline_styles() {
        let b = doc("a **bold** *it* `code` ~~no~~");
        let MdBlock::Paragraph { runs } = &b[0] else { panic!("not paragraph") };
        assert!(runs.iter().any(|r| r.bold && r.text == "bold"));
        assert!(runs.iter().any(|r| r.italic && r.text == "it"));
        assert!(runs.iter().any(|r| r.code && r.text == "code"));
        assert!(runs.iter().any(|r| r.strike && r.text == "no"));
    }

    #[test]
    fn link_carries_dest() {
        let b = doc("see [docs](https://x.dev)");
        let MdBlock::Paragraph { runs } = &b[0] else { panic!() };
        let link = runs.iter().find(|r| r.text == "docs").unwrap();
        assert_eq!(link.link.as_deref(), Some("https://x.dev"));
    }

    #[test]
    fn unordered_list_items() {
        let b = doc("- one\n- two\n- three");
        let MdBlock::List { ordered, items } = &b[0] else { panic!("not list") };
        assert!(!ordered);
        assert_eq!(items.len(), 3);
        assert_eq!(items[0].marker, "•");
        assert_eq!(items[1].runs[0].text, "two");
    }

    #[test]
    fn ordered_list_numbers() {
        let b = doc("1. a\n2. b");
        let MdBlock::List { ordered, items } = &b[0] else { panic!() };
        assert!(ordered);
        assert_eq!(items[0].marker, "1.");
        assert_eq!(items[1].marker, "2.");
    }

    #[test]
    fn nested_list_depth() {
        let b = doc("- top\n  - child");
        let MdBlock::List { items, .. } = &b[0] else { panic!() };
        assert_eq!(items.len(), 2);
        assert_eq!(items[0].depth, 0);
        assert_eq!(items[1].depth, 1);
    }

    #[test]
    fn code_block_text() {
        let b = doc("```\nfn main() {}\n```");
        let MdBlock::CodeBlock { text } = &b[0] else { panic!("not code") };
        assert_eq!(text, "fn main() {}");
    }

    #[test]
    fn blockquote_flattens() {
        let b = doc("> quoted line");
        assert!(matches!(b[0], MdBlock::Quote { .. }));
    }

    #[test]
    fn thematic_break() {
        let b = doc("a\n\n---\n\nb");
        assert!(b.iter().any(|x| matches!(x, MdBlock::Rule)));
    }
}
