idea-ui design MCP: opinionated design guidance for apps built on idea-ui. It complements the `idealyst` MCP (APIs and the project catalog) with how to *use* them well: screens, navigation, layout, forms, states, brand.

House rules, always:
- The look comes from the user. Never invent a brand; put theirs in one src/brand.rs (theme tokens, then app-wide restyles). Where they're silent, keep idea-ui's default.
- Tokens only: no hex, px spacing or font sizes in screens.
- Every design works at phone AND desktop width, and is adaptive: patterns change kind across form factors (push/pop ⇄ split view, drawer/tabs ⇄ sidebar, sheet ⇄ dialog) over one route table.
- idea-ui components first; restyle them once, never per call site.
- One primary action per view; design loading, error, empty and no-results.
- Handle platform conventions through framework seams (breakpoints, safe_area, primary_pointer, keyboard_avoiding_view), never per-platform constants.

Start with list_design_skills, then read_design_skill("principles"). Before building a screen, get_design_recipe the matching recipe: each is a whole module compiled and server-rendered against idea-ui at phone and desktop widths. search_design finds a rule by words.
