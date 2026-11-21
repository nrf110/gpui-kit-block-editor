use super::*;

fn replica(original: &Document) -> Document {
    let mut document = Document::new();
    document.apply_update(&original.encode_state()).unwrap();
    document
}

fn exchange(left: &mut Document, right: &mut Document) {
    let to_left = right.encode_update(&left.state_vector()).unwrap();
    let to_right = left.encode_update(&right.state_vector()).unwrap();
    left.apply_update(&to_left).unwrap();
    right.apply_update(&to_right).unwrap();
    assert_eq!(left.sections(), right.sections());
}

fn ids(document: &Document) -> Vec<SectionId> {
    document
        .sections()
        .into_iter()
        .map(|section| section.id)
        .collect()
}

#[test]
fn unicode_anchors_validate_source_boundaries() {
    let mut document = Document::new();
    let id = document.insert_section(None, "a😀e\u{301}終").unwrap();
    for byte in [0, 1, 5, 6, 8, 11] {
        for assoc in [Assoc::Before, Assoc::After] {
            let position = document
                .relative_position(&id, &node(&document, &id), byte, assoc)
                .unwrap();
            assert_eq!(position.section(), id);
            assert_eq!(
                document.resolve_position(&position),
                Some((id.clone(), position.node().to_owned(), byte))
            );
        }
    }
    for byte in [2, 3, 4, 7, 9, 10, 12, usize::MAX] {
        assert!(
            document
                .relative_position(&id, &node(&document, &id), byte, Assoc::After)
                .is_err()
        );
    }
    assert!(
        document
            .relative_position("missing", "missing", 0, Assoc::After)
            .is_err()
    );
}

#[test]
fn anchors_follow_unicode_insertions_and_association() {
    let mut document = Document::new();
    let id = document.insert_section(None, "a😀終").unwrap();
    let before = document
        .relative_position(&id, &node(&document, &id), 5, Assoc::Before)
        .unwrap();
    let after = document
        .relative_position(&id, &node(&document, &id), 5, Assoc::After)
        .unwrap();
    let start = document
        .relative_position(&id, &node(&document, &id), 0, Assoc::Before)
        .unwrap();
    let end = document
        .relative_position(&id, &node(&document, &id), 8, Assoc::After)
        .unwrap();
    let mut peer = replica(&document);
    edit_text(&mut peer, &id, "漢a😀🚀終!").unwrap();
    document.apply_update(&peer.encode_state()).unwrap();
    assert_eq!(
        document.resolve_position(&before),
        Some((id.clone(), before.node().to_owned(), 8))
    );
    assert_eq!(
        document.resolve_position(&after),
        Some((id.clone(), after.node().to_owned(), 12))
    );
    assert_eq!(
        document.resolve_position(&start),
        Some((id.clone(), start.node().to_owned(), 0))
    );
    assert_eq!(
        document.resolve_position(&end),
        Some((id, end.node().to_owned(), "漢a😀🚀終!".len()))
    );
}

#[test]
fn deleted_text_anchors_collapse_and_follow_undo_redo() {
    let mut document = Document::new();
    let id = document.insert_section(None, "a😀middle終").unwrap();
    let anchor = document
        .relative_position(&id, &node(&document, &id), 8, Assoc::After)
        .unwrap();
    document.clear_history();
    edit_text(&mut document, &id, "a😀終").unwrap();
    assert_eq!(
        document.resolve_position(&anchor),
        Some((id.clone(), anchor.node().to_owned(), 5))
    );
    assert!(document.undo());
    assert_eq!(
        document.resolve_position(&anchor),
        Some((id.clone(), anchor.node().to_owned(), 8))
    );
    assert!(document.redo());
    assert_eq!(
        document.resolve_position(&anchor),
        Some((id, anchor.node().to_owned(), 5))
    );
}

#[test]
fn deleted_parent_anchor_resolution_never_resurrects() {
    let mut document = Document::new();
    let id = document.insert_section(None, "😀source").unwrap();
    let anchor = document
        .relative_position(&id, &node(&document, &id), 4, Assoc::After)
        .unwrap();
    let mut peer = replica(&document);
    peer.remove_section(&id).unwrap();
    document.apply_update(&peer.encode_state()).unwrap();
    let state = document.encode_state();
    assert!(document.resolve_position(&anchor).is_none());
    assert_eq!(document.encode_state(), state);
    assert!(document.sections().is_empty());
}

