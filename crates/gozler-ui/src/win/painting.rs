#![cfg(test)]

use std::path::Path;

/// Windows draws the window (AGENTS.md: Owner ruling): a call that paints,
/// or that reaches a painting helper of this crate, may stand only in a
/// function that [`allowed`] lists, so a new painted surface fails the build
/// instead of waiting for a reviewer to find it.
#[test]
fn window_code_paints_only_where_the_owner_allowed() {
    let failures = misplaced(|code, file, function| {
        !PAINTING.iter().any(|call| code.contains(call)) || allowed(file, function)
    });
    assert!(
        failures.is_empty(),
        "painting outside the owner's allow-list (AGENTS.md: Windows draws the \
         window); use the native control, or ask the owner and add the function \
         to `allowed`:\n{}",
        failures.join("\n")
    );
}

/// One owner decides when the main window's pixels appear (`win::present`):
/// a call that paints at once, or a native setter that paints its control
/// inside the message, may stand only where [`presents`] allows it, so a
/// new forced paint, or a setter outside the wrapper that marks it, fails
/// the build.
#[test]
fn only_the_presenter_decides_when_the_window_paints() {
    let failures = misplaced(|code, file, function| {
        PRESENTING
            .iter()
            .filter(|call| code.contains(*call))
            .all(|call| presents(call, file, function))
    });
    assert!(
        failures.is_empty(),
        "a forced paint or a setter that paints at once outside `win::present`'s \
         wrappers; mark the change with `win::present::mark_after` in its `win` \
         wrapper and add the wrapper to `presents`:\n{}",
        failures.join("\n")
    );
}

/// The lines of the crate's code, outside tests, comments and imports,
/// that `fits` refuses, given the line without whitespace, its file under
/// `src` and the function it stands in.
fn misplaced(fits: impl Fn(&str, &str, &str) -> bool) -> Vec<String> {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("src");
    let mut folders = vec![root.clone()];
    let mut failures = Vec::new();
    while let Some(folder) = folders.pop() {
        for entry in std::fs::read_dir(folder).expect("source folders are readable") {
            let path = entry.expect("source entries are readable").path();
            if path.is_dir() {
                folders.push(path);
                continue;
            }
            if path.extension().is_none_or(|extension| extension != "rs") {
                continue;
            }
            let source = std::fs::read_to_string(&path).expect("Rust source is UTF-8");
            if source.starts_with("#![cfg(test)]") {
                continue;
            }
            let path = path.strip_prefix(&root).expect("source stays under src");
            let name = path.to_string_lossy().replace('\\', "/");
            let mut test_item = None;
            let mut in_use = false;
            let mut function = String::new();
            for (index, line) in source.lines().enumerate() {
                let text = line.trim();
                let indent = line.len() - line.trim_start().len();
                if text == "#[cfg(test)]" {
                    test_item = Some(indent);
                    continue;
                }
                if let Some(start) = test_item {
                    if indent == start && (text == "}" || text.ends_with(';')) {
                        test_item = None;
                    }
                    continue;
                }
                if text.starts_with("//") {
                    continue;
                }
                // An import names a call without making it.
                if text.starts_with("use ") || text.starts_with("pub(crate) use ") {
                    in_use = true;
                }
                if in_use {
                    in_use = !text.ends_with(';');
                    continue;
                }
                if let Some((_, after)) = text.split_once("fn ") {
                    function = after.split(['(', '<']).next().unwrap_or("").to_owned();
                }
                let code: String = text
                    .split("//")
                    .next()
                    .unwrap_or("")
                    .chars()
                    .filter(|character| !character.is_whitespace())
                    .collect();
                if !fits(&code, &name, &function) {
                    failures.push(format!("{name}:{} ({function}): {text}", index + 1));
                }
            }
        }
    }
    failures
}

/// Win32 calls and styles that paint, then this crate's own ways to reach
/// them: `draw_text` and `ImageList::draw` wrap two of them, and
/// `DrawAnswer::Drawn` tells a list that the window drew the cell.
const PAINTING: &[&str] = &[
    "WM_PAINT",
    "BeginPaint(",
    "WM_DRAWITEM",
    "WM_MEASUREITEM",
    "OWNERDRAW",
    "CDRF_SKIPDEFAULT",
    "DrawThemeBackground",
    "DrawThemeText",
    "DrawThemeEdge",
    "DrawThemeIcon",
    "ExtTextOutW",
    "DrawTextW",
    "DrawTextExW",
    "TextOutW",
    "FillRect",
    "FrameRect",
    "FillRgn",
    "PaintRgn",
    "Rectangle(",
    "RoundRect(",
    "Ellipse(",
    "LineTo(",
    "Polygon(",
    "Polyline(",
    "GradientFill",
    "BitBlt",
    "StretchBlt",
    "AlphaBlend",
    "PatBlt",
    "BeginBufferedPaint",
    "DrawFocusRect",
    "DrawEdge",
    "DrawFrameControl",
    "DrawIcon(",
    "DrawIconEx",
    "ImageList_Draw",
    "ID2D1",
    "IDWrite",
    "draw_text(",
    ".draw(",
    "DrawAnswer::Drawn",
];

