' ============================================================
' Layout.brs — flex layout engine for the Roku runtime.
'
' Mirrors the framework's `native-layout` (Taffy) semantics so a
' Rust-authored UI lays out identically on iOS / Android / Roku
' for the same StyleRules. Because Roku ships no Rust runtime, we
' can't link Taffy — this file is a from-scratch BrightScript port
' of the flex subset the framework's stylesheets actually use.
'
' Algorithm shape (two-pass):
'
'   1. Measure (bottom-up). For each node, compute its "natural"
'      size given the parent's available space. Leaves (Labels)
'      ask the SceneGraph widget via getBoundingRect(). Containers
'      sum their children along the main axis and take the max
'      along the cross axis, then add their own padding. Explicit
'      `width`/`height` overrides natural.
'
'   2. Position and size (top-down). Given a parent allocation,
'      determine each child's main-axis size (explicit, flex_grow
'      distribution, or natural), then cross-axis size (explicit,
'      stretch, or natural). Apply justify_content along the main
'      axis and align_items along the cross axis. Recurse.
'
' Final frames are stored in m.layoutFrames keyed by node id; the
' apply step writes translation / width / height onto the actual
' roSGNodes.
'
' Inputs (must be set by IdealystScene before calling
' layoutComputeFrames):
'
'   m.nodes[idStr]      = roSGNode handle
'   m.styles[idStr]     = AA from the ApplyStyle command's `style`
'                         field (or invalid for no style)
'   m.children[idStr]   = roArray of child node ids (integers)
'   m.nodeKinds[idStr]  = "View" / "Text" / "Button" / ...
'
' Supported (Phase 1):
'   flex_direction       Row, Column
'   justify_content      FlexStart, FlexEnd, Center,
'                        SpaceBetween, SpaceAround, SpaceEvenly
'   align_items          FlexStart, FlexEnd, Center, Stretch
'   gap / row_gap / column_gap
'   flex_grow
'   width / height       Px or Percent
'   min_width / max_width / min_height / max_height
'   padding_top/right/bottom/left
'   margin_top/right/bottom/left
'
' Not yet supported (TODO):
'   flex_direction       RowReverse, ColumnReverse
'   flex_wrap            (and the align_content that goes with it)
'   align_items          Baseline
'   align_self           per-child override
'   flex_shrink          (proper hypothetical-main-size algorithm)
'   flex_basis           (we currently use `width`/`height` as basis)
'   position             Absolute
'   text wrapping        (single-line measurement only)
' ============================================================

' ---- Entry point --------------------------------------------------

' Compute and apply layout to the subtree at `rootId`. `availW` /
' `availH` is the area the root is allowed to occupy. Idempotent —
' safe to call again if the tree changes.
sub layoutComputeFrames(rootId as integer, availW as float, availH as float)
    m.layoutMeasures = createObject("roAssociativeArray")
    m.layoutFrames = createObject("roAssociativeArray")

    ' Pass 1: bottom-up natural sizing.
    layoutMeasure(rootId, availW, availH)

    ' Pass 2: top-down allocate + position. Root gets the full
    ' viewport at origin (0, 0); its own translation comes from the
    ' parent (rootGroup) in the SceneGraph.
    layoutPositionAndSize(rootId, 0.0, 0.0, availW, availH)

    ' Pass 3: write translations + sizes onto the roSGNodes.
    layoutApplyFrames()
end sub

' ---- Pass 1: measure ----------------------------------------------

' Returns an AA `{ width, height }`. Also caches it in
' m.layoutMeasures[idStr] for the position pass.
function layoutMeasure(id as integer, availW as float, availH as float) as object
    idStr = id.ToStr()
    style = layoutStyleOf(id)
    kind = m.nodeKinds[idStr]

    explicitW = layoutResolveLength(style.width, availW)
    explicitH = layoutResolveLength(style.height, availH)

    ' Leaf: Text uses SGNode's reported size; other leaves report
    ' zero (Buttons / Posters etc. respect explicit sizes only
    ' until we add per-kind intrinsics).
    children = m.children[idStr]
    isLeaf = (children = invalid or children.Count() = 0)

    if isLeaf then
        natW = 0
        natH = 0
        if kind = "Text" then
            node = m.nodes[idStr]
            if node <> invalid then
                rect = node.boundingRect()
                if rect <> invalid then
                    natW = rect.width
                    natH = rect.height
                end if
            end if
        end if
        if explicitW <> invalid then natW = explicitW
        if explicitH <> invalid then natH = explicitH
        natW = layoutClamp(natW, style.min_width, style.max_width, availW)
        natH = layoutClamp(natH, style.min_height, style.max_height, availH)
        m.layoutMeasures[idStr] = { width: natW, height: natH }
        return m.layoutMeasures[idStr]
    end if

    ' Container — measure children in our content area.
    paddingT = layoutNum(style, "padding_top", 0.0)
    paddingR = layoutNum(style, "padding_right", 0.0)
    paddingB = layoutNum(style, "padding_bottom", 0.0)
    paddingL = layoutNum(style, "padding_left", 0.0)
    contentW = availW - paddingL - paddingR
    contentH = availH - paddingT - paddingB
    if contentW < 0 then contentW = 0
    if contentH < 0 then contentH = 0

    isRow = (layoutStr(style, "flex_direction", "Column") = "Row")
    gap = layoutGapFor(style, isRow)

    totalMain = 0.0
    maxCross = 0.0
    n = children.Count()
    for i = 0 to n - 1
        childId = children[i]
        cm = layoutMeasure(childId, contentW, contentH)
        cs = layoutStyleOf(childId)
        mainMargin = layoutMarginSum(cs, isRow, true)
        crossMargin = layoutMarginSum(cs, isRow, false)
        if isRow then
            childMain = cm.width + mainMargin
            childCross = cm.height + crossMargin
        else
            childMain = cm.height + mainMargin
            childCross = cm.width + crossMargin
        end if
        totalMain = totalMain + childMain
        if childCross > maxCross then maxCross = childCross
    end for
    if n > 1 then totalMain = totalMain + gap * (n - 1)

    if isRow then
        natW = totalMain + paddingL + paddingR
        natH = maxCross + paddingT + paddingB
    else
        natW = maxCross + paddingL + paddingR
        natH = totalMain + paddingT + paddingB
    end if

    if explicitW <> invalid then natW = explicitW
    if explicitH <> invalid then natH = explicitH
    natW = layoutClamp(natW, style.min_width, style.max_width, availW)
    natH = layoutClamp(natH, style.min_height, style.max_height, availH)

    m.layoutMeasures[idStr] = { width: natW, height: natH }
    return m.layoutMeasures[idStr]
end function

' ---- Pass 2: position + size ---------------------------------------

' Allocates an `allocW × allocH` box at `(x, y)` to `id` and lays
' out its descendants. (x, y) are relative to the parent.
sub layoutPositionAndSize(id as integer, x as float, y as float, allocW as float, allocH as float)
    idStr = id.ToStr()
    m.layoutFrames[idStr] = { x: x, y: y, width: allocW, height: allocH }

    children = m.children[idStr]
    if children = invalid or children.Count() = 0 then return

    style = layoutStyleOf(id)
    paddingT = layoutNum(style, "padding_top", 0.0)
    paddingR = layoutNum(style, "padding_right", 0.0)
    paddingB = layoutNum(style, "padding_bottom", 0.0)
    paddingL = layoutNum(style, "padding_left", 0.0)
    contentW = allocW - paddingL - paddingR
    contentH = allocH - paddingT - paddingB
    if contentW < 0 then contentW = 0
    if contentH < 0 then contentH = 0

    isRow = (layoutStr(style, "flex_direction", "Column") = "Row")
    mainAvail = contentH
    crossAvail = contentW
    if isRow then
        mainAvail = contentW
        crossAvail = contentH
    end if

    gap = layoutGapFor(style, isRow)
    alignItems = layoutStr(style, "align_items", "Stretch")
    justify = layoutStr(style, "justify_content", "FlexStart")

    n = children.Count()

    ' Per-child working state. BS doesn't have structs; we use
    ' parallel arrays indexed by i. Slightly ugly but the cost of
    ' AAs in BS for a frequently-rebuilt structure is real.
    childMain = []        ' allocated main-axis size
    childCross = []       ' allocated cross-axis size
    childMarginMainStart = []
    childMarginMainEnd = []
    childMarginCrossStart = []
    childMarginCrossEnd = []
    childIsGrow = []

    totalFixedMain = 0.0
    totalGrow = 0.0

    ' --- Phase 2a: resolve main-axis sizes ---
    for i = 0 to n - 1
        cs = layoutStyleOf(children[i])

        ms = layoutMarginStartFor(cs, isRow, true)
        me = layoutMarginEndFor(cs, isRow, true)
        cs2s = layoutMarginStartFor(cs, isRow, false)
        cs2e = layoutMarginEndFor(cs, isRow, false)
        childMarginMainStart.Push(ms)
        childMarginMainEnd.Push(me)
        childMarginCrossStart.Push(cs2s)
        childMarginCrossEnd.Push(cs2e)

        explicitMain = invalid
        if isRow then
            explicitMain = layoutResolveLength(cs.width, contentW)
        else
            explicitMain = layoutResolveLength(cs.height, contentH)
        end if

        grow = layoutNum(cs, "flex_grow", 0.0)

        if explicitMain <> invalid then
            childMain.Push(explicitMain)
            childIsGrow.Push(false)
            totalFixedMain = totalFixedMain + explicitMain + ms + me
        else if grow > 0 then
            ' Will be sized in Phase 2b. Reserve margin only.
            childMain.Push(0.0)
            childIsGrow.Push(true)
            totalGrow = totalGrow + grow
            totalFixedMain = totalFixedMain + ms + me
        else
            measure = m.layoutMeasures[children[i].ToStr()]
            if measure = invalid then measure = { width: 0, height: 0 }
            nat = measure.height
            if isRow then nat = measure.width
            childMain.Push(nat)
            childIsGrow.Push(false)
            totalFixedMain = totalFixedMain + nat + ms + me
        end if
    end for

    if n > 1 then totalFixedMain = totalFixedMain + gap * (n - 1)

    ' --- Phase 2b: distribute remaining main-axis space ---
    remaining = mainAvail - totalFixedMain
    if remaining < 0 then remaining = 0
    if totalGrow > 0 then
        per = remaining / totalGrow
        for i = 0 to n - 1
            if childIsGrow[i] then
                cs = layoutStyleOf(children[i])
                grow = layoutNum(cs, "flex_grow", 0.0)
                childMain[i] = per * grow
            end if
        end for
    end if

    ' Clamp each child's main size by its own min/max.
    for i = 0 to n - 1
        cs = layoutStyleOf(children[i])
        if isRow then
            childMain[i] = layoutClamp(childMain[i], cs.min_width, cs.max_width, contentW)
        else
            childMain[i] = layoutClamp(childMain[i], cs.min_height, cs.max_height, contentH)
        end if
    end for

    ' --- Phase 2c: resolve cross-axis sizes ---
    for i = 0 to n - 1
        cs = layoutStyleOf(children[i])
        availCrossChild = crossAvail - childMarginCrossStart[i] - childMarginCrossEnd[i]
        if availCrossChild < 0 then availCrossChild = 0

        explicitCross = invalid
        if isRow then
            explicitCross = layoutResolveLength(cs.height, contentH)
        else
            explicitCross = layoutResolveLength(cs.width, contentW)
        end if

        if explicitCross <> invalid then
            cross = explicitCross
        else if alignItems = "Stretch" then
            cross = availCrossChild
        else
            measure = m.layoutMeasures[children[i].ToStr()]
            if measure = invalid then measure = { width: 0, height: 0 }
            cross = measure.width
            if isRow then cross = measure.height
        end if

        if isRow then
            cross = layoutClamp(cross, cs.min_height, cs.max_height, contentH)
        else
            cross = layoutClamp(cross, cs.min_width, cs.max_width, contentW)
        end if
        childCross.Push(cross)
    end for

    ' --- Phase 2d: justify_content along main axis ---
    usedMain = 0.0
    for i = 0 to n - 1
        usedMain = usedMain + childMain[i] + childMarginMainStart[i] + childMarginMainEnd[i]
    end for
    if n > 1 then usedMain = usedMain + gap * (n - 1)

    freeMain = mainAvail - usedMain
    if freeMain < 0 then freeMain = 0

    startOffset = 0.0
    extraGap = 0.0
    if justify = "FlexEnd" then
        startOffset = freeMain
    else if justify = "Center" then
        startOffset = freeMain / 2.0
    else if justify = "SpaceBetween" then
        if n > 1 then extraGap = freeMain / (n - 1)
    else if justify = "SpaceAround" then
        if n > 0 then
            per = freeMain / n
            startOffset = per / 2.0
            extraGap = per
        end if
    else if justify = "SpaceEvenly" then
        per = freeMain / (n + 1)
        startOffset = per
        extraGap = per
    end if

    childMainPos = []
    cursor = startOffset
    for i = 0 to n - 1
        cursor = cursor + childMarginMainStart[i]
        childMainPos.Push(cursor)
        cursor = cursor + childMain[i] + childMarginMainEnd[i] + gap + extraGap
    end for

    ' --- Phase 2e: align_items along cross axis ---
    childCrossPos = []
    for i = 0 to n - 1
        availCrossChild = crossAvail - childMarginCrossStart[i] - childMarginCrossEnd[i]
        if availCrossChild < 0 then availCrossChild = 0
        freeCross = availCrossChild - childCross[i]
        if freeCross < 0 then freeCross = 0
        offset = 0.0
        if alignItems = "FlexEnd" then
            offset = freeCross
        else if alignItems = "Center" then
            offset = freeCross / 2.0
        end if
        ' Stretch / FlexStart → offset = 0 (already set above).
        childCrossPos.Push(childMarginCrossStart[i] + offset)
    end for

    ' --- Phase 2f: recurse into each child with its frame ---
    for i = 0 to n - 1
        if isRow then
            cx = paddingL + childMainPos[i]
            cy = paddingT + childCrossPos[i]
            cw = childMain[i]
            ch = childCross[i]
        else
            cx = paddingL + childCrossPos[i]
            cy = paddingT + childMainPos[i]
            cw = childCross[i]
            ch = childMain[i]
        end if
        layoutPositionAndSize(children[i], cx, cy, cw, ch)
    end for
end sub

' ---- Pass 3: apply ------------------------------------------------

sub layoutApplyFrames()
    for each key in m.layoutFrames
        f = m.layoutFrames[key]
        node = m.nodes[key]
        kind = m.nodeKinds[key]
        if node <> invalid and f <> invalid then
            node.translation = [f.x, f.y]
            if kind <> "Text" then
                ' Non-Text nodes: take the computed frame.
                ' Text nodes: don't set width/height — Roku's Label
                ' would truncate with "..." if the assigned width
                ' undershoots the rendered glyph run, and
                ' `boundingRect()` is sometimes a hair tighter than
                ' the actual paint. Leaving Labels unconstrained
                ' keeps them legible.
                node.width = f.width
                node.height = f.height
            end if
        end if
        ' Sync the background Rectangle (if this node has one) to
        ' fill its host's frame. The bg is a child of `node`, so its
        ' translation is [0, 0] relative to the host.
        bg = m.backgrounds[key]
        if bg <> invalid and f <> invalid then
            bg.translation = [0, 0]
            bg.width = f.width
            bg.height = f.height
        end if
        ' Buttons are composite Group + Rectangle + Label. Size both
        ' inner pieces to the button's full frame so the bg fills
        ' and the Label centers its text edge-to-edge.
        if kind = "Button" and f <> invalid then
            btnBg = m.buttonBgs[key]
            if btnBg <> invalid then
                btnBg.translation = [0, 0]
                btnBg.width = f.width
                btnBg.height = f.height
            end if
            btnLabel = m.buttonLabels[key]
            if btnLabel <> invalid then
                btnLabel.translation = [0, 0]
                btnLabel.width = f.width
                btnLabel.height = f.height
            end if
        end if
    end for
end sub

' ---- Helpers ------------------------------------------------------

function layoutStyleOf(id as integer) as object
    s = m.styles[id.ToStr()]
    if s = invalid then return {}
    return s
end function

' Read a numeric style field with a fallback. Distinguishes
' invalid (not set) from a legitimate 0 value.
function layoutNum(style as object, field as string, defaultValue as float) as float
    if style = invalid then return defaultValue
    v = style[field]
    if v = invalid then return defaultValue
    t = type(v)
    if t = "roFloat" or t = "Float" or t = "roInteger" or t = "Integer" or t = "roDouble" or t = "Double" or t = "roIntrinsicDouble" then
        return v
    end if
    return defaultValue
end function

function layoutStr(style as object, field as string, defaultValue as string) as string
    if style = invalid then return defaultValue
    v = style[field]
    if v = invalid then return defaultValue
    if type(v) = "roString" or type(v) = "String" then return v
    return defaultValue
end function

' Resolve a WireLength {kind:"Px"|"Percent"|"Auto", value:Number}
' against a parent dimension. Returns invalid if val is missing or
' Auto (caller treats as "use natural").
function layoutResolveLength(val as object, parentSize as float) as object
    if val = invalid then return invalid
    k = ""
    if val.kind <> invalid then k = val.kind
    if k = "Px" then return val.value
    if k = "Percent" then return (val.value / 100.0) * parentSize
    return invalid
end function

' Clamp value by optional min/max WireLengths against a parent size.
function layoutClamp(value as float, minLen as object, maxLen as object, parentSize as float) as float
    if minLen <> invalid then
        mn = layoutResolveLength(minLen, parentSize)
        if mn <> invalid and value < mn then value = mn
    end if
    if maxLen <> invalid then
        mx = layoutResolveLength(maxLen, parentSize)
        if mx <> invalid and value > mx then value = mx
    end if
    return value
end function

' Pick the right gap for the main axis. row_gap / column_gap take
' precedence over `gap` when set on their relevant axis.
function layoutGapFor(style as object, isRow as boolean) as float
    g = layoutNum(style, "gap", 0.0)
    if isRow then
        if style.column_gap <> invalid then g = style.column_gap
    else
        if style.row_gap <> invalid then g = style.row_gap
    end if
    return g
end function

' Per-side margin helpers. `mainAxis` true means the margin in the
' direction of flex_direction; false means cross-axis. `start` vs
' `end` selects which side along that axis.
function layoutMarginStartFor(style as object, isRow as boolean, mainAxis as boolean) as float
    field = "margin_top"
    if mainAxis then
        if isRow then field = "margin_left"
    else
        if isRow then field = "margin_top" else field = "margin_left"
    end if
    return layoutNum(style, field, 0.0)
end function

function layoutMarginEndFor(style as object, isRow as boolean, mainAxis as boolean) as float
    field = "margin_bottom"
    if mainAxis then
        if isRow then field = "margin_right"
    else
        if isRow then field = "margin_bottom" else field = "margin_right"
    end if
    return layoutNum(style, field, 0.0)
end function

function layoutMarginSum(style as object, isRow as boolean, mainAxis as boolean) as float
    return layoutMarginStartFor(style, isRow, mainAxis) + layoutMarginEndFor(style, isRow, mainAxis)
end function
