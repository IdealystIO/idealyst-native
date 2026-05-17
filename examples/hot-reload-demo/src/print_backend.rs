//! `PrintBackend` — a `framework_core::Backend` that just prints
//! each call. Stand-in for a real platform backend; lets us see
//! exactly what the wire delivered without building a UI.

use std::rc::Rc;

use framework_core::primitives;
use framework_core::{Backend, Color, StyleRules};

/// Numeric node ids minted as the backend creates them, so the
/// printed output references nodes consistently across calls.
#[derive(Clone, Copy, Debug)]
pub struct NodeRef(pub u64);

#[derive(Default)]
pub struct PrintBackend {
    next: u64,
}

impl PrintBackend {
    pub fn new() -> Self {
        Self::default()
    }

    fn mint(&mut self) -> NodeRef {
        self.next += 1;
        let n = NodeRef(self.next);
        n
    }
}

impl Backend for PrintBackend {
    type Node = NodeRef;

    fn create_view(&mut self) -> Self::Node {
        let n = self.mint();
        println!("[app]   create_view → {:?}", n);
        n
    }

    fn create_text(&mut self, content: &str) -> Self::Node {
        let n = self.mint();
        println!("[app]   create_text({:?}) → {:?}", content, n);
        n
    }

    fn create_button(
        &mut self,
        label: &str,
        on_click: Rc<dyn Fn()>,
        _leading: Option<&primitives::icon::IconData>,
        _trailing: Option<&primitives::icon::IconData>,
    ) -> Self::Node {
        let n = self.mint();
        println!("[app]   create_button({:?}) → {:?}", label, n);
        // Stash the on_click so it doesn't get optimized out; the
        // wire client wires this into the event channel.
        let _ = on_click;
        n
    }

    fn create_pressable(&mut self, on_click: Rc<dyn Fn()>) -> Self::Node {
        let n = self.mint();
        println!("[app]   create_pressable → {:?}", n);
        let _ = on_click;
        n
    }

    fn create_reactive_anchor(&mut self) -> Self::Node {
        let n = self.mint();
        println!("[app]   create_reactive_anchor → {:?}", n);
        n
    }

    fn create_image(&mut self, src: &str, alt: Option<&str>) -> Self::Node {
        let n = self.mint();
        println!("[app]   create_image({:?}, alt={:?}) → {:?}", src, alt, n);
        n
    }

    fn update_image_src(&mut self, node: &Self::Node, src: &str) {
        println!("[app]   update_image_src({:?}, {:?})", node, src);
    }

    fn create_text_input(
        &mut self,
        initial_value: &str,
        placeholder: Option<&str>,
        on_change: Rc<dyn Fn(String)>,
    ) -> Self::Node {
        let n = self.mint();
        println!(
            "[app]   create_text_input(value={:?}, placeholder={:?}) → {:?}",
            initial_value, placeholder, n
        );
        let _ = on_change;
        n
    }

    fn update_text_input_value(&mut self, node: &Self::Node, value: &str) {
        println!("[app]   update_text_input_value({:?}, {:?})", node, value);
    }

    fn create_toggle(
        &mut self,
        initial_value: bool,
        on_change: Rc<dyn Fn(bool)>,
    ) -> Self::Node {
        let n = self.mint();
        println!("[app]   create_toggle(value={}) → {:?}", initial_value, n);
        let _ = on_change;
        n
    }

    fn update_toggle_value(&mut self, node: &Self::Node, value: bool) {
        println!("[app]   update_toggle_value({:?}, {})", node, value);
    }

    fn create_scroll_view(&mut self, horizontal: bool) -> Self::Node {
        let n = self.mint();
        println!("[app]   create_scroll_view(horizontal={}) → {:?}", horizontal, n);
        n
    }

    fn create_slider(
        &mut self,
        initial_value: f32,
        min: f32,
        max: f32,
        step: Option<f32>,
        on_change: Rc<dyn Fn(f32)>,
    ) -> Self::Node {
        let n = self.mint();
        println!(
            "[app]   create_slider(value={}, min={}, max={}, step={:?}) → {:?}",
            initial_value, min, max, step, n
        );
        let _ = on_change;
        n
    }

    fn insert(&mut self, parent: &mut Self::Node, child: Self::Node) {
        println!("[app]   insert(parent={:?}, child={:?})", parent, child);
    }

    fn insert_many(&mut self, parent: &mut Self::Node, children: Vec<Self::Node>) {
        println!("[app]   insert_many(parent={:?}, {} children)", parent, children.len());
    }

    fn update_text(&mut self, node: &Self::Node, content: &str) {
        println!("[app]   update_text({:?}, {:?})", node, content);
    }

    fn update_button_label(&mut self, node: &Self::Node, label: &str) {
        println!("[app]   update_button_label({:?}, {:?})", node, label);
    }

    fn clear_children(&mut self, node: &Self::Node) {
        println!("[app]   clear_children({:?})", node);
    }

    fn apply_style(&mut self, node: &Self::Node, _style: &Rc<StyleRules>) {
        println!("[app]   apply_style({:?})", node);
    }

    fn set_disabled(&mut self, node: &Self::Node, disabled: bool) {
        println!("[app]   set_disabled({:?}, {})", node, disabled);
    }

    fn finish(&mut self, root: Self::Node) {
        println!("[app]   finish(root={:?})", root);
    }

    // --- Optional methods with practical defaults to avoid unimplemented! ---

    fn create_link(&mut self, config: primitives::link::LinkConfig) -> Self::Node {
        let n = self.mint();
        println!("[app]   create_link(route={:?}, url={:?}) → {:?}", config.route, config.url, n);
        n
    }

    fn create_overlay(
        &mut self,
        _placement: primitives::overlay::ViewportPlacement,
        _backdrop: primitives::overlay::BackdropMode,
        _on_dismiss: Option<Rc<dyn Fn()>>,
        _trap_focus: bool,
    ) -> Self::Node {
        let n = self.mint();
        println!("[app]   create_overlay → {:?}", n);
        n
    }

    fn create_graphics(
        &mut self,
        _on_ready: primitives::graphics::OnReady,
        _on_resize: primitives::graphics::OnResize,
        _on_lost: primitives::graphics::OnLost,
    ) -> Self::Node {
        let n = self.mint();
        println!("[app]   create_graphics → {:?}", n);
        n
    }

    fn create_navigator(
        &mut self,
        _callbacks: primitives::navigator::NavigatorCallbacks<Self::Node>,
        _control: Rc<primitives::navigator::NavigatorControl>,
    ) -> Self::Node {
        let n = self.mint();
        println!("[app]   create_navigator → {:?}", n);
        n
    }

    fn navigator_attach_initial(
        &mut self,
        navigator: &Self::Node,
        screen: Self::Node,
        scope_id: u64,
        _options: primitives::navigator::ScreenOptions,
    ) {
        println!(
            "[app]   navigator_attach_initial(nav={:?}, screen={:?}, scope={})",
            navigator, screen, scope_id
        );
    }

    fn update_icon_color(&mut self, node: &Self::Node, color: &Color) {
        println!("[app]   update_icon_color({:?}, {:?})", node, color.0);
    }
}