/// The functions that may paint, each with the reason it may.
fn allowed(file: &str, function: &str) -> bool {
    match file {
        // Measuring text and turning a glyph into the bitmap of a native
        // button's or menu's icon paint nothing on screen; `draw_text` and
        // `draw` are the helpers the surfaces below paint through.
        "win/gdi.rs" => matches!(
            function,
            "draw_text" | "width" | "wrapped_height" | "plain_width" | "glyph_coverage" | "draw"
        ),
        // The owner approved two surfaces in the lists. Search's Name and
        // Path cells are drawn with the matched parts in bold as Everything
        // does (`draw_cell`, `draw_icon`), because a list view draws a cell
        // in one font. A selected row gets the light blue fill behind its
        // icon and the icon on it (`fill`, `draw_icon_on`), which a list
        // view draws only in the "Explorer" visual style, inside a border
        // around the row; the focused row gets a lighter one (owner request,
        // 2026-10-28), which a list view does not draw. The focused row's
        // focus rectangle (`draw_focus`) covers the whole row, because a
        // list view draws it only over the cells it drew itself.
        "win/notify.rs" => matches!(
            function,
            "answer" | "draw_cell" | "draw_icon" | "fill" | "draw_icon_on" | "draw_focus"
        ),
        "app/main_window/search.rs" => function == "draw_cell",
        // The Viewer's text and hex area, which reads only the visible part
        // of a file of any size, because an edit control holds all its text
        // in memory. The owner approved it.
        "win/viewer.rs" => matches!(function, "area_procedure" | "paint_area"),
        // The presenter's cover copies the main window's client as Windows
        // painted it, so a paint pass of several controls, which takes
        // longer than a frame, appears in one step. No native control
        // holds the old picture. The owner approved it.
        "win/present.rs" => function == "copy_client",
        _ => false,
    }
}

/// Calls that paint at once: the forced paints, and the native setters
/// that paint their control inside the message instead of marking it. A
/// native message that does so and is missing here shows in a recording
/// or a paint trace, and then joins the list.
const PRESENTING: &[&str] = &[
    "RDW_UPDATENOW",
    "UpdateWindow(",
    "WM_SETREDRAW",
    "SetWindowTextW",
    "EnableWindow",
    "SetFocus",
    "STM_SETIMAGE",
    "BM_SETCHECK",
    "PBM_SETPOS",
    "PBM_SETRANGE32",
    "PBM_SETMARQUEE",
    "PBM_SETSTATE",
    "CB_SETCURSEL",
    "EM_SETSEL",
    "SB_SETTEXTW",
    "SB_SETPARTS",
    "SB_SETICON",
    "TB_CHECKBUTTON",
    "TB_ENABLEBUTTON",
    "HDM_SETITEMW",
    "SetScrollInfo(",
    "LVM_ENSUREVISIBLE",
    "LVM_SETITEMCOUNT",
    "LVM_SCROLL",
    "WM_VSCROLL",
];

/// Whether `call` of [`PRESENTING`] may stand in `function` of `file`,
/// each with the reason it may.
fn presents(call: &str, file: &str, function: &str) -> bool {
    match call {
        // The presenter's one paint pass when an update ends, the cover's
        // own paint while it is cloaked, and a list's scroll, which copies
        // its rows on the screen at once.
        "RDW_UPDATENOW" | "UpdateWindow(" => matches!(
            (file, function),
            ("win/present.rs", "publish" | "raise_cover" | "scroll_now")
        ),
        // `mark_after` turns drawing off on one control around a setter,
        // and `hold_after` on a list or a tree until the update ends; both
        // through `set_drawing`.
        "WM_SETREDRAW" => matches!((file, function), ("win/present.rs", "set_drawing")),
        // The window procedures that receive the message, which send nothing.
        "WM_VSCROLL"
            if matches!(
                (file, function),
                ("win/window.rs", "decode") | ("win/pane.rs", "procedure")
            ) =>
        {
            true
        }
        // The `win` wrappers that run the setter inside `mark_after`, and the
        // Viewer's area, which takes the focus on a click and draws no focus
        // cue.
        _ => matches!(
            (file, function),
            ("win/mod.rs", "set_text" | "focus")
                | (
                    "win/controls.rs",
                    "set_enabled"
                        | "select_all"
                        | "set_text_caret_at_end"
                        | "select_to_end"
                        | "select"
                        | "show_file_icon"
                        | "show_glyph"
                        | "set_checked"
                        | "set_moving"
                        | "set_measured"
                        | "set_rows"
                )
                | (
                    "win/controls/status_bar.rs",
                    "set_parts" | "set_text_with_border" | "set_icon"
                )
                | (
                    "win/controls/list_view.rs",
                    "show_sort"
                        | "set_count"
                        | "ensure_visible"
                        | "scroll_row"
                        | "insert_column"
                        | "set_header"
                        | "set_column_width"
                        | "delete_column"
                )
                | ("win/toolbar.rs", "set_checked" | "set_enabled")
                | ("win/pane.rs", "set_vertical_scroll")
                | ("win/present.rs", "send_image")
                | (
                    "win/file_operations.rs",
                    "show_paused" | "ensure_visible" | "scroll_toward"
                )
                | ("win/viewer.rs", "area_procedure" | "set_area_scroll")
        ),
    }
}
