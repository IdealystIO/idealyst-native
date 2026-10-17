# Idealyst MCP overview

This server answers questions about an idealyst project and drives its running apps. Its tools fall into four groups, each with a docs topic:

- `catalog`: what the project defines (components, primitives, utilities, macros, tokens, types, recipes, SDKs, icons, guides) and how fresh that catalog is.
- `robot`: find, read, press, type, drag, navigate and screenshot a running app; compare two platforms' native render.
- `dev`: launch, watch and stop `idealyst dev` sessions.
- `project`: lint a project and configure its devcontainer / VS Code workspace.

`app` explains the `app` argument, which means a running app on Robot tools and a crate on catalog tools.

The `idealyst://catalog` resource returns the whole catalog as one JSON document (every slice). Prefer the `list_*` / `describe_*` tools, which return only what you ask for.

## get_docs

Tool descriptions are one-line summaries. `get_docs` holds the rest: argument details, return shapes, platform caveats and workflows.

- `get_docs()`: lists the topics and which tools each covers.
- `get_docs(name: "<tool>")`: one tool's summary, parameters, and its full section.
- `get_docs(name: "<topic>")`: a whole topic.
- `query`: keeps only the sections matching any of its words, ranked by how many distinct words each hit. With a `name`, it searches inside that doc; without one, across every doc.

Docs are split by section rather than paged: the largest doc is a few KB, so `query` is the way to read less of it.
