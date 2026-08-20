// Assemble the claude.ai/design upload bundle from the Rust converter's
// output (tokens.css, components.css, manifest.json, recipes.json).
//
// Nothing here invents styling. The CSS is idea-ui's own preminted asset
// and the component markup is what `backend_ssr` rendered, so a shell's
// default output is byte-identical to what a real idea-ui app paints.
//
//   node .design-sync/build-bundle.mjs <converter-out-dir> <bundle-dir>

import { readFileSync, writeFileSync, mkdirSync, cpSync, existsSync } from 'node:fs';
import { join } from 'node:path';

const [, , SRC, OUT] = process.argv;
if (!SRC || !OUT) { console.error('usage: build-bundle.mjs <converter-out> <bundle-dir>'); process.exit(1); }

const GLOBAL = 'IdeaUI';
const manifest = JSON.parse(readFileSync(join(SRC, 'manifest.json'), 'utf8'));
const recipes = JSON.parse(readFileSync(join(SRC, 'recipes.json'), 'utf8'));
const byClass = new Map(manifest.map((m) => [m.class, m]));

// Groups mirror the idea-ui docs site's own sections, so the design-tool
// picker matches how the library is already documented.
const GROUPS = {
  Button: 'Actions', IconButton: 'Actions', Link: 'Actions', Menu: 'Actions', Pagination: 'Actions',
  Field: 'Forms', Select: 'Forms', Checkbox: 'Forms', Radio: 'Forms', RadioGroup: 'Forms',
  Switch: 'Forms', Slider: 'Forms', Textarea: 'Forms', Autocomplete: 'Forms',
  SegmentedControl: 'Forms', Calendar: 'Forms', RangeCalendar: 'Forms',
  DateInput: 'Forms', DatePicker: 'Forms', DateRangePicker: 'Forms', TimeInput: 'Forms',
  Stack: 'Layout', Grid: 'Layout', Center: 'Layout', Spacer: 'Layout', Divider: 'Layout', Card: 'Layout',
  Surface: 'Layout',
  Table: 'Data', List: 'Data', Avatar: 'Data', Tag: 'Data', Badge: 'Data', Typography: 'Data',
  Chip: 'Data',
  Alert: 'Status', Progress: 'Status', Spinner: 'Status', Skeleton: 'Status', ToastHost: 'Status',
  Modal: 'Overlays', Popover: 'Overlays', Tooltip: 'Overlays',
  Tabs: 'Navigation', Breadcrumbs: 'Navigation', Accordion: 'Navigation', Collapsible: 'Navigation',
  Icon: 'Foundations', Image: 'Foundations',
};
const groupOf = (n) => GROUPS[n] || 'Components';

// --- minimal HTML parser (SSR output only: well-formed, no comments/scripts)
const VOID = new Set(['br', 'hr', 'img', 'input', 'meta', 'link']);
function parse(html) {
  let i = 0;
  const parseNodes = () => {
    const out = [];
    while (i < html.length) {
      if (html.startsWith('</', i)) break;
      if (html[i] === '<') {
        const end = html.indexOf('>', i);
        const raw = html.slice(i + 1, end);
        const selfClose = raw.endsWith('/');
        const body = selfClose ? raw.slice(0, -1) : raw;
        const sp = body.search(/\s/);
        const tag = (sp === -1 ? body : body.slice(0, sp)).toLowerCase();
        const attrs = {};
        if (sp !== -1) {
          for (const m of body.slice(sp).matchAll(/([\w:-]+)\s*=\s*"([^"]*)"/g)) attrs[m[1]] = m[2];
        }
        i = end + 1;
        let children = [];
        if (!selfClose && !VOID.has(tag)) {
          children = parseNodes();
          const close = html.indexOf('>', i);
          if (close !== -1) i = close + 1;
        }
        out.push({ tag, attrs, children });
      } else {
        const next = html.indexOf('<', i);
        const text = html.slice(i, next === -1 ? html.length : next);
        if (text.trim()) out.push({ text });
        i = next === -1 ? html.length : next;
      }
    }
    return out;
  };
  return parseNodes();
}

// The converter wraps every recipe in a bare `view` so SSR gets a single
// root. Strip it: an unstyled wrapper is scaffolding, not the component.
function unwrap(nodes) {
  if (nodes.length === 1 && nodes[0].tag === 'div' && !nodes[0].attrs?.class && nodes[0].children?.length) {
    return nodes[0].children;
  }
  return nodes;
}

