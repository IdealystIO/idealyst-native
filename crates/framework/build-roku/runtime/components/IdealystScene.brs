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
    materializeFromJson()
end sub

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
        node = createObject("roSGNode", "Button")
        node.text = cmd.label
        ' Stash the handler id so the (forthcoming) reactivity pass
        ' can wire `buttonSelected` → handler dispatch.
        node.addField("idealystHandler", "integer", false)
        node.idealystHandler = cmd.on_click
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
    else if op = "Finish" then
        rootId = cmd.root
        root = getNode(rootId)
        if root <> invalid then
            rootGroup = m.top.findNode("rootGroup")
            if rootGroup <> invalid then rootGroup.appendChild(root)
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
        c = colorFromString(style.color)
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
        bgColor = colorFromString(style.background)
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
