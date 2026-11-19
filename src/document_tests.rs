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
            let position = document.relative_position(&id, byte, assoc).unwrap();
            assert_eq!(position.section(), id);
            assert_eq!(
                document.resolve_position(&position),
                Some((id.clone(), byte))
            );
        }
    }
    for byte in [2, 3, 4, 7, 9, 10, 12, usize::MAX] {
        assert!(document.relative_position(&id, byte, Assoc::After).is_err());
    }
    assert!(
        document
            .relative_position("missing", 0, Assoc::After)
            .is_err()
    );
}

#[test]
fn anchors_follow_unicode_insertions_and_association() {
    let mut document = Document::new();
    let id = document.insert_section(None, "a😀終").unwrap();
    let before = document.relative_position(&id, 5, Assoc::Before).unwrap();
    let after = document.relative_position(&id, 5, Assoc::After).unwrap();
    let start = document.relative_position(&id, 0, Assoc::Before).unwrap();
    let end = document.relative_position(&id, 8, Assoc::After).unwrap();
    let mut peer = replica(&document);
    peer.set_markdown(&id, "漢a😀🚀終!").unwrap();
    document.apply_update(&peer.encode_state()).unwrap();
    assert_eq!(document.resolve_position(&before), Some((id.clone(), 8)));
    assert_eq!(document.resolve_position(&after), Some((id.clone(), 12)));
    assert_eq!(document.resolve_position(&start), Some((id.clone(), 0)));
    assert_eq!(
        document.resolve_position(&end),
        Some((id, "漢a😀🚀終!".len()))
    );
}

#[test]
fn deleted_text_anchors_collapse_and_follow_undo_redo() {
    let mut document = Document::new();
    let id = document.insert_section(None, "a😀middle終").unwrap();
    let anchor = document.relative_position(&id, 8, Assoc::After).unwrap();
    document.clear_history();
    document.set_markdown(&id, "a😀終").unwrap();
    assert_eq!(document.resolve_position(&anchor), Some((id.clone(), 5)));
    assert!(document.undo());
    assert_eq!(document.resolve_position(&anchor), Some((id.clone(), 8)));
    assert!(document.redo());
    assert_eq!(document.resolve_position(&anchor), Some((id, 5)));
}

#[test]
fn deleted_parent_anchor_resolution_never_resurrects() {
    let mut document = Document::new();
    let id = document.insert_section(None, "😀source").unwrap();
    let anchor = document.relative_position(&id, 4, Assoc::After).unwrap();
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
    let before = document.relative_position(&id, 0, Assoc::Before).unwrap();
    let after = document.relative_position(&id, 0, Assoc::After).unwrap();
    document.set_markdown(&id, "😀").unwrap();
    assert_eq!(document.resolve_position(&before), Some((id.clone(), 0)));
    assert_eq!(document.resolve_position(&after), Some((id.clone(), 4)));
    {
        let mut txn = document.doc.transact_mut_with(REMOTE_ORIGIN);
        let section = document.section_map(&txn, &id).unwrap();
        section.insert(&mut txn, "source", TextPrelim::new("replacement"));
    }
    assert!(document.resolve_position(&before).is_none());
    assert!(document.resolve_position(&after).is_none());
}

#[test]
fn formatting_preserves_middle_identities_and_peer_insertions() {
    let mut alice = Document::new();
    let id = alice.insert_section(None, "alpha omega").unwrap();
    alice.clear_history();
    let anchor = alice.relative_position(&id, 6, Assoc::After).unwrap();
    let mut bob = replica(&alice);
    alice.set_markdown(&id, "**alpha omega**").unwrap();
    bob.set_markdown(&id, "alpha brave omega").unwrap();
    exchange(&mut alice, &mut bob);
    assert_eq!(alice.markdown(), "**alpha brave omega**");
    assert_eq!(alice.resolve_position(&anchor), Some((id.clone(), 14)));
    assert!(alice.undo());
    assert_eq!(alice.markdown(), "alpha brave omega");
    exchange(&mut alice, &mut bob);
    assert_eq!(bob.markdown(), "alpha brave omega");
    assert!(alice.redo());
    assert_eq!(alice.markdown(), "**alpha brave omega**");
}

#[test]
fn undo_redo_preserves_concurrent_peer_deletions() {
    let mut alice = Document::new();
    let id = alice.insert_section(None, "hello world").unwrap();
    alice.clear_history();
    let mut bob = replica(&alice);
    alice.set_markdown(&id, "hello brave world").unwrap();
    bob.set_markdown(&id, "hello wor").unwrap();
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
    alice.set_markdown(&id, "hello brave world").unwrap();
    bob.set_markdown(&id, "**hello world**").unwrap();
    exchange(&mut alice, &mut bob);
    assert_eq!(alice.markdown(), "**hello brave world**");
    assert!(alice.undo());
    assert_eq!(alice.markdown(), "**hello world**");
    assert!(alice.redo());
    assert_eq!(alice.markdown(), "**hello brave world**");
}