// --- shell emission
// A node whose first `iy-` class is a known sheet gets its class list
// recomputed from props; every other attribute is passed through as
// rendered. Axis props are named after the axes themselves, plus the
// tone+variant fusion the Rust API uses (`appearance` = `tone_variant`).
function axisInfo(node) {
  const cls = (node.attrs?.class || '').split(/\s+/).filter(Boolean);
  const base = cls.find((c) => byClass.has(c));
  return base ? { base, entry: byClass.get(base), extra: cls.filter((c) => c !== base && !c.startsWith(base + '-')) } : null;
}

// The content slot: deepest element whose children are all text. Children
// passed by the caller replace that text, so wrapper styling survives.
function findSlot(nodes) {
  let best = null;
  const walk = (ns, path) => {
    ns.forEach((n, idx) => {
      if (n.text) return;
      const kids = n.children || [];
      const allText = kids.length > 0 && kids.every((k) => k.text);
      if (allText && (!best || path.length + 1 > best.length)) best = [...path, idx];
      walk(kids, [...path, idx]);
    });
  };
  walk(nodes, []);
  return best;
}

const REACT_ATTR = { class: 'className', for: 'htmlFor', tabindex: 'tabIndex', colspan: 'colSpan', rowspan: 'rowSpan', readonly: 'readOnly', maxlength: 'maxLength' };
function jsAttrs(attrs) {
  const o = {};
  for (const [k, v] of Object.entries(attrs || {})) {
    if (k === 'style') { o.style = styleObj(v); continue; }
    o[REACT_ATTR[k] || k] = JSON.stringify(v);
  }
  return o;
}
function styleObj(css) {
  const o = {};
  for (const decl of css.split(';')) {
    const ix = decl.indexOf(':');
    if (ix === -1) continue;
    const prop = decl.slice(0, ix).trim().replace(/-([a-z])/g, (_, c) => c.toUpperCase());
    o[prop] = decl.slice(ix + 1).trim();
  }
  return '{' + Object.entries(o).map(([k, v]) => `${JSON.stringify(k)}:${JSON.stringify(v)}`).join(',') + '}';
}

// Only the ROOT node's sheet contributes props. A nested node keeps the
// classes the framework rendered: its axes belong to the component it is
// part of, not to this one. Without this, Card and Stack inherited the
// Typography axes (`kind`, `weight`, `align`) of the text inside their own
// recipe, and same-named axes from different sheets collided — `Stack.align`
// documented Typography's `left | right | center | justify` instead of the
// flex alignment Stack actually has.
function emitNode(node, path, slot, axesSeen, isRoot) {
  if (node.text) return JSON.stringify(node.text);
  const info = isRoot ? axisInfo(node) : null;
  const attrs = jsAttrs(node.attrs);
  if (info) {
    // Record the entry itself. Looking an axis up by NAME across the whole
    // manifest picks whichever sheet declares it first — which is how
    // Button's docs ended up quoting the macro sheet's `primary_solid`
    // instead of the `primary_filled` its markup actually stamps.
    for (const a of info.entry.axes) axesSeen.set(a.axis, a);
    const axisExpr = info.entry.axes
      .map((a) => `cx(${JSON.stringify(info.base)},${JSON.stringify(a.axis)},p.${a.axis},${JSON.stringify(a.default)})`)
      .join('+');
    const extra = info.extra.length ? '+' + JSON.stringify(' ' + info.extra.join(' ')) : '';
    attrs.className = `(${JSON.stringify(info.base)}${axisExpr ? '+' + axisExpr : ''}${extra}+(p.className?" "+p.className:""))`;
  }
  const isSlot = slot && path.join('.') === slot.join('.');
  const kids = isSlot
    ? ['(p.children!==undefined?p.children:' + (node.children.map((c) => JSON.stringify(c.text || '')).join('+') || '""') + ')']
    : (node.children || []).map((c, i) => emitNode(c, [...path, i], slot, axesSeen, false));
  const props = '{' + Object.entries(attrs).map(([k, v]) => `${JSON.stringify(k)}:${v}`).join(',') + '}';
  return `h(${JSON.stringify(node.tag)},${props}${kids.length ? ',' + kids.join(',') : ''})`;
}

// --- build
mkdirSync(join(OUT, 'tokens'), { recursive: true });
mkdirSync(join(OUT, '_preview'), { recursive: true });

