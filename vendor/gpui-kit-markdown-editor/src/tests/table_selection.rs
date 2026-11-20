use super::*;

#[gpui::test]
fn corner_selection_covers_short_and_aligned_table_frames(cx: &mut TestAppContext) {
    for table_source in [
        "| a | b | c |\n| --- | --- | --- |\n| 1 | 2 | 3 |",
        "| key | description | n |\n| :--- | :---: | ---: |\n| x | a wider middle column | 2 |\n| longer key | tiny | 123 |",
        "| a | b | c |\n| :---: | :---: | :---: |\n| 1 | 22 | 333 |",
        "| a | | c |\n| --- | :---: | ---: |\n| one | | 3 |",
    ] {
        let original = format!("before\n\n{table_source}\n\nafter");
        let (editor, cx) = open(cx, &original);
        let initial_source = source(&editor, cx);
        let (table, over) = editor.read_with(cx, |state, _| {
            let frame = state.frame.borrow();
            let grid = &frame.grids[0];
            (grid.table, grid.bounds.center())
        });
        // Show the hover-only affordance, then use its real mouse handler.
        cx.simulate_mouse_move(over, None, Modifiers::none());
        draw(cx);
        let corner = table_handle_point(&editor, cx);
        cx.simulate_mouse_move(corner, None, Modifiers::none());
        draw(cx);
        cx.simulate_mouse_down(corner, MouseButton::Left, Modifiers::none());
        cx.simulate_mouse_up(corner, MouseButton::Left, Modifiers::none());
        draw(cx);

        editor.read_with(cx, |state, _| {
            let (parent, range) = state
                .model
                .selected_blocks()
                .expect("corner selects blocks");
            let doc = &state.model.doc;
            assert_eq!(&doc.children(parent)[range.clone()], &[table]);
            let frame = state.frame.borrow();
            let grid = frame.grids.iter().find(|grid| grid.table == table).unwrap();
            let painted = frame.selected_block_bounds(doc, parent, range).unwrap();
            assert_eq!(painted, grid.bounds, "{table_source}");
            // The root table fills the width remaining after its control
            // gutter. Independently check the final column's trailing slack.
            let content = frame.content.unwrap();
            assert!(
                painted.left() >= content.left(),
                "{table_source}: {painted:?} vs {content:?}"
            );
            assert!(
                (painted.right() - content.right()).abs() < px(2.),
                "{table_source}: {painted:?} vs {content:?}"
            );
            let cells = crate::table::grid(doc, table);
            for row in &cells {
                let last = *row.last().unwrap();
                let cell = frame.cell_boxes[&last];
                assert!(painted.left() <= cell.left() && painted.right() >= cell.right());
                assert!(painted.top() <= cell.top() && painted.bottom() >= cell.bottom());
            }
            if table_source.starts_with("| a | b | c |\n| ---") {
                let last_text = frame.leaf(*cells[0].last().unwrap()).unwrap().bounds();
                assert!(
                    painted.right() > last_text.right() + px(20.),
                    "selection includes trailing slack, not just text"
                );
            }
        });
        assert_eq!(
            source(&editor, cx),
            initial_source,
            "selection does not edit the table"
        );
    }
}

#[gpui::test]
fn selecting_a_container_includes_its_descendant_table_frame(cx: &mut TestAppContext) {
    let (editor, cx) = open(
        cx,
        "> lead\n>\n> | a | b |\n> | --- | ---: |\n> | 1 | 2 |\n\noutside",
    );
    let quote = editor.read_with(cx, |state, _| {
        state.model.doc.children(state.model.doc.root)[0]
    });
    editor.update(cx, |state, cx| {
        state
            .set_selection(
                BlockSelection::Blocks {
                    anchor: quote,
                    head: quote,
                },
                cx,
            )
            .unwrap();
    });
    draw(cx);
    editor.read_with(cx, |state, _| {
        let doc = &state.model.doc;
        let frame = state.frame.borrow();
        let grid = &frame.grids[0];
        assert!(doc.path(grid.table).contains(&quote));
        let (parent, range) = state.model.selected_blocks().unwrap();
        let painted = frame.selected_block_bounds(doc, parent, range).unwrap();
        let leaves = frame
            .leaves
            .iter()
            .filter(|leaf| doc.path(leaf.info.block).contains(&quote))
            .map(|leaf| leaf.bounds())
            .reduce(|a, b| a.union(&b))
            .unwrap();
        assert_eq!(painted, leaves.union(&grid.bounds));
        assert!(painted.bottom() >= grid.bounds.bottom());
        assert!(painted.right() >= grid.bounds.right());
        let outside = frame.leaves.last().unwrap().bounds();
        assert!(
            painted.bottom() < outside.bottom(),
            "unselected siblings stay outside the highlight"
        );
    });
}

