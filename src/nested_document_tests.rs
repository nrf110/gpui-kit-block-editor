use super::*;

fn text(markdown: &str) -> BlockSpec {
    BlockSpec {
        kind: SectionKind::Text,
        markdown: markdown.into(),
        children: vec![],
    }
}

fn item(markdown: &str, checked: Option<bool>) -> BlockSpec {
    BlockSpec {
        kind: SectionKind::ListItem { checked },
        markdown: markdown.into(),
        children: vec![],
    }
}

fn list(ordered: bool, start: u64, children: Vec<BlockSpec>) -> BlockSpec {
    BlockSpec {
        kind: SectionKind::List { ordered, start },
        markdown: String::new(),
        children,
    }
}

fn quote(children: Vec<BlockSpec>) -> BlockSpec {
    BlockSpec {
        kind: SectionKind::Quote,
        markdown: String::new(),
        children,
    }
}

fn replica(document: &Document) -> Document {
    let mut peer = Document::new();
    peer.apply_update(&document.encode_state()).unwrap();
    peer
}

fn exchange(left: &mut Document, right: &mut Document) {
    let left_update = left.encode_update(&right.state_vector()).unwrap();
    let right_update = right.encode_update(&left.state_vector()).unwrap();
    left.apply_update(&right_update).unwrap();
    right.apply_update(&left_update).unwrap();
    assert_eq!(left.sections(), right.sections());
    assert_eq!(left.markdown(), right.markdown());
}

fn ids(document: &Document, parent: Option<&str>) -> Vec<SectionId> {
    document
        .children(parent)
        .into_iter()
        .map(|section| section.id)
        .collect()
}

fn source(document: &Document, id: &str) -> TextRef {
    let txn = document.doc.transact();
    let map = document.section_map(&txn, id).unwrap();
    let Some(Out::YText(source)) = map.get(&txn, "source") else {
        panic!("missing source")
    };
    source
}

#[test]
fn projection_includes_containers_and_direct_children_in_preorder() {
    let mut document = Document::new();
    let group = document
        .insert_block(
            None,
            None,
            &list(
                false,
                1,
                vec![
                    item("first", None),
                    BlockSpec {
                        children: vec![quote(vec![text("nested")])],
                        ..item("second", Some(true))
                    },
                ],
            ),
        )
        .unwrap();
    let sections = document.sections();
    assert_eq!(sections.len(), 5);
    assert_eq!(
        sections
            .iter()
            .map(|section| section.depth)
            .collect::<Vec<_>>(),
        [0, 1, 1, 2, 3]
    );
    assert_eq!(sections[0].id, group);
    assert_eq!(sections[0].markdown, "");
    assert_eq!(sections[3].markdown, "");
    assert_eq!(sections[1].parent.as_deref(), Some(group.as_str()));
    assert_eq!(ids(&document, None), [group.clone()]);
    assert_eq!(
        ids(&document, Some(&group)),
        [sections[1].id.clone(), sections[2].id.clone()]
    );
    assert!(document.children(Some("missing")).is_empty());
}

#[test]
fn markdown_serializes_nested_numbering_bullets_tasks_and_quotes() {
    let mut document = Document::new();
    document
        .insert_block(
            None,
            None,
            &list(
                true,
                3,
                vec![
                    BlockSpec {
                        children: vec![list(
                            false,
                            1,
                            vec![item("**child**", Some(false)), item("done", Some(true))],
                        )],
                        ..item("parent", None)
                    },
                    item("second\ncontinuation", None),
                ],
            ),
        )
        .unwrap();
    document
        .insert_block(
            None,
            None,
            &quote(vec![
                text("quoted"),
                list(false, 1, vec![item("entry", None)]),
            ]),
        )
        .unwrap();
    assert_eq!(
        document.markdown(),
        "3. parent\n   - [ ] **child**\n   - [x] done\n4. second\n   continuation\n\n> quoted\n>\n> - entry"
    );
    for section in document.sections() {
        if matches!(section.kind, SectionKind::ListItem { .. }) {
            assert!(!section.markdown.starts_with("- "));
            assert!(!section.markdown.starts_with("3. "));
        }
    }
}

#[test]
fn markdown_import_inserts_all_roots_and_focuses_first_editable_node() {
    let mut document = Document::new();
    let end = document.insert_section(None, "end").unwrap();
    let focused = document
        .insert_section(Some(&end), "- **first**\n- second\n\n> quoted")
        .unwrap();
    let sections = document.sections();
    assert!(matches!(sections[0].kind, SectionKind::List { .. }));
    assert_eq!(sections[1].id, focused);
    assert_eq!(sections[1].markdown, "**first**");
    assert_eq!(sections.last().unwrap().id, end);
    assert_eq!(
        document.markdown(),
        "- **first**\n- second\n\n> quoted\n\nend"
    );
    let before = document.sections();
    assert!(
        document
            .insert_section(Some(&focused), "root only")
            .is_err()
    );
    assert_eq!(document.sections(), before);
}

#[test]
fn code_and_table_literal_line_breaks_remain_inside_one_body() {
    let mut document = Document::new();
    let code = "```rust\nlet a = 1;\nlet b = 2;\n```";
    let table = "| a | b |\n| --- | --- |\n| 1 | 2 |";
    let id = document
        .insert_section(None, &format!("{code}\n\n{table}"))
        .unwrap();
    let sections = document.sections();
    assert_eq!(sections.len(), 2);
    assert_eq!(sections[0].id, id);
    assert_eq!(sections[0].markdown, code);
    assert_eq!(sections[1].markdown, table);
    assert_eq!(document.markdown(), format!("{code}\n\n{table}"));
    document
        .set_markdown(&id, "literal\nbody\nnot reparsed")
        .unwrap();
    assert_eq!(document.sections().len(), 2);
}

