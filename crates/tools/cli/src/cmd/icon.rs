//! `idealyst icon` — inspect generated icons.
//!
//! Today only `preview` is wired. It (re)runs the sync flow so the
//! generated files are current, then writes a self-contained HTML
//! page that shows each platform's icon inside a platform-styled
//! mockup (browser tab strip, iOS home tile, Android adaptive masks,
//! macOS dock chrome). The page is dropped at
//! `target/idealyst/icons/preview.html` and opened in the default
//! browser.

use std::path::{Path, PathBuf};
use std::process::Command;

use anyhow::{Context, Result};
use icon_gen::Target;

#[derive(clap::Args, Debug)]
pub struct Args {
    #[command(subcommand)]
    pub action: Action,
}

#[derive(clap::Subcommand, Debug)]
pub enum Action {
    /// Render a side-by-side preview of every generated icon inside
    /// platform-style mockups (browser tab strip, iOS home tile,
    /// Android adaptive masks, macOS dock). Opens in the default
    /// browser unless `--no-open` is passed.
    Preview {
        /// Project directory. Defaults to the current directory.
        #[arg(long, default_value = ".")]
        dir: PathBuf,
        /// Skip launching the browser; just write the HTML file.
        #[arg(long)]
        no_open: bool,
    },
}

pub fn run(args: Args) -> Result<()> {
    match args.action {
        Action::Preview { dir, no_open } => preview(&dir, !no_open),
    }
}

fn preview(project_dir: &Path, open_in_browser: bool) -> Result<()> {
    let dir = project_dir
        .canonicalize()
        .unwrap_or_else(|_| project_dir.to_path_buf());

    // Make sure each platform's outputs are current. The icon-gen
    // cache short-circuits this when nothing's changed, so the cost
    // on a repeat run is one stat-and-hash per platform.
    let Some(config) = icon_gen::load_config_from_manifest(&dir)? else {
        anyhow::bail!(
            "no `[package.metadata.idealyst.app.icon]` block at {} — \
             nothing to preview (declare `source = \"path/to/icon.svg\"` \
             or `foreground` + `background` to opt in)",
            dir.display(),
        );
    };

    let icons_root = dir.join("target").join("idealyst").join("icons");
    let web_out = icons_root.join("web");
    let ios_out = icons_root.join("ios");
    let android_out = icons_root.join("android");
    let macos_out = icons_root.join("macos");

    icon_gen::sync_web_icons(Some(&config.resolved_for(Target::Web)), &web_out)?;
    icon_gen::sync_ios_icons(Some(&config.resolved_for(Target::Ios)), &ios_out)?;
    let android = icon_gen::sync_android_icons(
        Some(&config.resolved_for(Target::Android)),
        &android_out,
    )?;
    icon_gen::sync_macos_icns(Some(&config.resolved_for(Target::Macos)), &macos_out)?;

    let preview_path = icons_root.join("preview.html");
    let html = build_preview_html(&dir, &icons_root, android.as_ref())?;
    std::fs::write(&preview_path, html)
        .with_context(|| format!("write {}", preview_path.display()))?;

    println!("[icon preview] wrote {}", preview_path.display());
    if open_in_browser {
        open_in_default_browser(&preview_path)?;
    }
    Ok(())
}