cpSync(join(SRC, 'tokens.css'), join(OUT, 'tokens', 'tokens.css'));
cpSync(join(SRC, 'components.css'), join(OUT, '_ds_bundle.css'));
// The live-engine remainder: components whose style application carries
// overrides or a computed layer cannot premint, so their rules come out of
// the render's head_css rather than the dumped asset. Table's measured
// columns and Toast's stack are the two here — without this file their
// markup references `ui-<hash>` classes nothing defines, and the layout
// collapses to min-content.
if (existsSync(join(SRC, 'runtime.css'))) cpSync(join(SRC, 'runtime.css'), join(OUT, 'runtime.css'));

// Rendered designs receive only styles.css's transitive @import closure,
// so the component CSS must be imported here — not merely linked by a card.
writeFileSync(join(OUT, 'styles.css'),
  `/* idea-ui — single stylesheet entry. Link this one file. */\n@import "./tokens/tokens.css";\n@import "./_ds_bundle.css";\n@import "./runtime.css";\n`);

const byComponent = new Map();
for (const r of recipes) {
  if (!byComponent.has(r.component)) byComponent.set(r.component, []);
  byComponent.get(r.component).push(r);
}

const shells = [];
const components = [];
for (const [name, rs] of byComponent) {
  const primary = rs[0];
  const nodes = unwrap(parse(primary.html));
  const slot = findSlot(nodes);
  const axesSeen = new Map();
  const body = nodes.map((n, i) => emitNode(n, [i], slot, axesSeen, true)).join(',');
  const root = nodes.length === 1 ? body : `h(React.Fragment,null,${body})`;
  shells.push(`  ${name}: function ${name}(props){var p=props||{};return ${root};}`);
  components.push({ name, group: groupOf(name), axes: [...axesSeen.values()], recipes: rs });
}

// `cx` mirrors StyleApplication::preminted_class_list exactly:
// `<base> <base>-<axis>-<value>` per axis, defaulting where the caller
// omitted the axis. Getting this wrong is what makes a stamped class miss
// its rule, so it lives in one place.
const bundleBody = `(function(){
var React=window.React,h=React.createElement;
function cx(base,axis,val,def){var v=val==null?def:val;return v==null?"":" "+base+"-"+axis+"-"+v;}
var ${GLOBAL}={
${shells.join(',\n')}
};
window.${GLOBAL}=${GLOBAL};
})();`;

const header = {
  namespace: GLOBAL,
  components: components.map((c) => ({ name: c.name, sourcePath: `components/${c.group}/${c.name}/${c.name}.jsx` })),
  sourceHashes: {},
  inlinedExternals: [],
  builtBy: 'idealyst-design-sync',
};
writeFileSync(join(OUT, '_ds_bundle.js'), `/* @ds-bundle: ${JSON.stringify(header).replace(/\*\//g, '*\\/')} */\n${bundleBody}`);

// --- per-component files
for (const c of components) {
  const dir = join(OUT, 'components', c.group, c.name);
  mkdirSync(dir, { recursive: true });

  const axisDocs = c.axes.map((a) => ({ axis: a.axis, values: a.values, def: a.default }));

  // .d.ts — the contract the design agent codes against.
  writeFileSync(join(dir, `${c.name}.d.ts`),
    `import * as React from 'react';\n\nexport interface ${c.name}Props {\n` +
    axisDocs.map((a) => `  /** ${a.axis} (default ${JSON.stringify(a.def)}) */\n  ${a.axis}?: ${a.values.map((v) => JSON.stringify(v)).join(' | ') || 'string'};\n`).join('') +
    `  /** Replaces the component's text/content slot. */\n  children?: React.ReactNode;\n` +
    `  /** Appended to the root's class list. */\n  className?: string;\n}\n\n` +
    `export declare const ${c.name}: React.FC<${c.name}Props>;\n`);

  // .jsx — the source the header points at.
  writeFileSync(join(dir, `${c.name}.jsx`),
    `// Generated from idea-ui's SSR-rendered output. Styling comes entirely\n` +
    `// from idea-ui's own preminted CSS (_ds_bundle.css); this shell only\n` +
    `// stamps the classes the framework would stamp.\n` +
    `export { ${c.name} } from '${GLOBAL}';\n`);

  // .prompt.md — usage reference, from the repo's compile-checked recipes.
  writeFileSync(join(dir, `${c.name}.prompt.md`),
    `# ${c.name}\n\n${c.recipes[0].doc}\n\n## Props\n\n` +
    (axisDocs.length
      ? axisDocs.map((a) => `- \`${a.axis}\` — ${a.values.join(' | ')} (default \`${a.def}\`)`).join('\n')
      : '_No style axes; content-only._') +
    `\n- \`children\` — replaces the text/content slot\n- \`className\` — appended to the root class list\n\n` +
    `## Usage\n\n\`\`\`jsx\n<${c.name}${axisDocs.filter((a) => a.axis === 'appearance').map(() => ' appearance="primary_filled"').join('')}>…</${c.name}>\n\`\`\`\n\n` +
    `## Source of truth\n\nThis component is Rust. The canonical API is \`idea_ui::${c.name}\`:\n\n` +
    c.recipes.map((r) => `- \`${r.recipe}\` — ${r.doc}`).join('\n') + '\n');

  // preview stories — the exact markup idea-ui rendered, one per recipe.
  const stories = c.recipes.map((r, i) => {
    const html = JSON.stringify(unwrapHtml(r.html));
    return `  ${storyName(r.recipe, i)}: function(){return h('div',{dangerouslySetInnerHTML:{__html:${html}}});}`;
  });
  writeFileSync(join(OUT, '_preview', `${c.name}.js`),
    `(function(){var React=window.React,h=React.createElement;window.__dsPreview={\n${stories.join(',\n')}\n};})();`);

  writeFileSync(join(dir, `${c.name}.html`), cardHtml(c));
}

