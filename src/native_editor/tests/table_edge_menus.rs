// Modified for gpui-kit-block-editor: incorporated gpui-kit-markdown-editor tests
// (Apache-2.0), relocated into the private native_editor module.

//! Mouse-path coverage for the table edge icons and their rendered menus.

use super::*;
use crate::native_editor::TableLine;
use gpui::{Bounds, Point, point};

const TABLE: &str = "| a | b |\n| --- | --- |\n| 1 | 2 |\n| 3 | 4 |";

fn click_edge(
    editor: &Entity<BlockEditorState>,
    cx: &mut VisualTestContext,
    line: TableLine,
) -> Bounds<Pixels> {
    let probe = editor.read_with(cx, |e, _| {
        let frame = e.frame.borrow();
        let grid = &frame.grids[0];
        let (row, col) = match line {
            TableLine::Column(col) => (1, col),
            TableLine::Row(row) => (row, 0),
        };
        point(
            (grid.cols[col].0 + grid.cols[col].1) / 2.,
            (grid.rows[row].0 + grid.rows[row].1) / 2.,
        )
    });
    cx.simulate_mouse_move(probe, None, Modifiers::none());
    draw(cx);
    let control = editor.read_with(cx, |e, _| {
        e.table_grips_at(probe)
            .into_iter()
            .find(|(_, candidate, _)| *candidate == line)
            .expect("hover reveals the requested edge icon")
            .2
    });
    assert_eq!(control.size.width, px(18.));
    assert!(control.size.height >= px(14.));
    // Travel from the cell to the icon through the gutter, rather than
    // clicking an unhovered geometry-only target.
    let halfway = point(
        (probe.x + control.center().x) / 2.,
        (probe.y + control.center().y) / 2.,
    );
    cx.simulate_mouse_move(halfway, None, Modifiers::none());
    draw(cx);
    cx.simulate_mouse_move(control.center(), None, Modifiers::none());
    draw(cx);
    editor.read_with(cx, |e, _| {
        assert_eq!(e.table_grip_hit(control.center()).unwrap().1, line);
        assert!(e.table_handle_at(control.center()).is_none());
    });
    cx.simulate_click(control.center(), Modifiers::none());
    draw(cx);
    editor.read_with(cx, |e, _| {
        assert_eq!(e.table_menu.as_ref().unwrap().line, line);
        assert!(matches!(e.model.selection, BlockSelection::Cells { .. }));
        assert!(
            !e.dragging,
            "an icon click must not start selecting table text"
        );
    });
    assert!(cx.debug_bounds("block-editor-table-menu").is_some());
    control
}

fn click_item(cx: &mut VisualTestContext, selector: &'static str) {
    let bounds = cx
        .debug_bounds(selector)
        .expect("rendered table menu entry");
    cx.simulate_mouse_move(bounds.center(), None, Modifiers::none());
    draw(cx);
    cx.simulate_click(bounds.center(), Modifiers::none());
    draw(cx);
}

#[gpui::test]
fn table_edge_column_menu_mouse_edits_emit_changes_and_undo(cx: &mut TestAppContext) {
    let (editor, cx) = open(cx, TABLE);
    let changed = events(&editor, cx);
    let table = editor.read_with(cx, |e, _| e.frame.borrow().grids[0].table);
    for (entry, expected) in [
        (
            "tm-col-left",
            "| a |  | b |\n| --- | --- | --- |\n| 1 |  | 2 |\n| 3 |  | 4 |",
        ),
        (
            "tm-col-right",
            "| a | b |  |\n| --- | --- | --- |\n| 1 | 2 |  |\n| 3 | 4 |  |",
        ),
        ("tm-col-delete", "| a |\n| --- |\n| 1 |\n| 3 |"),
    ] {
        click_edge(&editor, cx, TableLine::Column(1));
        let before = *changed.borrow();
        click_item(cx, entry);
        assert_eq!(source(&editor, cx), expected);
        assert_eq!(
            *changed.borrow(),
            before + 1,
            "one model change per menu action"
        );
        assert!(!editor.read_with(cx, |e, _| e.table_menu_open()));
        assert!(cx.debug_bounds("block-editor-table-menu").is_none());
        assert_eq!(
            editor.read_with(cx, |e, _| e.frame.borrow().grids[0].table),
            table
        );
        keys(cx, "M-z");
        assert_eq!(source(&editor, cx), TABLE, "one undo restores the table");
        keys(cx, "M-shift-z");
        assert_eq!(source(&editor, cx), expected);
        keys(cx, "M-z");
    }
}