#[gpui::test]
fn ordinary_block_selection_retains_leaf_bounds_and_empty_ranges(cx: &mut TestAppContext) {
    let (editor, cx) = open(
        cx,
        "> first\n>\n> second\n\n| a | b |\n| --- | --- |\n| 1 | 2 |",
    );
    let quote = editor.read_with(cx, |state, _| {
        state.model.doc.children(state.model.doc.root)[0]
    });
    editor.update(cx, |state, cx| {
        state
            .set_selection(
                BlockSelection::Blocks {
                    anchor: quote,
                    head: quote,
                },
                cx,
            )
            .unwrap();
    });
    draw(cx);
    editor.read_with(cx, |state, _| {
        let doc = &state.model.doc;
        let frame = state.frame.borrow();
        let (parent, range) = state.model.selected_blocks().unwrap();
        let expected = frame
            .leaves
            .iter()
            .filter(|leaf| doc.path(leaf.info.block).contains(&quote))
            .map(|leaf| leaf.bounds())
            .reduce(|a, b| a.union(&b))
            .unwrap();
        assert_eq!(
            frame.selected_block_bounds(doc, parent, range),
            Some(expected)
        );
        assert!(expected.bottom() < frame.grids[0].bounds.top());
        assert_eq!(frame.selected_block_bounds(doc, parent, 0..0), None);
        let count = doc.children(parent).len();
        assert_eq!(
            frame.selected_block_bounds(doc, parent, count..count + 1),
            None
        );
        let leaf = frame.leaves[0].info.block;
        assert_eq!(frame.selected_block_bounds(doc, leaf, 0..1), None);
    });
}

#[gpui::test]
fn table_corner_selection_dismisses_on_blank_space_without_editing(cx: &mut TestAppContext) {
    let (editor, cx) = open(cx, TBL);
    let corner = table_handle_point(&editor, cx);
    cx.simulate_click(corner, Modifiers::none());
    draw(cx);
    assert!(editor.read_with(cx, |state, _| state.model.selected_blocks().is_some()));
    let outside = editor.read_with(cx, |state, _| {
        let bounds = state.frame.borrow().content.unwrap();
        gpui::point(bounds.center().x, bounds.bottom() + px(40.))
    });
    cx.simulate_click(outside, Modifiers::none());
    draw(cx);
    editor.read_with(cx, |state, _| {
        assert!(state.selection().is_collapsed());
        assert!(!state.dragging);
        assert!(state.block_drag.is_none());
        assert!(
            !state.reveal.get(),
            "dismissal must not scroll to the old table"
        );
    });
    let focus = editor.read_with(cx, |state, _| state.focus_handle.clone());
    assert!(!cx.update(|window, _| focus.is_focused(window)));
    keys(cx, "delete");
    assert_eq!(
        source(&editor, cx),
        TBL,
        "outside clicks and Delete must not edit the table"
    );
}

struct OutsideControlHost {
    editor: Entity<BlockEditorState>,
    clicks: std::rc::Rc<std::cell::Cell<usize>>,
}

impl Render for OutsideControlHost {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        let clicks = self.clicks.clone();
        div()
            .w(px(600.))
            .child(BlockEditor::new(&self.editor))
            .child(
                div()
                    .id("outside-table-control")
                    .debug_selector(|| "outside-table-control".into())
                    .h(px(40.))
                    .child("Outside control")
                    .on_mouse_down(MouseButton::Left, move |_, _, cx| {
                        clicks.set(clicks.get() + 1);
                        cx.stop_propagation();
                    }),
            )
    }
}

#[gpui::test]
fn table_corner_selection_dismisses_before_an_outside_control_consumes_click(
    cx: &mut TestAppContext,
) {
    cx.update(crate::init_for_tests);
    let clicks = std::rc::Rc::new(std::cell::Cell::new(0));
    let host_clicks = clicks.clone();
    let (host, cx) = cx.add_window_view(move |window, cx| OutsideControlHost {
        editor: cx.new(|cx| BlockEditorState::new(window, cx).with_source(TBL)),
        clicks: host_clicks,
    });
    let editor = host.read_with(cx, |host, _| host.editor.clone());
    draw(cx);
    let corner = table_handle_point(&editor, cx);
    cx.simulate_click(corner, Modifiers::none());
    draw(cx);
    assert!(editor.read_with(cx, |state, _| state.model.selected_blocks().is_some()));
    let control = cx.debug_bounds("outside-table-control").unwrap();
    cx.simulate_click(control.center(), Modifiers::none());
    draw(cx);
    assert_eq!(
        clicks.get(),
        1,
        "dismissal must not consume the control's click"
    );
    assert!(editor.read_with(cx, |state, _| state.selection().is_collapsed()));
    assert_eq!(source(&editor, cx), TBL);
}