#[test]
fn empty_text_and_replaced_source_anchors_are_safe() {
    let mut document = Document::new();
    let id = document.insert_section(None, "").unwrap();
    let before = document
        .relative_position(&id, &node(&document, &id), 0, Assoc::Before)
        .unwrap();
    let after = document
        .relative_position(&id, &node(&document, &id), 0, Assoc::After)
        .unwrap();
    edit_text(&mut document, &id, "😀").unwrap();
    assert_eq!(
        document.resolve_position(&before),
        Some((id.clone(), before.node().to_owned(), 0))
    );
    assert_eq!(
        document.resolve_position(&after),
        Some((id.clone(), after.node().to_owned(), 4))
    );
    {
        let mut txn = document.doc.transact_mut_with(REMOTE_ORIGIN);
        let section = document.section_map(&txn, &id).unwrap();
        let body = rich_storage::body(&section, &txn).unwrap();
        let Some(Out::YMap(nodes)) = body.get(&txn, "nodes") else {
            panic!("missing nodes")
        };
        let Some(Out::YMap(leaf)) = nodes.get(&txn, before.node()) else {
            panic!("missing leaf")
        };
        leaf.insert(&mut txn, "text", TextPrelim::new("replacement"));
    }
    assert!(document.resolve_position(&before).is_none());
    assert!(document.resolve_position(&after).is_none());
}

#[test]
fn formatting_preserves_middle_identities_and_peer_insertions() {
    let mut alice = Document::new();
    let id = alice.insert_section(None, "alpha omega").unwrap();
    alice.clear_history();
    let anchor = alice
        .relative_position(&id, &node(&alice, &id), 6, Assoc::After)
        .unwrap();
    let mut bob = replica(&alice);
    let mut formatted = alice.sections()[0].content.clone();
    for run in &mut formatted.blocks[0].runs {
        run.marks.bold = true;
    }
    alice.set_content(&id, &formatted).unwrap();
    let mut edited = bob.sections()[0].content.clone();
    edited.blocks[0].runs[0].text = "alpha brave omega".into();
    bob.set_content(&id, &edited).unwrap();
    exchange(&mut alice, &mut bob);
    assert_eq!(body_content(&alice, &id).plain_text(), "alpha brave omega");
    assert!(
        body_content(&alice, &id).blocks[0]
            .runs
            .iter()
            .all(|run| run.marks.bold)
    );
    assert_eq!(
        alice.resolve_position(&anchor),
        Some((id.clone(), anchor.node().to_owned(), 12))
    );
    assert!(alice.undo());
    assert_eq!(body_content(&alice, &id).plain_text(), "alpha brave omega");
    assert!(
        body_content(&alice, &id).blocks[0]
            .runs
            .iter()
            .all(|run| !run.marks.bold)
    );
    exchange(&mut alice, &mut bob);
    assert_eq!(body_content(&bob, &id).plain_text(), "alpha brave omega");
    assert!(alice.redo());
    assert!(
        body_content(&alice, &id).blocks[0]
            .runs
            .iter()
            .all(|run| run.marks.bold)
    );
}

#[test]
fn undo_redo_preserves_concurrent_peer_deletions() {
    let mut alice = Document::new();
    let id = alice.insert_section(None, "hello world").unwrap();
    alice.clear_history();
    let mut bob = replica(&alice);
    edit_text(&mut alice, &id, "hello brave world").unwrap();
    edit_text(&mut bob, &id, "hello wor").unwrap();
    exchange(&mut alice, &mut bob);
    assert_eq!(alice.markdown(), "hello brave wor");
    assert!(alice.undo());
    assert_eq!(alice.markdown(), "hello wor");
    assert!(alice.redo());
    assert_eq!(alice.markdown(), "hello brave wor");
}