/// Compose the preview HTML. Self-contained: all CSS inline, all
/// images referenced via paths relative to the HTML file (which
/// lives next to the platform output directories under `target/`,
/// so `web/favicon.ico` etc. resolve correctly).
fn build_preview_html(
    project_dir: &Path,
    icons_root: &Path,
    android: Option<&icon_gen::AndroidOutputs>,
) -> Result<String> {
    let project_name = project_dir
        .file_name()
        .and_then(|n| n.to_str())
        .unwrap_or("project");

    // Adaptive layer paths (when present) drive the live-mockup
    // section that shows the Android system mask in action.
    let adaptive_section = match android.and_then(|a| a.adaptive.as_ref()) {
        Some(a) => {
            // Use the xxxhdpi assets (largest, sharpest) so the
            // mockup looks crisp regardless of the preview
            // window size.
            let fg = relative_to(&a.foreground_pngs[4], icons_root);
            let bg = relative_to(&a.background_pngs[4], icons_root);
            format!(
                r##"
    <section>
      <h2>Android adaptive — mask preview</h2>
      <p class="muted">
        API-26+ devices composite foreground over background and
        crop with whichever mask the launcher chooses. Pixel ships
        the circle mask; Samsung's One UI uses a squircle.
      </p>
      <div class="row">
        <figure>
          <div class="adaptive circle">
            <img class="bg" src="{bg}" alt="">
            <img class="fg" src="{fg}" alt="">
          </div>
          <figcaption>circle mask (Pixel)</figcaption>
        </figure>
        <figure>
          <div class="adaptive squircle">
            <img class="bg" src="{bg}" alt="">
            <img class="fg" src="{fg}" alt="">
          </div>
          <figcaption>squircle mask</figcaption>
        </figure>
        <figure>
          <div class="adaptive rounded-square">
            <img class="bg" src="{bg}" alt="">
            <img class="fg" src="{fg}" alt="">
          </div>
          <figcaption>rounded square</figcaption>
        </figure>
      </div>
    </section>"##,
            )
        }
        None => String::new(),
    };

    Ok(format!(
        r##"<!doctype html>
<html lang="en">
<head>
<meta charset="utf-8">
<title>{project_name} — icon preview</title>
<link rel="icon" href="web/favicon.ico">
<style>
  :root {{
    --bg: #0e1116;
    --panel: #161b22;
    --border: #30363d;
    --text: #e6edf3;
    --muted: #8b949e;
    --accent: #58a6ff;
  }}
  * {{ box-sizing: border-box; }}
  body {{
    margin: 0;
    font: 14px/1.5 -apple-system, BlinkMacSystemFont, "Segoe UI", system-ui, sans-serif;
    background: var(--bg);
    color: var(--text);
  }}
  header {{
    padding: 24px 32px;
    border-bottom: 1px solid var(--border);
  }}
  header h1 {{ margin: 0; font-size: 18px; font-weight: 600; }}
  header .muted {{ color: var(--muted); margin-top: 4px; }}
  main {{ max-width: 1100px; margin: 0 auto; padding: 32px; }}
  section {{
    background: var(--panel);
    border: 1px solid var(--border);
    border-radius: 8px;
    padding: 24px;
    margin-bottom: 24px;
  }}
  section h2 {{ margin: 0 0 8px; font-size: 16px; }}
  section p {{ margin: 0 0 16px; }}
  .muted {{ color: var(--muted); }}
  .row {{
    display: flex;
    flex-wrap: wrap;
    gap: 32px;
    align-items: flex-end;
  }}
  figure {{ margin: 0; text-align: center; }}
  figcaption {{ margin-top: 8px; color: var(--muted); font-size: 12px; }}

  /* Browser tab strip mockup. Mid-grey bar with a single tab
     containing the favicon + title. Matches Chrome's tab metrics
     roughly. */
  .tab-strip {{
    background: #1d2128;
    border: 1px solid var(--border);
    border-radius: 8px 8px 0 0;
    padding: 8px;
    width: 360px;
  }}
  .tab {{
    background: var(--bg);
    border-radius: 6px;
    padding: 6px 12px;
    display: flex;
    align-items: center;
    gap: 8px;
  }}
  .tab img {{ width: 16px; height: 16px; }}
  .tab .title {{ font-size: 12px; }}

  /* iOS home-screen tile. Apple's icon shape is a squircle. The
     system applies it; for the preview we approximate with
     border-radius: 22.37% (Apple HIG continuous-curvature value). */
  .ios-home {{
    width: 120px;
    height: 120px;
    border-radius: 22.37%;
    overflow: hidden;
    box-shadow: 0 8px 24px rgba(0,0,0,0.4);
  }}
  .ios-home img {{ width: 100%; height: 100%; display: block; }}

  /* macOS dock. Bottom-aligned icon with a soft shadow. The dock's
     own glass + reflection effect would be overkill here; we just
     show the icon at the standard 128 dock size. */
  .dock {{
    background: linear-gradient(180deg, #2a2f37 0%, #1d2128 100%);
    border-radius: 16px;
    padding: 16px 24px 24px;
    display: inline-flex;
    gap: 16px;
    align-items: flex-end;
  }}
  .dock img {{
    width: 96px; height: 96px;
    filter: drop-shadow(0 4px 6px rgba(0,0,0,0.5));
  }}

  /* Android adaptive masks. Per the spec the visible safe zone is a
     circle 66/108 ≈ 61% of the canvas, but the LAUNCHER's mask is
     applied OUTSIDE that — so we don't crop to 61%, we mask the
     entire 108×108 layered tile. */
  .adaptive {{
    width: 128px; height: 128px;
    position: relative;
    overflow: hidden;
  }}
  .adaptive .bg, .adaptive .fg {{
    position: absolute; inset: 0;
    width: 100%; height: 100%; object-fit: cover;
  }}
  .adaptive.circle {{ border-radius: 50%; }}
  .adaptive.squircle {{ border-radius: 25%; }}
  .adaptive.rounded-square {{ border-radius: 18%; }}

  /* Raw file listing — for when the user wants to see the actual
     emitted PNGs at full size. */
  .files {{
    display: grid;
    grid-template-columns: repeat(auto-fill, minmax(96px, 1fr));
    gap: 12px;
  }}
  .files figure {{
    background: var(--bg);
    border: 1px solid var(--border);
    border-radius: 4px;
    padding: 8px;
  }}
  .files img {{
    max-width: 100%;
    max-height: 64px;
    display: block;
    margin: 0 auto;
  }}
  code {{
    background: var(--bg);
    padding: 1px 4px;
    border-radius: 3px;
    color: var(--accent);
    font-size: 12px;
  }}
</style>
</head>
<body>
<header>
  <h1>{project_name} — icon preview</h1>
  <div class="muted">
    Generated from <code>[package.metadata.idealyst.app.icon]</code>.
    Refresh after editing the SVG to re-render.
  </div>
</header>
<main>

  <section>
    <h2>Web</h2>
    <p class="muted">Browser tab strip preview using <code>web/favicon.ico</code>.</p>
    <div class="tab-strip">
      <div class="tab">
        <img src="web/favicon.ico" alt="">
        <span class="title">{project_name}</span>
      </div>
    </div>
  </section>

  <section>
    <h2>iOS — home-screen tile</h2>
    <p class="muted">
      Apple's squircle mask is applied by the system. The preview
      approximates it with <code>border-radius: 22.37%</code>
      (Apple HIG continuous-curvature value).
    </p>
    <div class="row">
      <figure>
        <div class="ios-home">
          <img src="ios/AppIcon60x60@3x.png" alt="">
        </div>
        <figcaption>iPhone — 60pt @3x (180×180)</figcaption>
      </figure>
      <figure>
        <div class="ios-home">
          <img src="ios/AppIcon76x76@2x.png" alt="">
        </div>
        <figcaption>iPad — 76pt @2x (152×152)</figcaption>
      </figure>
      <figure>
        <div class="ios-home" style="width: 200px; height: 200px;">
          <img src="ios/AppIcon-1024.png" alt="">
        </div>
        <figcaption>App Store — 1024×1024</figcaption>
      </figure>
    </div>
  </section>

  <section>
    <h2>macOS — dock</h2>
    <p class="muted">
      Dock icon preview from <code>macos/AppIcon.icns</code>. macOS
      reads this from <code>Resources/</code> in the .app bundle
      and shows it in the dock, command-tab, Finder.
    </p>
    <div class="dock">
      <img src="ios/AppIcon-1024.png" alt="">
    </div>
    <p class="muted" style="margin-top: 12px;">
      <small>
        Note: the dock mockup samples the iOS 1024 PNG because
        the .icns binary format isn't browser-renderable. The
        bytes inside the .icns are the same RGBA the dock shows.
      </small>
    </p>
  </section>

  {adaptive_section}

  <section>
    <h2>All generated files</h2>
    <p class="muted">Raw outputs as they sit on disk under <code>target/idealyst/icons/</code>.</p>
    <div class="files">{file_listing}</div>
  </section>

</main>
</body>
</html>"##,
        file_listing = collect_file_listing(icons_root)?,
    ))
}

/// Walk the icons output tree and emit a `<figure>` per PNG. Skips
/// the .icns (browsers can't render it) and the cache sidecars.
fn collect_file_listing(icons_root: &Path) -> Result<String> {
    let mut entries = Vec::new();
    walk_pngs(icons_root, icons_root, &mut entries)?;
    entries.sort();
    Ok(entries
        .into_iter()
        .map(|rel| {
            format!(
                "<figure><img src=\"{rel}\" alt=\"\" loading=\"lazy\">\
                 <figcaption><code>{rel}</code></figcaption></figure>"
            )
        })
        .collect::<Vec<_>>()
        .join(""))
}

fn walk_pngs(root: &Path, dir: &Path, out: &mut Vec<String>) -> Result<()> {
    for entry in std::fs::read_dir(dir)? {
        let entry = entry?;
        let path = entry.path();
        if path.is_dir() {
            walk_pngs(root, &path, out)?;
        } else if let Some(ext) = path.extension().and_then(|s| s.to_str()) {
            if matches!(ext, "png" | "ico") {
                out.push(relative_to(&path, root));
            }
        }
    }
    Ok(())
}

fn relative_to(path: &Path, base: &Path) -> String {
    pathdiff::diff_paths(path, base)
        .unwrap_or_else(|| path.to_path_buf())
        .to_string_lossy()
        .replace('\\', "/")
}

fn open_in_default_browser(path: &Path) -> Result<()> {
    let cmd = if cfg!(target_os = "macos") {
        "open"
    } else if cfg!(target_os = "windows") {
        "start"
    } else {
        "xdg-open"
    };
    Command::new(cmd)
        .arg(path)
        .spawn()
        .with_context(|| format!("spawn `{cmd}` to open preview"))?;
    Ok(())
}