#[test]
fn concurrent_nested_reorder_and_text_edit_keep_one_item_and_anchor() {
    let mut alice = Document::new();
    let group = alice
        .insert_block(
            None,
            None,
            &list(false, 1, vec![item("first", None), item("second", None)]),
        )
        .unwrap();
    let children = alice.children(Some(&group));
    let first = &children[0].id;
    let second = &children[1].id;
    let anchor = alice.relative_position(second, 3, Assoc::After).unwrap();
    let original_source = source(&alice, second);
    let mut bob = replica(&alice);
    alice.move_section(second, Some(first)).unwrap();
    bob.set_markdown(second, "brave second!").unwrap();
    exchange(&mut alice, &mut bob);
    assert_eq!(ids(&alice, Some(&group)), [second.clone(), first.clone()]);
    assert_eq!(source(&alice, second), original_source);
    assert_eq!(alice.resolve_position(&anchor), Some((second.clone(), 9)));
    assert_eq!(alice.markdown(), "- brave second!\n- first");
}

#[test]
fn concurrent_child_inserts_stay_dense_for_a_later_between_insert() {
    let mut alice = Document::new();
    let group = alice
        .insert_block(
            None,
            None,
            &list(false, 1, vec![item("start", None), item("end", None)]),
        )
        .unwrap();
    let end = alice.children(Some(&group))[1].id.clone();
    let mut bob = replica(&alice);
    alice
        .insert_block(Some(&group), Some(&end), &item("alice", None))
        .unwrap();
    bob.insert_block(Some(&group), Some(&end), &item("bob", None))
        .unwrap();
    exchange(&mut alice, &mut bob);
    let order = ids(&alice, Some(&group));
    let middle = alice
        .insert_block(Some(&group), Some(&order[2]), &item("between", None))
        .unwrap();
    exchange(&mut alice, &mut bob);
    assert_eq!(
        ids(&alice, Some(&group)),
        [
            order[0].clone(),
            order[1].clone(),
            middle,
            order[2].clone(),
            order[3].clone()
        ]
    );
}

#[test]
fn moving_a_subtree_preserves_all_sources_and_stable_anchors() {
    let mut document = Document::new();
    let left = document
        .insert_block(None, None, &quote(vec![text("a😀child")]))
        .unwrap();
    let right = document
        .insert_block(None, None, &quote(vec![text("right")]))
        .unwrap();
    let child = document.children(Some(&left))[0].id.clone();
    let nested = document
        .insert_block(Some(&child), None, &text("grandchild"))
        .unwrap();
    let anchor = document
        .relative_position(&nested, 5, Assoc::After)
        .unwrap();
    let child_source = source(&document, &child);
    let nested_source = source(&document, &nested);
    document
        .reparent_section(&child, Some(&right), None)
        .unwrap();
    assert_eq!(
        document.resolve_position(&anchor),
        Some((nested.clone(), 5))
    );
    assert_eq!(source(&document, &child), child_source);
    assert_eq!(source(&document, &nested), nested_source);
    assert_eq!(document.children(Some(&child))[0].id, nested);
    assert!(!document.sections().iter().any(|section| section.id == left));
    assert_eq!(
        document
            .sections()
            .iter()
            .find(|section| section.id == child)
            .unwrap()
            .parent
            .as_deref(),
        Some(right.as_str())
    );
}

#[test]
fn cross_group_reorder_is_refused_but_explicit_reparent_is_supported() {
    let mut document = Document::new();
    let left = document
        .insert_block(None, None, &list(false, 1, vec![item("left", None)]))
        .unwrap();
    let right = document
        .insert_block(None, None, &list(true, 5, vec![item("right", None)]))
        .unwrap();
    let a = document.children(Some(&left))[0].id.clone();
    let b = document.children(Some(&right))[0].id.clone();
    let before = document.state_vector();
    assert!(document.move_section(&a, Some(&b)).is_err());
    assert!(document.reparent_section(&a, None, None).is_err());
    assert_eq!(document.state_vector(), before);
    document
        .reparent_section(&a, Some(&right), Some(&b))
        .unwrap();
    assert_eq!(ids(&document, Some(&right)), [a, b]);
    assert_eq!(ids(&document, None), [right]);
}

#[test]
fn invalid_targets_descendant_cycles_and_invalid_specs_are_atomic_errors() {
    let mut document = Document::new();
    let parent = document
        .insert_block(None, None, &quote(vec![text("child")]))
        .unwrap();
    let child = document.children(Some(&parent))[0].id.clone();
    document.clear_history();
    let before = document.state_vector();
    assert!(
        document
            .reparent_section(&parent, Some(&child), None)
            .is_err()
    );
    assert!(
        document
            .reparent_section(&parent, Some(&parent), None)
            .is_err()
    );
    assert!(
        document
            .reparent_section(&child, Some("missing"), None)
            .is_err()
    );
    assert!(
        document
            .insert_block(None, None, &item("orphan", None))
            .is_err()
    );
    assert!(
        document
            .insert_block(None, None, &list(false, 1, vec![text("not an item")]))
            .is_err()
    );
    assert!(
        document
            .replace_section(&child, &[list(false, 1, vec![text("bad")])])
            .is_err()
    );
    assert_eq!(document.state_vector(), before);
    assert!(!document.can_undo());
}

