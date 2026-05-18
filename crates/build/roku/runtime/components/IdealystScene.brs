' ============================================================
' IdealystScene runtime — materializes the Rust framework's UI
' tree from a baked command stream at app launch, then hands
' layout off to Layout.brs.
'
' The build pipeline ran the Rust app once, captured every Backend
' trait call as a RokuCommand, and serialized them into
' pkg:/data/ui.json. We replay those commands here against real
' SceneGraph nodes — there's no host connection at runtime.
'
' Two-stage replay:
'
'   1. Materialize: each Create* command spawns an SGNode; Insert
'      appends parent→child and records parent/children/style/kind
'      in side tables that Layout.brs reads. ApplyStyle stores the
'      style AA (it does NOT touch layout-affecting fields directly;
'      layout is computed in stage 2).
'
'   2. Finish: after the root is attached, run layoutComputeFrames
'      over the whole tree. That fills translation/width/height on
'      every node based on the framework's flex semantics.
'
' Side tables (read by Layout.brs):
'   m.nodes[idStr]      = roSGNode
'   m.styles[idStr]     = AA from the WireStyle JSON, or invalid
'   m.children[idStr]   = roArray of child ids (integers)
'   m.parents[idStr]    = parent id (integer) or invalid
'   m.nodeKinds[idStr]  = "View" / "Text" / "Button" / ...
'
' Supported create commands in this v0 runtime: CreateView,
' CreateText, CreateButton, CreateScrollView, CreateImage,
' CreateActivityIndicator, CreateReactiveAnchor. Tree mutations:
' Insert, ClearChildren. Updates: UpdateText, UpdateButtonLabel,
' UpdateImageSrc. Style: ApplyStyle. Lifecycle: Finish.
'
' Button events and other reactive plumbing are still TODO — see
' the framework's roadmap for Phase 2 (reactivity).
' ============================================================