#[test]
fn local_undo_does_not_remove_peer_formatting() {
    let mut alice = Document::new();
    let id = alice.insert_section(None, "hello world").unwrap();
    alice.clear_history();
    let mut bob = replica(&alice);
    edit_text(&mut alice, &id, "hello brave world").unwrap();
    let mut content = body_content(&bob, &id);
    content.blocks[0].runs[0].marks.bold = true;
    bob.set_content(&id, &content).unwrap();
    exchange(&mut alice, &mut bob);
    assert_eq!(body_content(&alice, &id).plain_text(), "hello brave world");
    assert!(
        body_content(&alice, &id).blocks[0]
            .runs
            .iter()
            .all(|run| run.marks.bold)
    );
    assert!(alice.undo());
    assert_eq!(body_content(&alice, &id).plain_text(), "hello world");
    assert!(
        body_content(&alice, &id).blocks[0]
            .runs
            .iter()
            .all(|run| run.marks.bold)
    );
    assert!(alice.redo());
    assert_eq!(body_content(&alice, &id).plain_text(), "hello brave world");
    assert!(
        body_content(&alice, &id).blocks[0]
            .runs
            .iter()
            .all(|run| run.marks.bold)
    );
}

#[test]
fn remote_updates_after_local_undo_preserve_redo() {
    let mut alice = Document::new();
    let id = alice.insert_section(None, "hello world").unwrap();
    alice.clear_history();
    let mut bob = replica(&alice);
    edit_text(&mut alice, &id, "hello local world").unwrap();
    assert!(alice.undo());
    assert!(alice.can_redo());
    edit_text(&mut bob, &id, "hello world!").unwrap();
    alice.apply_update(&bob.encode_state()).unwrap();
    assert!(alice.can_redo());
    assert!(alice.redo());
    assert_eq!(alice.markdown(), "hello local world!");
}

#[test]
fn bootstrap_raw_and_framed_updates_are_not_local_history() {
    let mut peer = Document::new();
    let id = peer.insert_section(None, "remote").unwrap();
    let mut document = replica(&peer);
    assert!(!document.can_undo());
    assert!(!document.undo());
    let vector = document.state_vector();
    edit_text(&mut peer, &id, "remote edited").unwrap();
    let framed =
        Message::Sync(SyncMessage::Update(peer.encode_update(&vector).unwrap())).encode_v1();
    document.receive_sync(&framed).unwrap();
    assert_eq!(document.markdown(), "remote edited");
    assert!(!document.can_undo());
    assert!(!document.can_redo());
    edit_text(&mut document, &id, "local remote edited").unwrap();
    assert!(document.undo());
    let vector = document.state_vector();
    edit_text(&mut peer, &id, "remote edited!").unwrap();
    let framed =
        Message::Sync(SyncMessage::Update(peer.encode_update(&vector).unwrap())).encode_v1();
    document.receive_sync(&framed).unwrap();
    assert!(document.can_redo());
    assert!(document.redo());
    assert_eq!(document.markdown(), "local remote edited!");
}

#[test]
fn structural_commands_have_independent_undo_items() {
    let mut document = Document::new();
    let first = document.insert_section(None, "first").unwrap();
    let second = document.insert_section(None, "second").unwrap();
    assert!(document.undo());
    assert_eq!(ids(&document), [first.clone()]);
    assert!(document.redo());
    assert_eq!(ids(&document), [first.clone(), second.clone()]);
    document.clear_history();
    edit_text(&mut document, &first, "first edited").unwrap();
    document.move_section(&second, Some(&first)).unwrap();
    edit_text(&mut document, &second, "second edited").unwrap();
    assert!(document.undo());
    assert_eq!(
        crate::block::content_markdown(&document.sections()[0].content),
        "second"
    );
    assert!(document.undo());
    assert_eq!(ids(&document), [first.clone(), second.clone()]);
    assert_eq!(
        crate::block::content_markdown(&document.sections()[0].content),
        "first edited"
    );
    assert!(document.undo());
    assert_eq!(
        crate::block::content_markdown(&document.sections()[0].content),
        "first"
    );
    assert!(document.redo());
    assert!(document.redo());
    assert_eq!(ids(&document), [second, first]);
}