#[test]
fn list_indent_reuses_matching_group_and_outdent_follows_parent_item() {
    let mut document = Document::new();
    let group = document
        .insert_block(
            None,
            None,
            &list(
                false,
                1,
                vec![item("a", None), item("b", None), item("c", None)],
            ),
        )
        .unwrap();
    let children = document.children(Some(&group));
    let (a, b, c) = (&children[0].id, &children[1].id, &children[2].id);
    assert!(!document.indent_section(a).unwrap());
    document.clear_history();
    assert!(document.indent_section(b).unwrap());
    let nested = document.children(Some(a))[0].id.clone();
    assert_eq!(ids(&document, Some(&nested)), [b.clone()]);
    assert!(document.indent_section(c).unwrap());
    assert_eq!(document.children(Some(a)).len(), 1);
    assert_eq!(ids(&document, Some(&nested)), [b.clone(), c.clone()]);
    assert!(document.outdent_section(b).unwrap());
    assert_eq!(ids(&document, Some(&group)), [a.clone(), b.clone()]);
    assert!(document.outdent_section(c).unwrap());
    assert_eq!(
        ids(&document, Some(&group)),
        [a.clone(), c.clone(), b.clone()]
    );
    assert!(document.children(Some(a)).is_empty());
    assert_eq!(document.markdown(), "- a\n- c\n- b");
}

#[test]
fn generic_indent_outdent_and_subtree_undo_are_single_commands() {
    let mut document = Document::new();
    let a = document.insert_section(None, "a").unwrap();
    let b = document.insert_section(None, "b").unwrap();
    let c = document.insert_block(Some(&b), None, &text("c")).unwrap();
    let anchor = document.relative_position(&c, 1, Assoc::Before).unwrap();
    document.clear_history();
    assert!(document.indent_section(&b).unwrap());
    assert_eq!(ids(&document, None), [a.clone()]);
    assert_eq!(ids(&document, Some(&a)), [b.clone()]);
    assert_eq!(document.history.undo_stack().len(), 1);
    assert!(document.undo());
    assert_eq!(ids(&document, None), [a.clone(), b.clone()]);
    assert_eq!(document.resolve_position(&anchor), Some((c.clone(), 1)));
    assert!(document.redo());
    assert!(document.outdent_section(&b).unwrap());
    assert_eq!(ids(&document, None), [a, b.clone()]);
    assert_eq!(ids(&document, Some(&b)), [c]);
    assert!(!document.outdent_section(&b).unwrap());
}

#[test]
fn empty_item_exit_splits_middle_list_without_reordering_trailing_items() {
    let mut document = Document::new();
    let group = document
        .insert_block(
            None,
            None,
            &list(
                true,
                4,
                vec![item("a", None), item("", Some(true)), item("c", None)],
            ),
        )
        .unwrap();
    let children = document.children(Some(&group));
    let empty = &children[1].id;
    let anchor = document.relative_position(empty, 0, Assoc::After).unwrap();
    let before = document.sections();
    document.clear_history();
    assert!(!document.exit_list_item(&children[0].id).unwrap());
    assert!(document.exit_list_item(empty).unwrap());
    let roots = document.children(None);
    assert_eq!(roots.len(), 3);
    assert_eq!(roots[0].id, group);
    assert_eq!(roots[1].id, *empty);
    assert_eq!(roots[1].kind, SectionKind::Text);
    assert_eq!(
        roots[2].kind,
        SectionKind::List {
            ordered: true,
            start: 6
        }
    );
    assert_eq!(document.children(Some(&roots[2].id))[0].id, children[2].id);
    assert_eq!(document.resolve_position(&anchor), Some((empty.clone(), 0)));
    assert_eq!(document.history.undo_stack().len(), 1);
    assert!(document.undo());
    assert_eq!(document.sections(), before);
}

#[test]
fn last_empty_item_exit_removes_group_and_top_level_outdent_unlists() {
    let mut document = Document::new();
    let id = document.insert_section(None, "").unwrap();
    document.wrap_list(&id, false, true).unwrap();
    let group = document.sections()[0].id.clone();
    document.clear_history();
    assert!(document.exit_list_item(&id).unwrap());
    assert_eq!(document.sections().len(), 1);
    assert_eq!(document.sections()[0].id, id);
    assert_eq!(document.sections()[0].kind, SectionKind::Text);
    assert!(document.undo());
    assert_eq!(document.sections()[0].id, group);
    document.set_markdown(&id, "nonempty").unwrap();
    assert!(document.outdent_section(&id).unwrap());
    assert_eq!(document.markdown(), "nonempty");
    assert_eq!(ids(&document, None), [id]);
}

#[test]
fn enter_replacement_retains_first_editable_id_source_children_and_anchors() {
    let mut document = Document::new();
    let id = document.insert_section(None, "alpha beta").unwrap();
    let child = document
        .insert_block(Some(&id), None, &quote(vec![text("keep child")]))
        .unwrap();
    let old_source = source(&document, &id);
    let anchor = document.relative_position(&id, 2, Assoc::After).unwrap();
    let before = document.sections();
    document.clear_history();
    let paths = document
        .replace_section(&id, &[text("alpha"), text("beta")])
        .unwrap();
    assert_eq!(paths[0], (vec![0], id.clone()));
    let next = paths[1].1.clone();
    assert_eq!(ids(&document, None), [id.clone(), next]);
    assert_eq!(ids(&document, Some(&id)), [child]);
    assert_eq!(source(&document, &id), old_source);
    assert_eq!(document.resolve_position(&anchor), Some((id.clone(), 2)));
    assert_eq!(document.history.undo_stack().len(), 1);
    assert!(document.undo());
    assert_eq!(document.sections(), before);
    assert!(document.redo());
    assert_eq!(document.resolve_position(&anchor), Some((id, 2)));
}