#[gpui::test]
fn table_edge_row_menu_mouse_edits_emit_changes_and_undo(cx: &mut TestAppContext) {
    let (editor, cx) = open(cx, TABLE);
    let changed = events(&editor, cx);
    for (entry, expected) in [
        (
            "tm-row-above",
            "| a | b |\n| --- | --- |\n|  |  |\n| 1 | 2 |\n| 3 | 4 |",
        ),
        (
            "tm-row-below",
            "| a | b |\n| --- | --- |\n| 1 | 2 |\n|  |  |\n| 3 | 4 |",
        ),
        ("tm-row-delete", "| a | b |\n| --- | --- |\n| 3 | 4 |"),
    ] {
        click_edge(&editor, cx, TableLine::Row(1));
        let before = *changed.borrow();
        click_item(cx, entry);
        assert_eq!(source(&editor, cx), expected);
        assert_eq!(*changed.borrow(), before + 1);
        assert!(!editor.read_with(cx, |e, _| e.table_menu_open()));
        keys(cx, "M-z");
        assert_eq!(source(&editor, cx), TABLE);
    }
}

#[gpui::test]
fn table_edge_menu_disabled_padding_escape_and_outside_click(cx: &mut TestAppContext) {
    let original = format!("{TABLE}\n\nafter");
    let (editor, cx) = open(cx, &original);
    click_edge(&editor, cx, TableLine::Row(0));
    let selection = editor.read_with(cx, |e, _| e.model.selection.clone());
    click_item(cx, "tm-row-delete");
    let menu = cx.debug_bounds("block-editor-table-menu").unwrap();
    cx.simulate_click(
        point(menu.left() + px(2.), menu.top() + px(2.)),
        Modifiers::none(),
    );
    draw(cx);
    editor.read_with(cx, |e, _| {
        assert!(
            e.table_menu_open(),
            "disabled entries and menu padding do not dismiss it"
        );
        assert_eq!(
            e.model.selection, selection,
            "popover clicks cannot reach table text"
        );
        assert!(!e.dragging);
    });
    assert_eq!(source(&editor, cx), original);
    let focus = editor.read_with(cx, |e, _| e.focus_handle.clone());
    assert!(cx.update(|window, _| focus.is_focused(window)));
    keys(cx, "escape");
    assert!(!editor.read_with(cx, |e, _| e.table_menu_open()));
    assert!(cx.debug_bounds("block-editor-table-menu").is_none());
    click_edge(&editor, cx, TableLine::Column(0));
    let after = editor.read_with(cx, |e, _| {
        let bounds = e.frame.borrow().leaves.last().unwrap().bounds();
        point(bounds.left() + px(1.), bounds.center().y)
    });
    cx.simulate_click(after, Modifiers::none());
    draw(cx);
    assert!(!editor.read_with(cx, |e, _| e.table_menu_open()));
    assert!(editor.read_with(cx, |e, _| matches!(
        e.model.selection,
        BlockSelection::Text { .. }
    )));
    assert_eq!(source(&editor, cx), original);
}