#[test]
fn undo_skips_moves_overwritten_by_a_peer() {
    let mut alice = Document::new();
    let first = alice.insert_section(None, "first").unwrap();
    let second = alice.insert_section(None, "second").unwrap();
    let third = alice.insert_section(None, "third").unwrap();
    alice.clear_history();
    let mut bob = replica(&alice);
    alice.move_section(&third, Some(&first)).unwrap();
    exchange(&mut alice, &mut bob);
    bob.move_section(&third, Some(&second)).unwrap();
    exchange(&mut alice, &mut bob);
    assert_eq!(ids(&alice), [first.clone(), third.clone(), second.clone()]);
    assert!(!alice.undo());
    assert_eq!(ids(&alice), [first, third, second]);
    assert!(!alice.redo());
}

#[test]
fn section_delete_undo_restores_nested_peer_text() {
    let mut alice = Document::new();
    let id = alice.insert_section(None, "a😀source").unwrap();
    let mut bob = replica(&alice);
    edit_text(&mut bob, &id, "a😀peer source").unwrap();
    exchange(&mut alice, &mut bob);
    alice.clear_history();
    let anchor = alice
        .relative_position(&id, &node(&alice, &id), 5, Assoc::After)
        .unwrap();
    alice.remove_section(&id).unwrap();
    assert!(alice.resolve_position(&anchor).is_none());
    assert!(alice.undo());
    assert_eq!(alice.markdown(), "a😀peer source");
    assert_eq!(
        alice.resolve_position(&anchor),
        Some((id.clone(), anchor.node().to_owned(), 5))
    );
    assert!(alice.redo());
    assert!(alice.sections().is_empty());
    assert!(alice.resolve_position(&anchor).is_none());
    assert!(alice.undo());
    assert_eq!(alice.markdown(), "a😀peer source");
}

#[test]

fn typing_section_changes_are_history_boundaries() {
    let mut document = Document::new();
    let first = document.insert_section(None, "a").unwrap();
    let second = document.insert_section(None, "b").unwrap();
    document.clear_history();
    edit_text(&mut document, &first, "a1").unwrap();
    edit_text(&mut document, &second, "b1").unwrap();
    edit_text(&mut document, &first, "a12").unwrap();
    assert!(document.undo());
    assert_eq!(document.sections()[0].content.plain_text(), "a1");
    assert_eq!(document.sections()[1].content.plain_text(), "b1");
    assert!(document.undo());
    assert_eq!(document.sections()[1].content.plain_text(), "b");
}

#[test]
fn nested_explicit_groups_make_structural_and_text_edits_atomic() {
    let mut document = Document::new();
    let first = document.insert_section(None, "/table").unwrap();
    let original = document.sections();
    document.clear_history();
    document.begin_history_group();
    document.stop_capturing();
    let table = RichDocument::from_markdown("| a | b |\n| --- | --- |").unwrap();
    document.set_content(&first, &table).unwrap();
    document.stop_capturing();
    document.begin_history_group();
    let second = document.insert_section(None, "").unwrap();
    document.end_history_group();
    document.stop_capturing();
    edit_text(&mut document, &second, "next").unwrap();
    document.stop_capturing();
    document.end_history_group();
    edit_text(&mut document, &second, "next typed").unwrap();
    assert!(document.undo());
    assert_eq!(
        crate::block::content_markdown(&document.sections()[1].content),
        "next"
    );
    assert!(document.undo());
    assert_eq!(document.sections(), original);
    assert!(document.redo());
    assert_eq!(ids(&document), [first, second]);
    assert_eq!(
        crate::block::content_markdown(&document.sections()[1].content),
        "next"
    );
}

#[test]
fn explicit_capture_stops_and_history_clear_are_boundaries() {
    let mut document = Document::new();
    let id = document.insert_section(None, "a").unwrap();
    document.clear_history();
    edit_text(&mut document, &id, "ab").unwrap();
    document.stop_capturing();
    edit_text(&mut document, &id, "abc").unwrap();
    assert!(document.undo());
    assert_eq!(document.markdown(), "ab");
    assert!(document.can_redo());
    document.clear_history();
    assert!(!document.can_undo());
    assert!(!document.can_redo());
    assert!(!document.undo());
    assert!(!document.redo());
    assert_eq!(document.markdown(), "ab");
}