#[test]
fn ordinary_replacement_does_not_overwrite_concurrent_reorder() {
    let mut alice = Document::new();
    let a = alice.insert_section(None, "a").unwrap();
    let b = alice.insert_section(None, "b").unwrap();
    let mut bob = replica(&alice);
    alice.replace_section(&b, &[text("b edited")]).unwrap();
    bob.move_section(&b, Some(&a)).unwrap();
    exchange(&mut alice, &mut bob);
    assert_eq!(ids(&alice, None), [b, a]);
    assert_eq!(alice.markdown(), "b edited\n\na");
}

#[test]
fn enter_in_checked_task_creates_unchecked_sibling_items_not_paragraphs() {
    let mut document = Document::new();
    let group = document
        .insert_block(None, None, &list(false, 1, vec![item("done", Some(true))]))
        .unwrap();
    let id = document.children(Some(&group))[0].id.clone();
    let paths = document
        .replace_section(&id, &[text("done"), text("next"), text("")])
        .unwrap();
    assert_eq!(paths[0].1, id);
    let items = document.children(Some(&group));
    assert_eq!(items.len(), 3);
    assert_eq!(
        items[0].kind,
        SectionKind::ListItem {
            checked: Some(true)
        }
    );
    assert_eq!(
        items[1].kind,
        SectionKind::ListItem {
            checked: Some(false)
        }
    );
    assert_eq!(
        items[2].kind,
        SectionKind::ListItem {
            checked: Some(false)
        }
    );
    assert_eq!(document.markdown(), "- [x] done\n- [ ] next\n- [ ] ");
}

#[test]
fn typed_list_replacement_preserves_id_and_returns_original_parse_paths() {
    let mut document = Document::new();
    let id = document.insert_section(None, "body").unwrap();
    let old_source = source(&document, &id);
    let anchor = document.relative_position(&id, 2, Assoc::After).unwrap();
    let parsed = crate::block_markdown::parse_blocks("- **body**\n- next").unwrap();
    let paths = document.replace_section(&id, &parsed.blocks).unwrap();
    assert_eq!(
        paths.iter().find(|(path, _)| path == &[0, 0]).unwrap().1,
        id
    );
    let group = paths
        .iter()
        .find(|(path, _)| path == &[0])
        .unwrap()
        .1
        .clone();
    assert_eq!(ids(&document, None), [group.clone()]);
    assert_eq!(document.children(Some(&group))[0].markdown, "**body**");
    assert_eq!(source(&document, &id), old_source);
    assert_eq!(document.resolve_position(&anchor), Some((id.clone(), 4)));
    let parsed = crate::block_markdown::parse_blocks("2. body").unwrap();
    let paths = document.replace_section(&id, &parsed.blocks).unwrap();
    assert_eq!(
        paths.iter().find(|(path, _)| path == &[0]).unwrap().1,
        group
    );
    assert_eq!(
        paths.iter().find(|(path, _)| path == &[0, 0]).unwrap().1,
        id
    );
    assert_eq!(document.markdown(), "2. body\n3. next");
}

#[test]
fn quote_replacement_in_item_maps_paths_without_synthetic_item_offsets() {
    let mut document = Document::new();
    let group = document
        .insert_block(None, None, &list(false, 1, vec![item("body", None)]))
        .unwrap();
    let id = document.children(Some(&group))[0].id.clone();
    let old_source = source(&document, &id);
    let paths = document
        .replace_section(&id, &[quote(vec![text("body")])])
        .unwrap();
    assert_eq!(paths.len(), 2);
    assert_eq!(paths[0].0, [0]);
    assert_eq!(paths[1], (vec![0, 0], id.clone()));
    assert_eq!(source(&document, &id), old_source);
    assert_eq!(document.children(Some(&group)).len(), 1);
    assert_eq!(document.markdown(), "- > body");
}

#[test]
fn list_toggle_changes_kind_and_unlists_without_replacing_text() {
    let mut document = Document::new();
    let id = document.insert_section(None, "**body**").unwrap();
    let old_source = source(&document, &id);
    let anchor = document.relative_position(&id, 3, Assoc::After).unwrap();
    document.clear_history();
    document.wrap_list(&id, false, false).unwrap();
    assert_eq!(document.markdown(), "- **body**");
    assert_eq!(document.history.undo_stack().len(), 1);
    document.wrap_list(&id, true, true).unwrap();
    assert_eq!(document.markdown(), "1. [ ] **body**");
    document
        .set_kind(
            &id,
            SectionKind::ListItem {
                checked: Some(true),
            },
        )
        .unwrap();
    assert_eq!(document.markdown(), "1. [x] **body**");
    document.wrap_list(&id, true, true).unwrap();
    assert_eq!(document.markdown(), "**body**");
    assert_eq!(ids(&document, None), [id.clone()]);
    assert_eq!(source(&document, &id), old_source);
    assert_eq!(document.resolve_position(&anchor), Some((id, 3)));
}

#[test]
fn quote_toggle_preserves_item_identity_and_list_structure() {
    let mut document = Document::new();
    let id = document.insert_section(None, "body").unwrap();
    let old_source = source(&document, &id);
    document.wrap_quote(&id).unwrap();
    assert_eq!(document.markdown(), "> body");
    document.wrap_quote(&id).unwrap();
    assert_eq!(ids(&document, None), [id.clone()]);
    document.wrap_list(&id, false, false).unwrap();
    document.clear_history();
    document.wrap_quote(&id).unwrap();
    assert_eq!(document.markdown(), "> - body");
    assert_eq!(document.history.undo_stack().len(), 1);
    document.wrap_quote(&id).unwrap();
    assert_eq!(document.markdown(), "- body");
    assert_eq!(source(&document, &id), old_source);
}