#[test]
fn remote_updates_after_local_undo_preserve_redo() {
    let mut alice = Document::new();
    let id = alice.insert_section(None, "hello world").unwrap();
    alice.clear_history();
    let mut bob = replica(&alice);
    alice.set_markdown(&id, "hello local world").unwrap();
    assert!(alice.undo());
    assert!(alice.can_redo());
    bob.set_markdown(&id, "hello world!").unwrap();
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
    peer.set_markdown(&id, "remote edited").unwrap();
    let framed =
        Message::Sync(SyncMessage::Update(peer.encode_update(&vector).unwrap())).encode_v1();
    document.receive_sync(&framed).unwrap();
    assert_eq!(document.markdown(), "remote edited");
    assert!(!document.can_undo());
    assert!(!document.can_redo());
    document.set_markdown(&id, "local remote edited").unwrap();
    assert!(document.undo());
    let vector = document.state_vector();
    peer.set_markdown(&id, "remote edited!").unwrap();
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
    document.set_markdown(&first, "first edited").unwrap();
    document.move_section(&second, Some(&first)).unwrap();
    document.set_markdown(&second, "second edited").unwrap();
    assert!(document.undo());
    assert_eq!(document.sections()[0].markdown, "second");
    assert!(document.undo());
    assert_eq!(ids(&document), [first.clone(), second.clone()]);
    assert_eq!(document.sections()[0].markdown, "first edited");
    assert!(document.undo());
    assert_eq!(document.sections()[0].markdown, "first");
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
    bob.set_markdown(&id, "a😀peer source").unwrap();
    exchange(&mut alice, &mut bob);
    alice.clear_history();
    let anchor = alice.relative_position(&id, 5, Assoc::After).unwrap();
    alice.remove_section(&id).unwrap();
    assert!(alice.resolve_position(&anchor).is_none());
    assert!(alice.undo());
    assert_eq!(alice.markdown(), "a😀peer source");
    assert_eq!(alice.resolve_position(&anchor), Some((id.clone(), 5)));
    assert!(alice.redo());
    assert!(alice.sections().is_empty());
    assert!(alice.resolve_position(&anchor).is_none());
    assert!(alice.undo());
    assert_eq!(alice.markdown(), "a😀peer source");
}

#[test]
fn typing_uses_the_standard_timeout_and_section_boundaries() {
    let mut document = Document::new();
    let first = document.insert_section(None, "a").unwrap();
    let second = document.insert_section(None, "b").unwrap();
    document.clear_history();
    // Drive the same clock used by UndoManager without sleeping in the test.
    document.history_time.store(1000, Ordering::Relaxed);
    document.set_markdown(&first, "ab").unwrap();
    document.history_time.store(1499, Ordering::Relaxed);
    document.set_markdown(&first, "abc").unwrap();
    document.history_time.store(1999, Ordering::Relaxed);
    document.set_markdown(&first, "abcd").unwrap();
    assert!(document.undo());
    assert_eq!(document.sections()[0].markdown, "abc");
    assert!(document.undo());
    assert_eq!(document.sections()[0].markdown, "a");
    assert!(!document.can_undo());
    document.set_markdown(&first, "a1").unwrap();
    document.set_markdown(&second, "b1").unwrap();
    document.set_markdown(&first, "a12").unwrap();
    assert!(document.undo());
    assert_eq!(document.sections()[0].markdown, "a1");
    assert_eq!(document.sections()[1].markdown, "b1");
    assert!(document.undo());
    assert_eq!(document.sections()[1].markdown, "b");
}

#[test]
fn nested_explicit_groups_make_structural_and_text_edits_atomic() {
    let mut document = Document::new();
    let first = document.insert_section(None, "/table").unwrap();
    document.clear_history();
    document.begin_history_group();
    let frozen = document.history_time.load(Ordering::Relaxed);
    assert_ne!(frozen, 0);
    document.stop_capturing();
    document
        .set_markdown(&first, "| a | b |\n| --- | --- |")
        .unwrap();
    document.stop_capturing();
    document.begin_history_group();
    let second = document.insert_section(None, "").unwrap();
    document.end_history_group();
    assert_eq!(document.history_time.load(Ordering::Relaxed), frozen);
    document.stop_capturing();
    document.set_markdown(&second, "next").unwrap();
    document.stop_capturing();
    document.end_history_group();
    assert_eq!(document.history_time.load(Ordering::Relaxed), 0);
    assert_eq!(document.history.undo_stack().len(), 1);
    document.set_markdown(&second, "next typed").unwrap();
    assert!(document.undo());
    assert_eq!(document.sections()[1].markdown, "next");
    assert!(document.undo());
    assert_eq!(
        document.sections(),
        [Section {
            id: first.clone(),
            markdown: "/table".into(),
            parent: None,
            kind: SectionKind::Text,
            depth: 0,
        }]
    );
    assert!(document.redo());
    assert_eq!(ids(&document), [first, second]);
    assert_eq!(document.sections()[1].markdown, "next");
}

#[test]
fn explicit_capture_stops_and_history_clear_are_boundaries() {
    let mut document = Document::new();
    let id = document.insert_section(None, "a").unwrap();
    document.clear_history();
    document.set_markdown(&id, "ab").unwrap();
    document.stop_capturing();
    document.set_markdown(&id, "abc").unwrap();
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
