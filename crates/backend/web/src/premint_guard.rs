//! Boot-time MINTED-CLASS scan — premint builds only.
//!
//! A premint build stamps build-time classes instead of resolving
//! rules, and the shipped CSS asset contains a rule for every class the
//! DUMP's build pass saw. A sheet constructed on a path that pass never
//! executed (a subtree first built at runtime) still derives a class —
//! but no CSS rule matches it, and stamping it would render the node
//! silently unstyled. This scan closes that hole: it reads the classes
//! the loaded stylesheets ACTUALLY contain and installs them as the
//! guard set (`runtime_shared::install_minted_classes`), so the attach
//! paths fall back to the live engine for unknown classes (`--premint`)
//! or panic naming the uncrawled construction (`--premint-only`).
//!
//! Failure posture: if the scan finds NO premint classes (the asset
//! failed to load, or this build has none), the guard stays DISARMED —
//! classes are assumed minted, which is the pre-guard behavior. That
//! matters because arming an empty set under `--premint-only` would
//! turn a slow stylesheet into a boot panic. The premint asset is a
//! single file and the browser parses stylesheets atomically, so a
//! non-empty scan is never a partial one.

use wasm_bindgen::prelude::*;

#[wasm_bindgen(
    inline_js = "export function __iy_scan_minted_classes(){\
const out=new Set();\
const collect=(rules)=>{\
  for(const r of rules){\
    if(r.selectorText){\
      const re=/\\.(iy-[A-Za-z0-9_-]+)/g;let m;\
      while((m=re.exec(r.selectorText)))out.add(m[1]);\
    }\
    if(r.cssRules)collect(r.cssRules);\
  }\
};\
for(const ss of document.styleSheets){\
  let rules;try{rules=ss.cssRules}catch(e){continue}\
  if(rules)collect(rules);\
}\
return Array.from(out).join(' ');}"
)]
extern "C" {
    fn __iy_scan_minted_classes() -> String;
}

/// Scan the loaded stylesheets and arm the minted-class guard. Called
/// from the boot seam on premint builds, before the app tree is built
/// (the `<link>` for the premint asset precedes the module script, so
/// its CSSOM is parsed by the time wasm runs).
pub fn install_minted_class_guard() {
    let scanned = __iy_scan_minted_classes();
    if scanned.is_empty() {
        // No premint classes visible — asset missing or not a premint
        // page. Leave the guard disarmed rather than veto everything.
        return;
    }
    runtime_shared::install_minted_classes(scanned.split(' ').map(str::to_string));
}