#[test]
fn deleting_a_parent_removes_all_current_descendants_in_one_undo_item() {
    let mut document = Document::new();
    let parent = document
        .insert_block(
            None,
            None,
            &quote(vec![list(false, 1, vec![item("child", None)])]),
        )
        .unwrap();
    let sections = document.sections();
    let child = sections.last().unwrap().id.clone();
    let anchor = document.relative_position(&child, 2, Assoc::After).unwrap();
    document.clear_history();
    document.remove_section(&parent).unwrap();
    assert!(document.sections().is_empty());
    assert!(document.resolve_position(&anchor).is_none());
    assert_eq!(document.history.undo_stack().len(), 1);
    assert!(document.undo());
    assert_eq!(document.sections(), sections);
    assert_eq!(document.resolve_position(&anchor), Some((child, 2)));
    assert!(document.redo());
    assert!(document.sections().is_empty());
}

#[test]
fn deleting_last_child_prunes_empty_list_and_quote_ancestors() {
    let mut document = Document::new();
    document
        .insert_block(
            None,
            None,
            &quote(vec![list(false, 1, vec![item("child", None)])]),
        )
        .unwrap();
    let before = document.sections();
    let child = before.last().unwrap().id.clone();
    document.clear_history();
    document.remove_section(&child).unwrap();
    assert!(document.sections().is_empty());
    assert_eq!(document.history.undo_stack().len(), 1);
    assert!(document.undo());
    assert_eq!(document.sections(), before);
}

#[test]
fn deleted_parent_hides_concurrently_inserted_orphan_subtrees_without_erasing_them() {
    let mut alice = Document::new();
    let parent = alice
        .insert_block(None, None, &quote(vec![text("existing")]))
        .unwrap();
    let mut bob = replica(&alice);
    alice.remove_section(&parent).unwrap();
    let orphan = bob
        .insert_block(Some(&parent), None, &quote(vec![text("peer text")]))
        .unwrap();
    let child = bob.children(Some(&orphan))[0].id.clone();
    let anchor = bob.relative_position(&child, 4, Assoc::After).unwrap();
    exchange(&mut alice, &mut bob);
    assert!(alice.sections().is_empty());
    assert!(alice.resolve_position(&anchor).is_none());
    assert!(alice.section_map(&alice.doc.transact(), &orphan).is_ok());
    assert_eq!(
        source(&alice, &child).get_string(&alice.doc.transact()),
        "peer text"
    );
    let restored = replica(&alice);
    assert!(restored.sections().is_empty());
    assert_eq!(
        source(&restored, &child).get_string(&restored.doc.transact()),
        "peer text"
    );
}

#[test]
fn concurrent_reparent_cycles_break_deterministically_without_losing_nodes() {
    let mut alice = Document::new();
    let a = alice.insert_section(None, "a").unwrap();
    let b = alice.insert_section(None, "b").unwrap();
    let a_anchor = alice.relative_position(&a, 1, Assoc::After).unwrap();
    let b_anchor = alice.relative_position(&b, 1, Assoc::After).unwrap();
    let mut bob = replica(&alice);
    alice.reparent_section(&a, Some(&b), None).unwrap();
    bob.reparent_section(&b, Some(&a), None).unwrap();
    exchange(&mut alice, &mut bob);
    let root = a.min(b);
    let sections = alice.sections();
    assert_eq!(sections.len(), 2);
    assert_eq!(sections[0].id, root);
    assert_eq!(sections[0].parent, None);
    assert_eq!(sections[1].parent.as_deref(), Some(root.as_str()));
    assert_eq!(alice.resolve_position(&a_anchor).unwrap().1, 1);
    assert_eq!(alice.resolve_position(&b_anchor).unwrap().1, 1);
    let state = alice.state_vector();
    assert_eq!(replica(&alice).sections(), sections);
    assert_eq!(alice.state_vector(), state);
}

#[test]
fn concurrent_three_node_cycle_projects_every_node_once() {
    let mut alice = Document::new();
    let a = alice.insert_section(None, "a").unwrap();
    let b = alice.insert_section(None, "b").unwrap();
    let c = alice.insert_section(None, "c").unwrap();
    let mut bob = replica(&alice);
    let mut carol = replica(&alice);
    alice.reparent_section(&a, Some(&b), None).unwrap();
    bob.reparent_section(&b, Some(&c), None).unwrap();
    carol.reparent_section(&c, Some(&a), None).unwrap();
    exchange(&mut alice, &mut bob);
    exchange(&mut alice, &mut carol);
    exchange(&mut alice, &mut bob);
    let mut actual: Vec<_> = alice
        .sections()
        .into_iter()
        .map(|section| section.id)
        .collect();
    actual.sort();
    let mut expected = vec![a, b, c];
    expected.sort();
    assert_eq!(actual, expected);
    assert_eq!(alice.children(None).len(), 1);
}

#[test]
fn concurrent_cross_parent_moves_choose_one_whole_parent_rank_register() {
    let mut alice = Document::new();
    let a = alice
        .insert_block(None, None, &quote(vec![text("a")]))
        .unwrap();
    let b = alice
        .insert_block(None, None, &quote(vec![text("b")]))
        .unwrap();
    let moving = alice.insert_section(None, "moving").unwrap();
    let mut bob = replica(&alice);
    alice.reparent_section(&moving, Some(&a), None).unwrap();
    bob.reparent_section(&moving, Some(&b), None).unwrap();
    let position = |document: &Document| {
        let txn = document.doc.transact();
        let map = document.section_map(&txn, &moving).unwrap();
        let Some(Out::Any(Any::String(position))) = map.get(&txn, "position") else {
            panic!("missing position")
        };
        position.to_string()
    };
    let alice_placement = position(&alice);
    let bob_placement = position(&bob);
    exchange(&mut alice, &mut bob);
    let merged = position(&alice);
    assert!(merged == alice_placement || merged == bob_placement);
    assert_eq!(
        alice
            .sections()
            .iter()
            .filter(|section| section.id == moving)
            .count(),
        1
    );
}