function storyName(fn, i) {
  // `button_icon_block` -> `ButtonIconBlock`; the card prints it with the
  // words split back out, so it stays readable as a heading.
  const n = fn.split(/[^a-z0-9]+/i).filter(Boolean).map((w) => w.charAt(0).toUpperCase() + w.slice(1)).join('');
  return n || `Story${i}`;
}
function unwrapHtml(html) {
  const m = html.match(/^<div>([\s\S]*)<\/div>$/);
  return m ? m[1] : html;
}
function cardHtml(c) {
  return `<!-- @dsCard group="${c.group}" -->
<!doctype html>
<html><head><meta charset="utf-8">
  <link rel="stylesheet" href="../../../styles.css">
  <style>
    body{margin:0;padding:24px;background:var(--color-background);color:var(--color-text);font-family:var(--iy-default-font)}
    .ds-grid{display:grid;grid-template-columns:repeat(auto-fit,minmax(320px,1fr));gap:20px;align-items:start}
    .ds-cell{border:1px solid var(--color-border);border-radius:var(--radius-md);padding:12px;min-width:0;overflow:hidden;
      display:flex;flex-direction:column;align-items:stretch;gap:8px}
    .ds-cell>h4{margin:0;align-self:stretch;font:600 12px system-ui;color:var(--color-text-muted);text-transform:uppercase;letter-spacing:.04em}
  </style>
</head><body>
  <div class="ds-grid" id="g"></div>
  <script src="../../../_vendor/react.js"></script>
  <script src="../../../_vendor/react-dom.js"></script>
  <script src="../../../_ds_bundle.js"></script>
  <script src="../../../_preview/${c.name}.js"></script>
  <script>
    var h=React.createElement,g=document.getElementById('g');
    var P=window.__dsPreview||{};
    var root=ReactDOM.createRoot(g);
    root.render(Object.keys(P).map(function(k){
      return h('div',{className:'ds-cell',key:k},h('h4',null,k.replace(/([a-z])([A-Z])/g,'$1 $2')),h(P[k]));
    }));
  </script>
</body></html>`;
}

// The conventions header is prepended to the README, which is what the
// design agent reads. Hand-authored and human-editable — the generator
// never rewrites it, it only stitches it in.
const headerPath = new URL('./conventions.md', import.meta.url);
const conventions = existsSync(headerPath) ? readFileSync(headerPath, 'utf8') + '\n\n---\n\n' : '';
writeFileSync(join(OUT, 'README.md'), conventions +
  `# idea-ui (Idealyst)\n\n${components.length} components, rendered from the Rust design system.\n\n` +
  `- \`styles.css\` — the single stylesheet entry; it \`@import\`s the tokens and idea-ui's own component CSS.\n` +
  `- \`_ds_bundle.js\` — React shells on \`window.${GLOBAL}\`.\n\n## Components\n\n` +
  components.map((c) => `- **${c.name}** (${c.group})`).join('\n') + '\n');

console.log(`[bundle] ${components.length} components across ${new Set(components.map((c) => c.group)).size} groups`);