#[gpui::test]
fn table_edge_icons_hide_off_table_and_keep_open_menu_control(cx: &mut TestAppContext) {
    let (editor, cx) = open(cx, TABLE);
    let (corner, away) = editor.read_with(cx, |e, _| {
        let frame = e.frame.borrow();
        let bounds = frame.grids[0].bounds;
        let corner = e.table_handles()[0].1;
        assert!(corner.left() >= px(0.) && corner.top() >= px(0.));
        assert!(e.table_grip_hit(corner.center()).is_none());
        (
            corner,
            point(bounds.right() + px(60.), bounds.bottom() + px(60.)),
        )
    });
    cx.simulate_mouse_move(away, None, Modifiers::none());
    draw(cx);
    assert!(editor.read_with(cx, |e, _| e.table_grips_at(away).is_empty()));
    let control = click_edge(&editor, cx, TableLine::Column(0));
    assert!(
        control.bottom() <= corner.top(),
        "column icon clears the whole-table handle"
    );
    cx.simulate_mouse_move(away, None, Modifiers::none());
    draw(cx);
    assert_eq!(
        editor.read_with(cx, |e, _| e
            .table_grips_at(away)
            .into_iter()
            .map(|(_, line, _)| line)
            .collect::<Vec<_>>()),
        vec![TableLine::Column(0)],
        "the active icon stays visible while the pointer travels to the menu"
    );
    keys(cx, "escape");
    assert!(editor.read_with(cx, |e, _| e.table_grips_at(away).is_empty()));
}

struct MenuHost {
    editor: Entity<BlockEditorState>,
    scroll: gpui::ScrollHandle,
    outer_clicks: std::rc::Rc<std::cell::Cell<usize>>,
}

impl Render for MenuHost {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        let clicks = self.outer_clicks.clone();
        div()
            .id("table-menu-test-scroller")
            .w(px(600.))
            .h(px(220.))
            .overflow_y_scroll()
            .track_scroll(&self.scroll)
            .on_mouse_down(MouseButton::Left, move |_, _, _| {
                clicks.set(clicks.get() + 1)
            })
            .child(BlockEditor::new(&self.editor))
    }
}

fn wheel(cx: &mut VisualTestContext, position: Point<Pixels>) {
    cx.simulate_mouse_move(position, None, Modifiers::none());
    draw(cx);
    cx.simulate_event(gpui::ScrollEvent {
        position,
        delta: gpui::ScrollDelta::Pixels(point(px(0.), px(-80.))),
        ..Default::default()
    });
    draw(cx);
}

#[gpui::test]
fn table_edge_menu_contains_clicks_and_scroll_without_scrolling_document(cx: &mut TestAppContext) {
    cx.update(crate::native_editor::init_for_tests);
    let scroll = gpui::ScrollHandle::new();
    let outer_clicks = std::rc::Rc::new(std::cell::Cell::new(0));
    let host_scroll = scroll.clone();
    let host_clicks = outer_clicks.clone();
    let text = format!("{TABLE}\n\n{}", "after\n\n".repeat(30));
    let (host, cx) = cx.add_window_view(move |window, cx| MenuHost {
        editor: cx.new(|cx| BlockEditorState::new(window, cx).with_source(&text)),
        scroll: host_scroll,
        outer_clicks: host_clicks,
    });
    let editor = host.read_with(cx, |host, _| host.editor.clone());
    draw(cx);
    click_edge(&editor, cx, TableLine::Row(0));
    click_item(cx, "tm-row-delete");
    let menu = cx.debug_bounds("block-editor-table-menu").unwrap();
    cx.simulate_click(
        point(menu.left() + px(2.), menu.top() + px(2.)),
        Modifiers::none(),
    );
    draw(cx);
    assert_eq!(
        outer_clicks.get(),
        0,
        "neither icons nor menu clicks reach the outer editor"
    );
    let before = scroll.offset();
    wheel(cx, menu.center());
    assert_eq!(
        scroll.offset(),
        before,
        "scroll input over the popover is contained"
    );
    assert!(editor.read_with(cx, |e, _| e.table_menu_open()));
    keys(cx, "escape");
    wheel(cx, point(px(500.), px(180.)));
    assert_ne!(
        scroll.offset(),
        before,
        "the underlying document can still scroll outside the menu"
    );
}