#[test]
fn batch_moves_across_lists_and_root_preserve_sources_subtrees_and_task_roles() {
    let mut document = Document::new();
    let left = document
        .insert_block(
            None,
            None,
            &list(
                false,
                1,
                vec![item("done", Some(true)), item("plain", None)],
            ),
        )
        .unwrap();
    let original_items = document.children(Some(&left));
    let done = original_items[0].id.clone();
    let plain = original_items[1].id.clone();
    let right = document
        .insert_block(None, None, &list(true, 4, vec![item("task", Some(false))]))
        .unwrap();
    let task = document.children(Some(&right))[0].id.clone();
    let paragraph = document.insert_section(None, "paragraph").unwrap();
    let nested = document
        .insert_block(Some(&paragraph), None, &quote(vec![text("a😀child")]))
        .unwrap();
    let child = document.children(Some(&nested))[0].id.clone();
    let paragraph_source = source(&document, &paragraph);
    let child_source = source(&document, &child);
    let anchor = document.relative_position(&child, 5, Assoc::After).unwrap();
    document
        .move_sections(
            &[paragraph.clone(), plain.clone(), done.clone()],
            Some(&right),
            Some(&task),
        )
        .unwrap();
    assert_eq!(
        ids(&document, Some(&right)),
        [done.clone(), plain.clone(), paragraph.clone(), task.clone()]
    );
    let children = document.children(Some(&right));
    assert_eq!(
        children
            .iter()
            .map(|child| child.kind.clone())
            .collect::<Vec<_>>(),
        [
            SectionKind::ListItem {
                checked: Some(true)
            },
            SectionKind::ListItem { checked: None },
            SectionKind::ListItem {
                checked: Some(false)
            },
            SectionKind::ListItem {
                checked: Some(false)
            },
        ]
    );
    assert_eq!(ids(&document, None), [right.clone()]);
    assert_eq!(source(&document, &paragraph), paragraph_source);
    assert_eq!(source(&document, &child), child_source);
    assert_eq!(document.resolve_position(&anchor), Some((child.clone(), 5)));

    document
        .move_sections(&[paragraph.clone(), done.clone()], None, Some(&right))
        .unwrap();
    assert_eq!(
        ids(&document, None),
        [done.clone(), paragraph.clone(), right.clone()]
    );
    assert_eq!(document.children(None)[0].kind, SectionKind::Text);
    assert_eq!(document.children(None)[1].kind, SectionKind::Text);
    assert_eq!(ids(&document, Some(&right)), [plain, task]);
    assert_eq!(ids(&document, Some(&paragraph)), [nested]);
    assert_eq!(source(&document, &paragraph), paragraph_source);
    assert_eq!(source(&document, &child), child_source);
    assert_eq!(document.resolve_position(&anchor), Some((child, 5)));
}

#[test]
fn batch_reordering_excludes_moving_siblings_and_uses_document_order() {
    let mut document = Document::new();
    let a = document.insert_section(None, "a").unwrap();
    let b = document.insert_section(None, "b").unwrap();
    let c = document.insert_section(None, "c").unwrap();
    let d = document.insert_section(None, "d").unwrap();
    let e = document.insert_section(None, "e").unwrap();
    document.clear_history();
    document
        .move_sections(&[d.clone(), b.clone()], None, Some(&e))
        .unwrap();
    assert_eq!(
        ids(&document, None),
        [a.clone(), c.clone(), b.clone(), d.clone(), e.clone()]
    );
    document
        .move_sections(&[d.clone(), b.clone()], None, Some(&a))
        .unwrap();
    assert_eq!(
        ids(&document, None),
        [b.clone(), d.clone(), a.clone(), c.clone(), e.clone()]
    );
    document
        .move_sections(&[d.clone(), b.clone()], None, None)
        .unwrap();
    assert_eq!(
        ids(&document, None),
        [a.clone(), c.clone(), e.clone(), b.clone(), d.clone()]
    );
    assert_eq!(document.history.undo_stack().len(), 3);
    assert!(document.undo());
    assert_eq!(
        ids(&document, None),
        [b.clone(), d.clone(), a.clone(), c.clone(), e.clone()]
    );
    assert!(document.redo());
    assert_eq!(ids(&document, None), [a, c, e, b, d]);
}

