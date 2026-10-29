//! Print a recipe's server-rendered HTML — the page the render checks
//! read. For writing a recipe's `//! render:` lines or debugging a failing
//! one:
//!
//! ```text
//! cargo run -p mcp-design-recipes-check --example dump -- <recipe> [path] [phone|tablet|desktop]
//! ```

fn main() {
    let args: Vec<String> = std::env::args().collect();
    let name = &args[1];
    let path = args.get(2).cloned().unwrap_or("/".into());
    let (w, h) = match args.get(3).map(String::as_str) {
        Some("phone") => (390.0, 844.0),
        Some("tablet") => (820.0, 1180.0),
        _ => (1280.0, 800.0),
    };
    let app = mcp_design_recipes_check::RECIPES.iter().find(|(n, _)| n == name).expect("recipe").1;
    let html = std::thread::Builder::new()
        .stack_size(16 << 20)
        .spawn(move || {
            backend_ssr::newcore::render_path_with(&path, |_| {}, move || {
                runtime_shared::set_viewport_size(runtime_shared::ViewportSize::new(w, h));
                app()
            })
            .html
        })
        .unwrap()
        .join()
        .unwrap();
    println!("{html}");
}