#[test]
fn deferred_sync_validates_every_frame_and_replies_without_applying() {
    let mut local = Document::new();
    let id = local.insert_section(None, "local").unwrap();
    local.clear_history();
    let mut peer = Document::new();
    peer.insert_section(None, "remote").unwrap();
    let mut frames = Message::Sync(SyncMessage::Update(peer.encode_state())).encode_v1();
    frames.extend(peer.start_sync());
    frames.extend(Message::Sync(SyncMessage::SyncStep2(peer.encode_state())).encode_v1());
    let snapshot = local.encode_state();
    let replies = local.reply_sync_without_applying(&frames).unwrap();
    assert_eq!(replies.len(), 1);
    assert_eq!(local.encode_state(), snapshot);
    assert!(!local.can_undo());
    peer.receive_sync(&replies[0]).unwrap();
    assert!(peer.sections().iter().any(|section| section.id == id));
    local.receive_sync(&frames).unwrap();
    assert_eq!(local.sections(), peer.sections());
    assert!(!local.can_undo());
    frames.push(0);
    assert!(local.reply_sync_without_applying(&frames).is_err());
    assert!(local.reply_sync_without_applying(&[]).is_err());
}

fn body_content(document: &Document, id: &str) -> RichDocument {
    document
        .sections()
        .into_iter()
        .find(|section| section.id == id)
        .unwrap()
        .content
}

fn leaf(document: &Document, id: &str) -> TextRef {
    let node = node(document, id);
    let txn = document.doc.transact();
    rich_storage::node_text(&document.section_map(&txn, id).unwrap(), &txn, &node).unwrap()
}

#[test]
fn concurrent_independent_marks_and_text_merge_without_replacing_text() {
    let mut bold = Document::new();
    let id = bold.insert_section(None, "hello world").unwrap();
    bold.clear_history();
    let original = leaf(&bold, &id);
    let anchor = bold
        .relative_position(&id, &node(&bold, &id), 6, Assoc::After)
        .unwrap();
    let mut italic = replica(&bold);
    let mut typing = replica(&bold);
    let mut content = body_content(&bold, &id);
    content.blocks[0].runs[0].marks.bold = true;
    bold.set_content(&id, &content).unwrap();
    let mut content = body_content(&italic, &id);
    content.blocks[0].runs[0].marks.italic = true;
    italic.set_content(&id, &content).unwrap();
    let mut content = body_content(&typing, &id);
    content.blocks[0].runs[0].text = "hello brave world".into();
    typing.set_content(&id, &content).unwrap();
    exchange(&mut bold, &mut italic);
    exchange(&mut bold, &mut typing);
    exchange(&mut italic, &mut typing);
    exchange(&mut bold, &mut italic);
    let content = body_content(&bold, &id);
    assert_eq!(content.plain_text(), "hello brave world");
    assert!(
        content.blocks[0]
            .runs
            .iter()
            .all(|run| run.marks.bold && run.marks.italic)
    );
    assert_eq!(leaf(&bold, &id), original);
    assert_eq!(bold.resolve_position(&anchor).unwrap().2, 12);
    assert!(bold.undo());
    let content = body_content(&bold, &id);
    assert_eq!(content.plain_text(), "hello brave world");
    assert!(
        content.blocks[0]
            .runs
            .iter()
            .all(|run| !run.marks.bold && run.marks.italic)
    );
    assert_eq!(leaf(&bold, &id), original);
}

#[test]
fn formatting_only_keeps_text_and_noop_keeps_state_vector() {
    let mut document = Document::new();
    let id = document.insert_section(None, "a😀終").unwrap();
    let original = leaf(&document, &id);
    let anchor = document
        .relative_position(&id, &node(&document, &id), 5, Assoc::After)
        .unwrap();
    let mut content = body_content(&document, &id);
    content.blocks[0].runs[0].marks.bold = true;
    document.set_content(&id, &content).unwrap();
    assert_eq!(leaf(&document, &id), original);
    assert_eq!(document.resolve_position(&anchor).unwrap().2, 5);
    let state = document.state_vector();
    document.set_content(&id, &content).unwrap();
    assert_eq!(document.state_vector(), state);
}