#[test]
fn batch_extraction_pruning_and_roles_undo_once_without_reverting_peer_text() {
    let mut alice = Document::new();
    let outer = alice
        .insert_block(
            None,
            None,
            &quote(vec![list(
                false,
                1,
                vec![item("alpha", Some(true)), item("beta", Some(false))],
            )]),
        )
        .unwrap();
    let group = alice.children(Some(&outer))[0].id.clone();
    let children = alice.children(Some(&group));
    let a = children[0].id.clone();
    let b = children[1].id.clone();
    let child = alice.insert_block(Some(&a), None, &text("leaf")).unwrap();
    let tail = alice.insert_section(None, "tail").unwrap();
    let a_source = source(&alice, &a);
    let child_source = source(&alice, &child);
    let anchor = alice.relative_position(&a, 2, Assoc::After).unwrap();
    let child_anchor = alice.relative_position(&child, 2, Assoc::After).unwrap();
    let mut bob = replica(&alice);
    alice.clear_history();
    alice
        .move_sections(&[b.clone(), a.clone()], None, Some(&tail))
        .unwrap();
    bob.set_markdown(&a, "peer alpha!").unwrap();
    bob.set_markdown(&child, "new leaf").unwrap();
    exchange(&mut alice, &mut bob);
    assert_eq!(ids(&alice, None), [a.clone(), b.clone(), tail.clone()]);
    assert_eq!(alice.children(None)[0].kind, SectionKind::Text);
    assert_eq!(alice.children(None)[1].kind, SectionKind::Text);
    assert_eq!(alice.history.undo_stack().len(), 1);
    assert!(alice.undo());
    exchange(&mut alice, &mut bob);
    assert_eq!(ids(&alice, None), [outer.clone(), tail.clone()]);
    assert_eq!(ids(&alice, Some(&outer)), [group.clone()]);
    assert_eq!(ids(&alice, Some(&group)), [a.clone(), b.clone()]);
    let restored = alice.children(Some(&group));
    assert_eq!(restored[0].markdown, "peer alpha!");
    assert_eq!(
        restored[0].kind,
        SectionKind::ListItem {
            checked: Some(true)
        }
    );
    assert_eq!(
        restored[1].kind,
        SectionKind::ListItem {
            checked: Some(false)
        }
    );
    assert_eq!(source(&alice, &a), a_source);
    assert_eq!(source(&alice, &child), child_source);
    assert_eq!(alice.resolve_position(&anchor), Some((a.clone(), 7)));
    assert_eq!(
        alice.resolve_position(&child_anchor),
        Some((child.clone(), 6))
    );
    assert!(!alice.can_undo());
    assert!(alice.redo());
    exchange(&mut alice, &mut bob);
    assert_eq!(ids(&alice, None), [a.clone(), b, tail]);
    assert_eq!(ids(&alice, Some(&a)), [child.clone()]);
    assert_eq!(alice.children(None)[0].markdown, "peer alpha!");
    assert_eq!(alice.children(None)[0].kind, SectionKind::Text);
    assert_eq!(source(&alice, &a), a_source);
    assert_eq!(source(&alice, &child), child_source);
    assert_eq!(alice.resolve_position(&anchor), Some((a, 7)));
    assert_eq!(alice.resolve_position(&child_anchor), Some((child, 6)));
}

#[test]
fn invalid_batch_kinds_targets_and_cycles_leave_no_writes_or_history() {
    let mut document = Document::new();
    let paragraph = document.insert_section(None, "paragraph").unwrap();
    let container = document
        .insert_block(None, None, &quote(vec![text("child")]))
        .unwrap();
    let child = document.children(Some(&container))[0].id.clone();
    let group = document
        .insert_block(None, None, &list(false, 1, vec![item("entry", None)]))
        .unwrap();
    let entry = document.children(Some(&group))[0].id.clone();
    document.clear_history();
    let before = document.encode_state();
    for (selection, parent, target) in [
        (
            vec![paragraph.clone(), container.clone()],
            Some(group.as_str()),
            None,
        ),
        (
            vec![paragraph.clone(), group.clone()],
            Some(group.as_str()),
            None,
        ),
        (
            vec![paragraph.clone(), container.clone()],
            Some(child.as_str()),
            None,
        ),
        (vec![paragraph.clone(), "missing".into()], None, None),
        (vec![paragraph.clone()], Some("missing"), None),
        (vec![paragraph.clone()], None, Some(entry.as_str())),
        (vec![paragraph.clone()], None, Some("missing")),
        (
            vec![paragraph.clone(), container.clone()],
            None,
            Some(container.as_str()),
        ),
    ] {
        assert!(document.move_sections(&selection, parent, target).is_err());
        assert_eq!(document.encode_state(), before);
        assert!(!document.can_undo());
    }
}

#[test]
fn batch_deduplicates_covered_descendants_and_moves_whole_containers() {
    let mut document = Document::new();
    let group = document
        .insert_block(
            None,
            None,
            &list(false, 1, vec![item("parent", Some(true))]),
        )
        .unwrap();
    let parent = document.children(Some(&group))[0].id.clone();
    let nested = document
        .insert_block(Some(&parent), None, &quote(vec![text("leaf")]))
        .unwrap();
    let leaf = document.children(Some(&nested))[0].id.clone();
    let tail = document.insert_section(None, "tail").unwrap();
    let before = document.sections();
    let group_source = source(&document, &group);
    let leaf_source = source(&document, &leaf);
    let anchor = document.relative_position(&leaf, 2, Assoc::After).unwrap();
    document.clear_history();
    document
        .move_sections(
            &[leaf.clone(), parent.clone(), group.clone(), group.clone()],
            None,
            None,
        )
        .unwrap();
    assert_eq!(ids(&document, None), [tail, group.clone()]);
    assert_eq!(document.children(None)[1].kind, before[0].kind);
    assert_eq!(ids(&document, Some(&group)), [parent.clone()]);
    assert_eq!(document.children(Some(&group))[0].kind, before[1].kind);
    assert_eq!(ids(&document, Some(&parent)), [nested]);
    assert_eq!(source(&document, &group), group_source);
    assert_eq!(source(&document, &leaf), leaf_source);
    assert_eq!(document.resolve_position(&anchor), Some((leaf.clone(), 2)));
    assert_eq!(document.history.undo_stack().len(), 1);
    assert!(document.undo());
    assert_eq!(document.sections(), before);
    assert!(document.redo());
    assert_eq!(document.resolve_position(&anchor), Some((leaf, 2)));
}