sub init()
    m.nodes = createObject("roAssociativeArray")
    m.styles = createObject("roAssociativeArray")
    m.children = createObject("roAssociativeArray")
    m.parents = createObject("roAssociativeArray")
    m.nodeKinds = createObject("roAssociativeArray")
    ' Per-node background Rectangle (only present when the node has
    ' a `background` style). Roku's Group has no fill of its own —
    ' we compose a Rectangle as the View's first child and sync its
    ' size in the layout pass. Layout.brs reads this map to size
    ' each background to its host's computed frame.
    m.backgrounds = createObject("roAssociativeArray")
    ' Custom-button inner-node lookups. Each `CreateButton` builds a
    ' Group containing a Rectangle (bg) + Label, and stashes both
    ' here so Layout.brs can size them and ApplyStyle can recolor.
    m.buttonBgs = createObject("roAssociativeArray")
    m.buttonLabels = createObject("roAssociativeArray")
    ' Per-node state overlays (`hovered` / `focused` / `pressed` /
    ' `disabled`). Same author API as web — stylesheets declare
    ' `state hovered { ... }` and the framework hands us the merged
    ' overlay through `apply_styled_states`. We store them here so
    ' focus navigation (in Reactivity.brs) can re-merge on the fly.
    m.styleOverlays = createObject("roAssociativeArray")
    ' Theme registry: keyed by theme name, each value is an AA
    ' from token name to `{ kind, value }`. Populated by
    ' RegisterThemeVariant wire ops; consulted by resolveWireColor
    ' / resolveWireLength when applying styles. The active theme
    ' name lives in `m.activeTheme`; when it changes (driven by
    ' the bound active-theme signal), `reapplyAllStyles` walks
    ' every entry in `m.styles` and re-applies through
    ' `applyNonLayoutStyle`.
    m.themes = createObject("roAssociativeArray")
    m.activeTheme = ""
    m.activeThemeSignalId = invalid
    ' Set up Reactivity.brs's side tables (signals, subscribers,
    ' button actions). Lives in the same component-scoped `m` so
    ' all three scripts share it.
    reactivityInit()
    materializeFromJson()
end sub

' Scene-level key dispatcher. Roku invokes this for every remote
' press while the Scene has focus (which it does by default). On
' OK ("OK" / "select"), we fire the first registered button action.
'
' v0 limitation: with only one button on screen, this works fine.
' Multi-button screens are handled via an internal focus index over
' `m.buttonOrder`. D-pad cycles through (with wrap), OK fires
' whichever button is focused. The visual treatment of focus comes
' from the framework's `state hovered { ... }` stylesheet overlay,
' shipped through `apply_styled_states` and re-applied on every
' focus change by `applyButtonFocusVisuals`.
function onKeyEvent(key as string, press as boolean) as boolean
    if not press then return false
    routing = focusedItemRouting()
    if routing = "list-vertical" then
        ' MarkupList: native handles up/down/OK; we own left/right
        ' as the "exit the list" gesture.
        if key = "up" or key = "down" or key = "OK" or key = "select" or key = "play" then
            return false
        end if
        if key = "right" then
            moveFocus(1)
            return true
        else if key = "left" then
            moveFocus(-1)
            return true
        end if
        return false
    else if routing = "list-horizontal" then
        ' RowList carousel: native handles left/right/OK; we own
        ' up/down as the exit gesture.
        if key = "left" or key = "right" or key = "OK" or key = "select" or key = "play" then
            return false
        end if
        if key = "down" then
            moveFocus(1)
            return true
        else if key = "up" then
            moveFocus(-1)
            return true
        end if
        return false
    end if
    if key = "OK" or key = "select" or key = "play" then
        fireFocusedButton()
        return true
    else if key = "right" or key = "down" then
        moveFocus(1)
        return true
    else if key = "left" or key = "up" then
        moveFocus(-1)
        return true
    end if
    return false
end function

sub materializeFromJson()
    json = ReadAsciiFile("pkg:/data/ui.json")
    if json = invalid or Len(json) = 0 then
        ? "[idealyst] no pkg:/data/ui.json found"
        return
    end if
    commands = ParseJson(json)
    if commands = invalid then
        ? "[idealyst] failed to parse pkg:/data/ui.json"
        return
    end if
    if type(commands) <> "roArray" then
        ? "[idealyst] ui.json must be a JSON array; got "; type(commands)
        return
    end if

    for each cmd in commands
        applyCommand(cmd)
    end for

    ? "[idealyst] materialized "; commands.Count(); " commands"

    ' All buttons are registered by now (`BindButton` commands ran
    ' during the loop above and pushed each button id into
    ' `m.buttonOrder`). Seed the focus state and paint the initial
    ' state-overlay visuals.
    focusInitial()
end sub

sub applyCommand(cmd as object)
    op = cmd.op
    if op = "CreateView" then
        ' Plain Group — no LayoutGroup native stacking. Position
        ' and size of children are driven by Layout.brs's flex pass.
        node = createObject("roSGNode", "Group")
        registerNode(cmd.id, node, "View")
    else if op = "CreateText" then
        node = createObject("roSGNode", "Label")
        node.text = cmd.content
        node.color = "0xFFFFFFFF"
        ' Default font size suited to TV viewing distance. The
        ' user's stylesheet can override via ApplyStyle.font_size.
        defaultFont = createObject("roSGNode", "Font")
        defaultFont.size = 36
        node.font = defaultFont
        registerNode(cmd.id, node, "Text")
    else if op = "CreateButton" then
        ' We don't use Roku's `Button` SGNode — it's a small-menu
        ' widget that renders as text plus a bullet by default, and
        ' it needs focus-bitmap assets to look like an actual button
        ' on a TV. Compose our own: a Group containing a Rectangle
        ' (background) and a Label (centered text). Layout.brs sizes
        ' both inner nodes to the Group's allocated frame.
        node = createObject("roSGNode", "Group")
        bg = createObject("roSGNode", "Rectangle")
        bg.color = "0x2563EBFF"
        node.appendChild(bg)
        labelNode = createObject("roSGNode", "Label")
        labelNode.text = cmd.label
        labelNode.color = "0xFFFFFFFF"
        labelNode.horizAlign = "center"
        labelNode.vertAlign = "center"
        defaultFont = createObject("roSGNode", "Font")
        defaultFont.size = 40
        labelNode.font = defaultFont
        node.appendChild(labelNode)
        ' Stash references so Layout.brs can size them and ApplyStyle
        ' can recolor the bg / restyle the label.
        m.buttonBgs[cmd.id.ToStr()] = bg
        m.buttonLabels[cmd.id.ToStr()] = labelNode
        registerNode(cmd.id, node, "Button")
    else if op = "CreateScrollView" then
        ' Treat as a flex container for now. Real scrolling needs a
        ' MarkupGrid / RowList with an itemComponent — out of scope
        ' for the initial layout pass.
        node = createObject("roSGNode", "Group")
        registerNode(cmd.id, node, "ScrollView")
    else if op = "CreateImage" then
        node = createObject("roSGNode", "Poster")
        node.uri = cmd.src
        registerNode(cmd.id, node, "Image")
    else if op = "CreateActivityIndicator" then
        node = createObject("roSGNode", "BusySpinner")
        registerNode(cmd.id, node, "ActivityIndicator")
    else if op = "CreateReactiveAnchor" then
        ' Layout-transparent stub. Treat as a View; reactivity will
        ' swap its children later.
        node = createObject("roSGNode", "Group")
        registerNode(cmd.id, node, "View")
    else if op = "Insert" then
        parent = getNode(cmd.parent)
        child = getNode(cmd.child)
        if parent <> invalid and child <> invalid then
            parent.appendChild(child)
            recordParentChild(cmd.parent, cmd.child)
        end if
    else if op = "ClearChildren" then
        parent = getNode(cmd.parent)
        if parent <> invalid then
            parent.removeChildrenIndex(parent.getChildCount(), 0)
            m.children[cmd.parent.ToStr()] = createObject("roArray", 4, true)
        end if
    else if op = "UpdateText" then
        node = getNode(cmd.id)
        if node <> invalid then node.text = cmd.content
    else if op = "UpdateButtonLabel" then
        node = getNode(cmd.id)
        if node <> invalid then node.text = cmd.label
    else if op = "UpdateImageSrc" then
        node = getNode(cmd.id)
        if node <> invalid then node.uri = cmd.src
    else if op = "ApplyStyle" then
        node = getNode(cmd.id)
        if node <> invalid and cmd.style <> invalid then
            m.styles[cmd.id.ToStr()] = cmd.style
            applyNonLayoutStyle(cmd.id, node, cmd.style)
        end if
    else if op = "ApplyStyleStates" then
        ' Same as ApplyStyle but also records per-state overlays.
        ' Focus navigation (driven by D-pad onKeyEvent) re-merges
        ' base + the relevant overlay and re-applies. Mirrors how
        ' the web backend lets CSS :hover / :focus do this natively.
        node = getNode(cmd.id)
        if node <> invalid and cmd.base <> invalid then
            idStr = cmd.id.ToStr()
            m.styles[idStr] = cmd.base
            m.styleOverlays[idStr] = {
                hovered: cmd.hovered,
                focused: cmd.focused,
                pressed: cmd.pressed,
                disabled: cmd.disabled
            }
            applyNonLayoutStyle(cmd.id, node, cmd.base)
        end if
    else if op = "CreateSignal" then
        ' `initial` came through serde_json::Value, so it could be
        ' a number, bool, string, etc. — pass straight through.
        signalCreate(cmd.id, cmd.initial)
    else if op = "BindText" then
        ' signal_ids comes through as a JSON array; ParseJson
        ' returns it as an roArray of integers.
        bindText(cmd.node_id, cmd.signal_ids, cmd.method)
    else if op = "BindWhen" then
        ' Reactive if/else. Each branch carries its own subtree as
        ' a `Slot` (root_node_id + list of construction commands).
        ' Subscribers play the active branch and tear down the
        ' previous one on every signal change — inactive subtrees
        ' never materialize on the device.
        bindWhen(cmd.anchor_id, cmd.signal_ids, cmd.cond_method, cmd.then_slot, cmd.otherwise_slot)
    else if op = "BindSwitch" then
        ' N-way structural reactivity. Each arm + the default ships
        ' as a `Slot`; only the matching one is materialized.
        bindSwitch(cmd.anchor_id, cmd.signal_ids, cmd.cond_method, cmd.arms, cmd.default_slot)
    else if op = "BindRepeat" then
        ' Reactive unbounded list. The wire carries one row `Slot`
        ' as a template; the runtime clones it per row with fresh
        ' node ids and tears down clones when `count` shrinks.
        ' `row_index_signal_id` (Option<u64> from Rust → null/int
        ' in JSON → invalid/integer in BS) names the synthetic
        ' signal id the closure's `i` parameter bound to at
        ' snapshot; if set, per-clone signal substitution makes
        ' the row's `i` resolve to its actual row index.
        bindRepeat(cmd.anchor_id, cmd.signal_ids, cmd.count_method, cmd.row_template, cmd.row_index_signal_id)
    else if op = "BindButton" then
        ' output_signal_id is Option<u64>; in JSON it's either a
        ' number or null. ParseJson maps null → invalid, so we
        ' pass it through; bindButton treats invalid as "no
        ' output signal" (the action is fire-and-forget).
        bindButton(cmd.button_id, cmd.input_signal_ids, cmd.method, cmd.output_signal_id)
    else if op = "CreateMarkupList" then
        ' Native windowed list. Queue the op rather than running
        ' it now — `ApplyStyleStates` on the anchor lands AFTER
        ' `CreateMarkupList` in the wire stream (the walker
        ' calls `attach_style` after `build_virtualizer_declarative`
        ' returns), so any style patches we make here would be
        ' clobbered by the subsequent assignment. The Finish op
        ' drains this queue before running layout.
        if m.pendingMarkupLists = invalid then
            m.pendingMarkupLists = createObject("roArray", 4, true)
        end if
        m.pendingMarkupLists.Push(cmd)
    else if op = "RegisterThemeVariant" then
        ' Register a named theme variant. Each variant's tokens are
        ' an array of `{ name, value: { kind, value } }` AAs; flatten
        ' to a name → value-AA map for O(1) lookup at style-apply
        ' time.
        tokenMap = createObject("roAssociativeArray")
        for each tok in cmd.tokens
            tokenMap[tok.name] = tok.value
        end for
        m.themes[cmd.name] = tokenMap
    else if op = "BindActiveThemeSignal" then
        ' Bind the active-theme-name signal. Seeds the initial
        ' theme and registers a subscriber that re-applies every
        ' styled node on signal change so the device can react to
        ' theme-toggle presses without rebuilding the tree.
        m.activeTheme = cmd.initial_name
        m.activeThemeSignalId = cmd.signal_id
        sub_ = {
            kind: "theme",
            signal_ids: [cmd.signal_id],
            signal_id: cmd.signal_id
        }
        signalSubscribe(cmd.signal_id, sub_)
    else if op = "Finish" then
        rootId = cmd.root
        root = getNode(rootId)
        if root <> invalid then
            rootGroup = m.top.findNode("rootGroup")
            if rootGroup <> invalid then rootGroup.appendChild(root)
        end if
        ' Stash the root id so the reactivity engine can re-run
        ' layout after every signal mutation (text contents change
        ' size → frames need recompute).
        m.rootId = rootId
        ' Drain deferred virtualizer setup. Each queued
        ' CreateMarkupList patches its anchor's style with the
        ' viewport's width/height; doing it now (after every
        ' ApplyStyleStates has run) means the patches survive
        ' into the layout pass.
        if m.pendingMarkupLists <> invalid then
            for each pending in m.pendingMarkupLists
                createMarkupList(pending)
            end for
            m.pendingMarkupLists = invalid
        end if
        runLayout(rootId)
    else
        ? "[idealyst] TODO unhandled op: "; op
    end if
end sub

' --- Side-table bookkeeping ---

sub registerNode(id as object, node as object, kind as string)
    idStr = id.ToStr()
    m.nodes[idStr] = node
    m.nodeKinds[idStr] = kind
    m.children[idStr] = createObject("roArray", 4, true)
end sub

function getNode(id as object) as object
    return m.nodes[id.ToStr()]
end function

sub recordParentChild(parentId as object, childId as object)
    pStr = parentId.ToStr()
    cStr = childId.ToStr()
    arr = m.children[pStr]
    if arr = invalid then
        arr = createObject("roArray", 4, true)
        m.children[pStr] = arr
    end if
    arr.Push(childId)
    m.parents[cStr] = parentId
end sub

' ----------------------------------------------------------------
' Layout invocation. Called once after Finish attaches the root.
' Uses the FHD design resolution (1920x1080) minus the rootGroup's
' 80px chrome offset. HD scenes (720p) would need a scaling pass
' here — TODO once we support multi-resolution manifests.
' ----------------------------------------------------------------

sub runLayout(rootId as object)
    if rootId = invalid then return
    ' Match the rootGroup translation in IdealystScene.xml: 80px on
    ' each side. (Stays in sync via convention; if you change one,
    ' change the other.)
    chrome = 80
    designW = 1920
    designH = 1080
    availW = designW - 2 * chrome
    availH = designH - 2 * chrome
    layoutComputeFrames(rootId, availW * 1.0, availH * 1.0)
end sub

' ----------------------------------------------------------------
' Style application — only the non-layout fields. Layout-affecting
' props (sizing, flex, padding, margin, gap) are stored verbatim in
' m.styles and consumed by Layout.brs.
' ----------------------------------------------------------------

sub applyNonLayoutStyle(id as object, node as object, style as object)
    ' WireColor is a transparent serde newtype around String, so the
    ' JSON for `color` / `background` is just a bare string
    ' ("#FFCC00"), not `{"value": "#FFCC00"}`. Same for the rest of
    ' this function.
    if style.color <> invalid then
        c = colorFromString(resolveWireColor(style.color))
        if hasField(node, "color") then node.color = c
    end if

    if style.font_size <> invalid then
        if hasField(node, "font") then
            font = createObject("roSGNode", "Font")
            font.size = style.font_size
            node.font = font
        end if
    end if

    if style.text_align <> invalid then
        if hasField(node, "horizAlign") then
            align = style.text_align
            if align = "Left" then node.horizAlign = "left"
            if align = "Center" then node.horizAlign = "center"
            if align = "Right" then node.horizAlign = "right"
        end if
    end if

    if style.opacity <> invalid then
        node.opacity = style.opacity
    end if

    ' --- Background. Group has no fill; we lazily compose a
    ' Rectangle as the View's first child (rendered behind real
    ' content). Layout.brs syncs its size to the host's frame.
    if style.background <> invalid then
        bgColor = colorFromString(resolveWireColor(style.background))
        idStr = id.ToStr()
        bg = m.backgrounds[idStr]
        if bg = invalid then
            bg = createObject("roSGNode", "Rectangle")
            bg.color = bgColor
            ' Front-of-list insertion. Real children render on top.
            node.insertChild(bg, 0)
            m.backgrounds[idStr] = bg
        else
            bg.color = bgColor
        end if
    end if

    ' font_weight, border_*_radius, transforms — TODO. Roku has no
    ' typeface-weight axis without a custom font URI; per-corner
    ' radii aren't exposed (only uniform `borderRadius` on
    ' Rectangle/Poster).
end sub

' Roku exposes no "does this node have field X?" intrinsic; ask for
' the field list and check membership.
function hasField(node as object, fieldName as string) as boolean
    fields = node.getFields()
    return fields.DoesExist(fieldName)
end function

' --- Color helpers ---
' Rust ships colors as CSS strings ("#rrggbb" / "#rrggbbaa").
' Roku wants the "0xRRGGBBAA" form.

function colorFromString(s as string) as string
    if Len(s) = 0 then return "0xFFFFFFFF"
    if Left(s, 1) = "#" then
        hex = Mid(s, 2)
        if Len(hex) = 6 then hex = hex + "FF"
        if Len(hex) = 8 then return "0x" + UCase(hex)
    end if
    return "0xFFFFFFFF"
end function

' Resolve a wire color AA (`{ kind: "Literal", value: "#..." }` or
' `{ kind: "Token", name: "...", fallback: "#..." }`) to a CSS-style
' string. Tokens look up the named entry in the active theme's
' color tokens; missing tokens fall back to the literal that
' shipped with the wire op. Returns "" only if the wire payload
' is malformed (kind missing).
function resolveWireColor(wc as object) as string
    if wc = invalid then return ""
    if wc.kind = "Literal" then return wc.value
    if wc.kind = "Token" then
        themes = m.themes
        if themes <> invalid then
            tokens = themes[m.activeTheme]
            if tokens <> invalid then
                t = tokens[wc.name]
                if t <> invalid and t.kind = "Color" then
                    return t.value
                end if
            end if
        end if
        return wc.fallback
    end if
    return ""
end function