#[test]
fn semantic_deletion_and_structural_node_anchors_are_safe() {
    use crate::native_editor::rich::{RichKind, RichNode};
    let mut document = Document::new();
    let id = document.insert_section(None, "body").unwrap();
    let mut content = body_content(&document, &id);
    let mut wrapper = RichNode::paragraph("");
    wrapper.kind = RichKind::Quote;
    wrapper.children = content.blocks;
    let wrapper_id = wrapper.id.clone();
    content.blocks = vec![wrapper];
    document.set_content(&id, &content).unwrap();
    let structural = document
        .relative_position(&id, &wrapper_id, 0, Assoc::After)
        .unwrap();
    let text_id = content.blocks[0].children[0].id.clone();
    let text = document
        .relative_position(&id, &text_id, 2, Assoc::After)
        .unwrap();
    assert_eq!(
        document.resolve_position(&structural).unwrap().1,
        wrapper_id
    );
    content.blocks[0].children.clear();
    document.set_content(&id, &content).unwrap();
    assert!(document.resolve_position(&text).is_none());
    assert!(document.resolve_position(&structural).is_some());
}

#[test]
fn unsupported_schema_and_mixed_sync_frames_reject_atomically() {
    let mut document = Document::new();
    document.insert_section(None, "keep").unwrap();
    document.clear_history();
    let before = document.encode_state();
    let unknown = Doc::new();
    let meta = unknown.get_or_insert_map("meta");
    {
        let mut txn = unknown.transact_mut();
        meta.insert(&mut txn, "schema", "gpui-kit-block-editor");
        meta.insert(&mut txn, "version", i64::from(RICH_DOCUMENT_VERSION + 1));
    }
    let unknown = unknown
        .transact()
        .encode_state_as_update_v1(&StateVector::default());
    assert!(document.apply_update(&unknown).is_err());
    let mut peer = Document::new();
    peer.insert_section(None, "must not partially appear")
        .unwrap();
    let mut frames = Message::Sync(SyncMessage::Update(peer.encode_state())).encode_v1();
    frames.extend(Message::Sync(SyncMessage::Update(unknown)).encode_v1());
    assert!(document.receive_sync(&frames).is_err());
    assert!(document.reply_sync_without_applying(&frames).is_err());
    assert_eq!(document.encode_state(), before);
    assert!(!document.can_undo());
}

#[test]
fn json_roundtrip_preserves_semantics_and_ids_but_not_yjs_identity() {
    let mut document = Document::new();
    let title = document.insert_section(None, "**Title**").unwrap();
    let child = document.wrap_collapsible(&title).unwrap();
    edit_text(&mut document, &child, "child 😀").unwrap();
    let anchor = document
        .relative_position(&child, &node(&document, &child), 2, Assoc::After)
        .unwrap();
    let json = document.export_json().unwrap();
    let restored = Document::import_json(&json).unwrap();
    assert_eq!(restored.sections(), document.sections());
    assert!(!restored.can_undo());
    assert!(restored.resolve_position(&anchor).is_none());
    let mut invalid: serde_json::Value = serde_json::from_str(&json).unwrap();
    invalid["version"] = (RICH_DOCUMENT_VERSION + 1).into();
    assert!(Document::import_json(&invalid.to_string()).is_err());
    invalid["version"] = RICH_DOCUMENT_VERSION.into();
    invalid["sections"][1]["id"] = invalid["sections"][0]["id"].clone();
    assert!(Document::import_json(&invalid.to_string()).is_err());
    let mut invalid: serde_json::Value = serde_json::from_str(&json).unwrap();
    invalid["sections"][0]["content"]["version"] = (RICH_DOCUMENT_VERSION + 1).into();
    assert!(Document::import_json(&invalid.to_string()).is_err());
}

#[test]
fn invalid_semantic_snapshot_does_not_write_or_create_history() {
    let mut document = Document::new();
    let id = document.insert_section(None, "keep").unwrap();
    document.clear_history();
    let before = document.encode_state();
    let mut invalid = body_content(&document, &id);
    invalid.blocks.push(invalid.blocks[0].clone());
    assert!(document.set_content(&id, &invalid).is_err());
    assert_eq!(document.encode_state(), before);
    assert!(!document.can_undo());
}