#[test]
fn batch_lifts_nested_items_into_parent_list_and_prunes_only_empty_group() {
    let mut document = Document::new();
    let outer = document
        .insert_block(
            None,
            None,
            &list(false, 1, vec![item("owner", None), item("tail", None)]),
        )
        .unwrap();
    let children = document.children(Some(&outer));
    let owner = children[0].id.clone();
    let tail = children[1].id.clone();
    let nested = document
        .insert_block(
            Some(&owner),
            None,
            &list(true, 3, vec![item("a", Some(true)), item("b", None)]),
        )
        .unwrap();
    let children = document.children(Some(&nested));
    let a = children[0].id.clone();
    let b = children[1].id.clone();
    let before = document.sections();
    document.clear_history();
    document
        .move_sections(&[b.clone(), a.clone()], Some(&outer), Some(&tail))
        .unwrap();
    assert_eq!(ids(&document, Some(&outer)), [owner.clone(), a, b, tail]);
    assert!(document.children(Some(&owner)).is_empty());
    assert_eq!(
        document.children(Some(&outer))[1].kind,
        SectionKind::ListItem {
            checked: Some(true)
        }
    );
    assert_eq!(document.history.undo_stack().len(), 1);
    assert!(document.undo());
    assert_eq!(document.sections(), before);
}

#[test]
fn batch_converts_plain_text_once_and_keeps_selected_descendants_attached() {
    let mut document = Document::new();
    let paragraph = document.insert_section(None, "paragraph").unwrap();
    let child = document
        .insert_block(Some(&paragraph), None, &text("child"))
        .unwrap();
    let group = document
        .insert_block(None, None, &list(false, 1, vec![item("entry", None)]))
        .unwrap();
    let original_source = source(&document, &paragraph);
    let before = document.sections();
    document.clear_history();
    document
        .move_sections(
            &[child.clone(), paragraph.clone(), paragraph.clone()],
            Some(&group),
            None,
        )
        .unwrap();
    let children = document.children(Some(&group));
    assert_eq!(children.len(), 2);
    assert_eq!(children[1].id, paragraph);
    assert_eq!(children[1].kind, SectionKind::ListItem { checked: None });
    assert_eq!(ids(&document, Some(&paragraph)), [child.clone()]);
    assert_eq!(
        document.children(Some(&paragraph))[0].kind,
        SectionKind::Text
    );
    assert_eq!(source(&document, &paragraph), original_source);
    assert_eq!(document.history.undo_stack().len(), 1);
    assert!(document.undo());
    assert_eq!(document.sections(), before);
    assert!(document.redo());
    document
        .move_sections(&[child.clone(), paragraph.clone()], None, None)
        .unwrap();
    assert_eq!(ids(&document, None), [group, paragraph.clone()]);
    assert_eq!(document.children(None)[1].kind, SectionKind::Text);
    assert_eq!(ids(&document, Some(&paragraph)), [child]);
    assert_eq!(source(&document, &paragraph), original_source);
}

#[test]
fn concurrent_root_and_cross_list_moves_keep_parent_and_list_role_consistent() {
    let mut alice = Document::new();
    let original = alice
        .insert_block(
            None,
            None,
            &list(
                false,
                1,
                vec![item("alpha", None), item("beta", None), item("guard", None)],
            ),
        )
        .unwrap();
    let children = alice.children(Some(&original));
    let alpha = children[0].id.clone();
    let beta = children[1].id.clone();
    let destination = alice
        .insert_block(None, None, &list(true, 3, vec![item("target", None)]))
        .unwrap();
    let alpha_source = source(&alice, &alpha);
    let mut bob = replica(&alice);
    // Swap actors for the two items so either client-ID conflict ordering
    // exercises a root conversion losing to a cross-list placement.
    alice
        .move_sections(std::slice::from_ref(&alpha), None, None)
        .unwrap();
    bob.move_sections(std::slice::from_ref(&alpha), Some(&destination), None)
        .unwrap();
    bob.move_sections(std::slice::from_ref(&beta), None, None)
        .unwrap();
    alice
        .move_sections(std::slice::from_ref(&beta), Some(&destination), None)
        .unwrap();
    bob.set_markdown(&alpha, "peer alpha").unwrap();
    exchange(&mut alice, &mut bob);
    for id in [&alpha, &beta] {
        let sections = alice.sections();
        let block = sections.iter().find(|block| &block.id == id).unwrap();
        if block.parent.is_none() {
            assert_eq!(block.kind, SectionKind::Text);
        } else {
            assert_eq!(block.parent.as_deref(), Some(destination.as_str()));
            assert_eq!(block.kind, SectionKind::ListItem { checked: None });
        }
    }
    assert_eq!(source(&alice, &alpha), alpha_source);
    assert!(
        alice
            .sections()
            .iter()
            .any(|block| block.id == alpha && block.markdown == "peer alpha")
    );
}

#[test]
fn concurrent_checkbox_edits_cannot_turn_extracted_blocks_back_into_list_items() {
    let mut alice = Document::new();
    let group = alice
        .insert_block(
            None,
            None,
            &list(
                false,
                1,
                vec![
                    item("alpha", Some(false)),
                    item("beta", Some(false)),
                    item("guard", None),
                ],
            ),
        )
        .unwrap();
    let children = alice.children(Some(&group));
    let alpha = children[0].id.clone();
    let beta = children[1].id.clone();
    let mut bob = replica(&alice);
    alice
        .move_sections(std::slice::from_ref(&alpha), None, None)
        .unwrap();
    bob.set_kind(
        &alpha,
        SectionKind::ListItem {
            checked: Some(true),
        },
    )
    .unwrap();
    bob.move_sections(std::slice::from_ref(&beta), None, None)
        .unwrap();
    alice
        .set_kind(
            &beta,
            SectionKind::ListItem {
                checked: Some(true),
            },
        )
        .unwrap();
    exchange(&mut alice, &mut bob);
    let roots = alice.children(None);
    for id in [&alpha, &beta] {
        assert_eq!(
            roots.iter().find(|block| &block.id == id).unwrap().kind,
            SectionKind::Text
        );
    }
    assert_eq!(alice.children(Some(&group))[0].markdown, "guard");
    assert!(!alice.markdown().contains("[x]"));
}
