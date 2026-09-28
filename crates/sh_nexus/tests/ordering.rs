//! `core/ordering.rs`: ordering, deduplication, and gap detection.
//!
//! The module under test is the one that decides whether a chat client is
//! *correct*. A duplicate that is shown twice, or a message that silently never
//! arrives, is not a cosmetic bug — it is the client lying about what was said.
//! So these tests are written around the claims the module makes rather than
//! around its statements:
//!
//! | Claim | Where it is pinned |
//! |---|---|
//! | The order is a **total** order, and it is a function of the message set | `permuting_a_batch_does_not_change_the_ordered_result`, `every_permutation_of_a_batch_reconciles_to_the_same_result` |
//! | Nothing is lost by deduplicating | `reconciliation_preserves_exactly_the_input_client_msg_ids` |
//! | Arbitrary input cannot panic | `arbitrary_messages_reconcile_without_panicking` |
//! | `edited_at` and `thread_id` cannot affect the order | `an_edit_does_not_move_a_message`, `a_thread_reply_is_ordered_as_a_root_message` |
//! | A disagreement is **reported**, not merged | the `DifferingField` cases, `an_edit_arriving_as_a_new_value_is_reported_rather_than_silently_merged` |
//! | A gap is **named**, never filled | `a_gap_names_an_interval_and_fills_nothing` |
//! | "Cannot tell" is not "no gap" | `is_healthy_is_false_for_every_status_but_complete` |
//!
//! # Style
//!
//! Parameterized cases use `#[rstest]` with `#[case]`, per `AGENTS.md` §4.3 and
//! ADR-008. Work unit 1A's `for (name, case, expected) in cases` tables are
//! grandfathered, not blessed, and the rule applies from 1B. The signal is
//! better, too: a `#[case]` failure names the case that broke and the others
//! still report as passing.
//!
//! Properties use `proptest`, per `AGENTS.md` §4.4. The permutation invariant is
//! additionally checked *exhaustively* over all 120 orderings of a five-message
//! batch, because "for any permutation" is a claim that a random sample
//! supports and an enumeration proves.
//!
//! Every fixture is built from fixed values. No test reads a clock and none
//! generates an id at random, so a failure is reproducible from its own name.

use std::cmp::Ordering as CmpOrdering;
use std::collections::HashSet;

use chrono::{DateTime, TimeZone, Utc};
use proptest::prelude::*;
use rstest::rstest;
use sh_nexus::core::models::{Attachment, Message, Reaction};
use sh_nexus::core::ordering::{
    assess_sync, compare, reconcile, DifferingField, IngestOutcome, OrderedMessages,
    SyncExpectation, SyncStatus,
};
use smallvec::SmallVec;
use uuid::Uuid;

/// The instant every fixture is anchored to: 2026-09-27T14:26:40Z, written as an
/// offset because a `DateTime` built from a wall clock makes a test's output
/// depend on when it ran.
const BASE_SECOND: i64 = 1_789_000_000;

/// `BASE_SECOND` plus an offset, as a `DateTime<Utc>`.
///
/// The range `0..3` used throughout deliberately causes **timestamp ties**, so
/// the tiebreaker is exercised by nearly every test here rather than by one
/// test that happens to construct a tie.
fn at(second: i64) -> DateTime<Utc> {
    Utc.timestamp_opt(BASE_SECOND + second, 0)
        .single()
        .expect("the fixture offset is inside chrono's range")
}

/// A message with every field at a known value.
///
/// `id` is derived from `client` so two fixtures never collide by accident, and
/// `timestamp` is derived from `second` alone, so passing the same `second`
/// twice is how a test asks for a tie.
fn message(id: &str, client: u128, second: i64) -> Message {
    Message {
        id: id.to_owned(),
        client_msg_id: Uuid::from_u128(client),
        channel_id: "c_1".to_owned(),
        user_id: "u_1".to_owned(),
        content: format!("body of {id}"),
        timestamp: at(second),
        edited_at: None,
        reactions: SmallVec::new(),
        thread_id: None,
        attachments: SmallVec::new(),
    }
}

/// A message the server has not acknowledged yet: no server `id`.
///
/// The empty `id` is not a placeholder this test suite invented — it is what
/// `core/ordering.rs`'s `precedence` treats as "not yet known to the server", and
/// it is the shape an optimistic send has on the wire (`PLAN.md` §6's
/// `message.ack` is what supplies the real one).
fn unacked(client: u128, second: i64) -> Message {
    let mut pending = message("", client, second);
    pending.content = format!("pending {}", client);
    pending
}

/// The ids of an ordered set, in order.
fn ids(ordered: &OrderedMessages) -> Vec<&str> {
    ordered.messages().iter().map(|m| m.id.as_str()).collect()
}

/// A small batch built to be nasty: two timestamp ties, one unacknowledged
/// message, one thread reply, one edited message.
fn awkward_batch() -> Vec<Message> {
    let mut reply = message("m_3", 3, 1);
    reply.thread_id = Some("m_1".to_owned());

    let mut edited = message("m_4", 4, 1);
    edited.edited_at = Some(at(500));

    vec![
        message("m_2", 2, 1),
        unacked(1, 0),
        reply,
        message("m_0", 5, 0),
        edited,
    ]
}

/// Every ordering of `items`, by recursive rotation.
fn permutations<T: Clone>(items: &[T]) -> Vec<Vec<T>> {
    if items.is_empty() {
        return vec![Vec::new()];
    }
    let mut out = Vec::new();
    for index in 0..items.len() {
        let mut rest = items.to_vec();
        let head = rest.remove(index);
        for tail in permutations(&rest) {
            let mut ordered = Vec::with_capacity(items.len());
            ordered.push(head.clone());
            ordered.extend(tail);
            out.push(ordered);
        }
    }
    out
}

// ---------------------------------------------------------------------------
// Ordering
// ---------------------------------------------------------------------------

/// The order is oldest-first, whatever order the messages arrived in.
#[test]
fn messages_are_ordered_oldest_first_whatever_the_arrival_order() {
    let out = reconcile(vec![
        message("m_3", 3, 3),
        message("m_1", 1, 1),
        message("m_2", 2, 2),
    ]);

    assert_eq!(ids(&out), ["m_1", "m_2", "m_3"]);
}

/// The contract is **ascending**, and it says so: a caller that wants a
/// bottom-anchored newest-first list reverses, it does not re-sort.
///
/// A `DateTime` for 1970 is the natural way to say "the beginning of time" in a
/// test, and it is also a good way to be wrong: `at(-1)` is *before* the epoch,
/// so the assertion below is only meaningful because the comparator is
/// timestamp-based and not string-based.
#[test]
fn the_order_is_ascending_by_time_not_by_identifier() {
    let out = reconcile(vec![message("m_aaa", 1, 2), message("m_zzz", 2, 1)]);

    assert_eq!(
        out.messages()[0].timestamp,
        at(1),
        "the older message comes first even though its id sorts later"
    );
    assert_eq!(ids(&out), ["m_zzz", "m_aaa"]);
}

/// Ties on `timestamp` are normal, and the server's id breaks them.
#[rstest]
#[case("m_1", 1, "m_2", 2, ["m_1", "m_2"])]
#[case("m_2", 2, "m_1", 1, ["m_1", "m_2"])]
#[case("m_10", 10, "m_2", 2, ["m_10", "m_2"])]
#[case("m_2", 2, "m_10", 10, ["m_10", "m_2"])]
fn a_timestamp_tie_is_broken_by_the_server_id(
    #[case] left_id: &str,
    #[case] left_client: u128,
    #[case] right_id: &str,
    #[case] right_client: u128,
    #[case] expected: [&str; 2],
) {
    // Both messages land in the same millisecond, on purpose.
    let left = message(left_id, left_client, 7);
    let right = message(right_id, right_client, 7);
    assert_eq!(
        left.timestamp, right.timestamp,
        "the case is meaningless unless the timestamps tie"
    );

    let out = reconcile(vec![left, right]);

    assert_eq!(ids(&out), expected);
}

/// Two unacknowledged messages share an empty `id`, so `id` cannot break their
/// tie — `client_msg_id` has to, or the order is not a total order at all.
///
/// This is the case that makes the third key component load-bearing rather than
/// decorative: it is exactly the shape two optimistic sends in the same
/// millisecond produce.
#[test]
fn a_timestamp_tie_with_no_server_id_is_broken_by_the_client_msg_id() {
    let out = reconcile(vec![unacked(20, 5), unacked(3, 5)]);

    assert_eq!(out.messages().len(), 2);
    assert_eq!(
        out.messages()[0].client_msg_id,
        Uuid::from_u128(3),
        "with no server id to compare, the client id decides"
    );
    assert_eq!(
        compare(&out.messages()[0], &out.messages()[1]),
        CmpOrdering::Less
    );
}

/// The tiebreaker is lexicographic, and that is the accepted cost.
///
/// Asserted directly rather than left as prose, because the day somebody
/// "fixes" it with a numeric-aware comparison this test should fail and force
/// the conversation about whether the cost is worth paying.
#[test]
fn the_id_tiebreaker_is_lexicographic_which_is_the_documented_cost() {
    let out = reconcile(vec![message("m_2", 2, 4), message("m_10", 10, 4)]);

    assert_eq!(
        ids(&out),
        ["m_10", "m_2"],
        "`m_10` sorts before `m_2` as bytes; that is the price of a total order \
         that two clients and a database reload all reproduce identically"
    );
}

/// `edited_at` is not in the order key, and must never be.
///
/// The construction is the whole point: the *edited* message has the id that
/// sorts first, so a comparator that leaked `edited_at` into the key would put
/// it second and this fails.
#[test]
fn an_edit_does_not_move_a_message() {
    let mut edited = message("m_1", 1, 4);
    edited.edited_at = Some(at(9_999));

    let out = reconcile(vec![edited, message("m_2", 2, 4)]);

    assert_eq!(
        ids(&out),
        ["m_1", "m_2"],
        "an edit changes the message's content, never its position"
    );
}

/// A thread reply is a root message as far as top-level ordering goes.
///
/// The reply has the *later* id and the *earlier* timestamp, so a
/// thread-aware comparator that grouped replies under their parent would put it
/// first and this fails.
#[test]
fn a_thread_reply_is_ordered_as_a_root_message() {
    let mut reply = message("m_zzz", 9, 0);
    reply.thread_id = Some("m_1".to_owned());

    let out = reconcile(vec![reply, message("m_1", 1, 5)]);

    assert_eq!(
        ids(&out),
        ["m_zzz", "m_1"],
        "thread_id is not in the order key, so a reply interleaves by time"
    );
    assert_eq!(out.messages()[0].thread_id.as_deref(), Some("m_1"));
}

/// `compare` is a strict total order over distinct messages, so the output of
/// sorting cannot depend on the sort's stability.
#[test]
fn no_two_distinct_messages_compare_equal() {
    let out = reconcile(awkward_batch());

    for (index, left) in out.messages().iter().enumerate() {
        for right in &out.messages()[index + 1..] {
            assert_ne!(
                compare(left, right),
                CmpOrdering::Equal,
                "{left:?} and {right:?} share an order key, so their relative \
                 position is not determined"
            );
        }
    }
}

// ---------------------------------------------------------------------------
// Deduplication
// ---------------------------------------------------------------------------

/// The headline case: one message delivered twice is one message.
#[test]
fn a_duplicate_client_msg_id_collapses_to_one_message() {
    let original = message("m_1", 1, 3);

    let out = reconcile(vec![original.clone(), original.clone()]);

    assert_eq!(out.messages().len(), 1);
    assert_eq!(out.messages()[0], original);
}

/// The caller is told which of its own messages was the repeat, by position.
#[test]
fn outcomes_line_up_with_the_input_in_input_order() {
    let first = message("m_1", 1, 1);
    let second = message("m_2", 2, 2);

    let out = reconcile(vec![first.clone(), second.clone(), first.clone()]);

    assert_eq!(
        out.outcomes(),
        &[
            IngestOutcome::Added,
            IngestOutcome::Added,
            IngestOutcome::Collapsed
        ],
        "one outcome per message handed in, in the order it was handed in"
    );
    assert!(out.is_clean());
}

/// Identity is `client_msg_id`, so two *different* messages are two messages
/// even when they share everything else.
#[test]
fn two_distinct_messages_with_identical_content_are_both_kept() {
    let mut second = message("m_2", 2, 1);
    second.content = message("m_1", 1, 1).content;

    let out = reconcile(vec![message("m_1", 1, 1), second]);

    assert_eq!(out.messages().len(), 2);
    assert!(out.is_clean());
}

/// The ACK handoff: the optimistic copy and the acknowledged copy are one
/// message, and the acknowledged copy is the one kept.
///
/// This is the case that makes `client_msg_id` the right identity key. Deduping
/// on `id` instead would show the user's own message twice — the exact failure
/// the module exists to prevent, produced by the identity function itself.
#[test]
fn an_acknowledged_copy_replaces_the_optimistic_one() {
    let pending = unacked(7, 2);
    let mut acked = pending.clone();
    acked.id = "m_server_1".to_owned();

    let out = reconcile(vec![pending, acked.clone()]);

    assert_eq!(out.messages().len(), 1);
    assert_eq!(out.messages()[0], acked, "a real id beats no id");
    assert_eq!(
        out.outcomes()[1],
        IngestOutcome::Conflicting {
            this_copy_was_retained: true,
            fields: SmallVec::from_vec(vec![DifferingField::Id]),
        },
        "and the caller is told that the id is what changed"
    );
    assert!(
        !out.is_clean(),
        "a disagreement is never silently clean, even a legitimate one"
    );
}

/// An edit arrives as a whole new value carrying the original ids, exactly as
/// `Message::edited_at` documents. It is reported, not merged.
///
/// The module cannot tell an edit from two different messages sharing one
/// `client_msg_id`, and the report is the honest shape of that: the caller is
/// told precisely which fields moved, and `state/actions.rs` — which §3.2 makes
/// the sole owner of mutations — decides what to do.
#[test]
fn an_edit_arriving_as_a_new_value_is_reported_rather_than_silently_merged() {
    let original = message("m_1", 1, 2);
    let mut edited = original.clone();
    edited.content = "body of m_1, corrected".to_owned();
    edited.edited_at = Some(at(600));

    let out = reconcile(vec![original, edited.clone()]);

    assert_eq!(
        out.messages(),
        [edited],
        "the edited copy is the later truth"
    );
    assert_eq!(
        out.outcomes()[1],
        IngestOutcome::Conflicting {
            this_copy_was_retained: true,
            fields: SmallVec::from_vec(vec![DifferingField::Content, DifferingField::EditedAt]),
        },
        "named by field, in declaration order, and never by value -- \
         AGENTS.md 7.5 forbids message content in anything that can reach a log"
    );
}

/// Whichever copy the caller hands in *last* is not what decides: the later
/// truth wins, whichever side that is.
#[test]
fn the_earlier_copy_is_kept_when_a_later_one_arrives_after_it() {
    let mut edited = message("m_1", 1, 2);
    edited.edited_at = Some(at(600));
    let original = message("m_1", 1, 2);

    let out = reconcile(vec![edited, original]);

    assert_eq!(out.messages().len(), 1);
    assert_eq!(out.messages()[0].edited_at, Some(at(600)));
    assert_eq!(
        out.outcomes()[1],
        IngestOutcome::Conflicting {
            this_copy_was_retained: false,
            fields: SmallVec::from_vec(vec![DifferingField::EditedAt]),
        },
        "the flag tells the caller which of ITS two copies survived"
    );
}

/// Every field is detected, named correctly, and its retention is decided
/// exactly when the precedence chain covers it.
///
/// The third case parameter is the interesting one. Four fields are compared by
/// `precedence` (`id`, `content`, `timestamp`, `edited_at`) and five are not
/// (`channel_id`, `user_id`, `reactions`, `thread_id`, `attachments`), and a
/// change in one of the five leaves the retained copy to input order — the one
/// limit `precedence` documents. Asserting that split per field is what stops
/// the two lists from drifting apart silently.
#[rstest]
#[case(DifferingField::Id, "id", true, |m: &mut Message| m.id = "m_other".to_owned())]
#[case(DifferingField::ChannelId, "channel_id", false, |m: &mut Message| m.channel_id = "c_2".to_owned())]
#[case(DifferingField::UserId, "user_id", false, |m: &mut Message| m.user_id = "u_2".to_owned())]
#[case(DifferingField::Content, "content", true, |m: &mut Message| m.content = "body of m_1 rewritten".to_owned())]
#[case(DifferingField::Timestamp, "timestamp", true, |m: &mut Message| m.timestamp = at(8))]
#[case(DifferingField::EditedAt, "edited_at", true, |m: &mut Message| m.edited_at = Some(at(50)))]
#[case(DifferingField::Reactions, "reactions", false, |m: &mut Message| {
    m.reactions = SmallVec::from_vec(vec![Reaction {
        emoji: "\u{1f44d}".to_owned(),
        user_ids: vec!["u_1".to_owned()],
    }]);
})]
#[case(DifferingField::ThreadId, "thread_id", false, |m: &mut Message| m.thread_id = Some("m_0".to_owned()))]
#[case(DifferingField::Attachments, "attachments", false, |m: &mut Message| {
    m.attachments = SmallVec::from_vec(vec![Attachment {
        id: "a_1".to_owned(),
        filename: "shot.png".to_owned(),
        url: "https://example.invalid/shot.png".to_owned(),
        mime_type: "image/png".to_owned(),
        size: 1_024,
    }]);
})]
fn every_differing_field_is_reported_by_name(
    #[case] expected: DifferingField,
    #[case] expected_name: &'static str,
    #[case] incoming_is_retained: bool,
    #[case] mutate: fn(&mut Message),
) {
    let original = message("m_1", 1, 2);
    let mut changed = original.clone();
    mutate(&mut changed);
    assert_ne!(original, changed, "the case must actually change something");

    let out = reconcile(vec![original, changed]);

    assert_eq!(out.messages().len(), 1, "one identity, one message");
    assert_eq!(
        out.outcomes()[1],
        IngestOutcome::Conflicting {
            this_copy_was_retained: incoming_is_retained,
            fields: SmallVec::from_vec(vec![expected]),
        }
    );
    assert_eq!(
        expected.as_str(),
        expected_name,
        "the report names the field as the struct spells it"
    );
}

/// Every field is enumerated, and `as_str` covers all of them.
///
/// The list-length check is the compile-time guard's runtime shadow: if a
/// variant were added and never given a name, the match would not compile, and
/// this asserts the list is complete rather than assuming it.
#[test]
fn every_differing_field_has_the_name_of_the_field_it_stands_for() {
    let all = [
        DifferingField::Id,
        DifferingField::ChannelId,
        DifferingField::UserId,
        DifferingField::Content,
        DifferingField::Timestamp,
        DifferingField::EditedAt,
        DifferingField::Reactions,
        DifferingField::ThreadId,
        DifferingField::Attachments,
    ];
    let names: HashSet<&str> = all.iter().map(|field| field.as_str()).collect();

    assert_eq!(
        names,
        HashSet::from([
            "id",
            "channel_id",
            "user_id",
            "content",
            "timestamp",
            "edited_at",
            "reactions",
            "thread_id",
            "attachments",
        ])
    );
}

/// A batch of nothing is nothing.
#[test]
fn an_empty_batch_reconciles_to_nothing() {
    let out = reconcile(Vec::new());

    assert!(out.messages().is_empty());
    assert!(out.outcomes().is_empty());
    assert!(out.is_clean());
}

// ---------------------------------------------------------------------------
// The mandated property, exhaustively and then as a property
// ---------------------------------------------------------------------------

/// `AGENTS.md` §4.4, checked the hard way: **all 120** orderings of a batch that
/// was built to be awkward — two timestamp ties, an unacknowledged message, a
/// thread reply, an edited message — reconcile to the same ordered result.
///
/// A property test samples permutations; this enumerates them. It is the
/// difference between evidence and proof at this size, and the batch is
/// deliberately one where a wrong comparator *would* produce a difference.
#[test]
fn every_permutation_of_a_batch_reconciles_to_the_same_result() {
    let batch = awkward_batch();
    let orderings = permutations(&batch);
    assert_eq!(orderings.len(), 120, "5! orderings, all of them");

    let expected = reconcile(batch.clone());

    for ordering in &orderings {
        let out = reconcile(ordering.clone());
        assert_eq!(
            out.messages(),
            expected.messages(),
            "this ordering disagrees: {:?}",
            ordering.iter().map(|m| m.id.as_str()).collect::<Vec<_>>()
        );
        assert_eq!(
            out.outcomes().len(),
            ordering.len(),
            "one outcome per message handed in"
        );
    }
}

// `AGENTS.md` 4.4's property, as a property.
proptest! {
    /// For any permutation of a batch of messages, the ordered result is
    /// identical.
    #[test]
    fn permuting_a_batch_does_not_change_the_ordered_result(
        batch in distinct_batch(),
        keys in prop::collection::vec(any::<u8>(), 0..24),
    ) {
        let expected = reconcile(batch.clone());
        let permuted = reorder_by(&batch, &keys);

        let actual = reconcile(permuted);

        prop_assert_eq!(actual.messages(), expected.messages());
        prop_assert_eq!(actual.outcomes().len(), batch.len());
    }

    /// The deduplicated output's `client_msg_id` set equals the input's set:
    /// nothing is lost, and nothing is invented.
    #[test]
    fn reconciliation_preserves_exactly_the_input_client_msg_ids(
        batch in arbitrary_messages(),
    ) {
        let input: HashSet<Uuid> = batch.iter().map(|m| m.client_msg_id).collect();
        let expected_len = input.len();
        let out = reconcile(batch);

        let output: HashSet<Uuid> = out.messages().iter().map(|m| m.client_msg_id).collect();
        prop_assert_eq!(output, input);
        // And no id appears twice, which is the other half of "deduplicated".
        prop_assert_eq!(out.messages().len(), expected_len);
    }

    /// Arbitrary messages, including ones that disagree with each other, are
    /// reconciled without panicking. `AGENTS.md` §7.1 forbids a panic in a
    /// production path; this is where that is proved rather than promised.
    #[test]
    fn arbitrary_messages_reconcile_without_panicking(
        batch in arbitrary_messages(),
    ) {
        let out = reconcile(batch);
        prop_assert!(out.messages().len() <= out.outcomes().len());
    }

    /// The result really is ordered, and really is deduplicated — the two
    /// postconditions every caller of `reconcile` relies on and none of the
    /// other properties state directly.
    #[test]
    fn a_reconciled_batch_is_always_ascending_and_deduplicated(
        batch in arbitrary_messages(),
    ) {
        let out = reconcile(batch);

        for pair in out.messages().windows(2) {
            prop_assert!(
                compare(&pair[0], &pair[1]) == CmpOrdering::Less,
                "{:?} should sort before {:?}",
                pair[0],
                pair[1]
            );
            prop_assert_ne!(pair[0].client_msg_id, pair[1].client_msg_id);
        }
    }

    /// Permuting a batch of **disagreeing pairs** changes neither the surviving
    /// set nor which fields disagreed.
    ///
    /// This is the property that holds for a batch in which each identity
    /// appears **at most twice**, and the restriction is the point rather than
    /// a limitation to apologise for: with three or more copies of one
    /// identity, the second comparison is against whichever copy the first
    /// comparison retained, so the *sequence* of reported field lists genuinely
    /// does depend on arrival order. The field list for a pair is symmetric and
    /// therefore order-free, and the retained flag is the one documented
    /// exception. Everything else is a function of the set.
    #[test]
    fn permuting_a_batch_of_disagreeing_pairs_changes_neither_the_set_nor_the_report(
        batch in pairs_of_disagreeing_copies(),
        keys in prop::collection::vec(any::<u8>(), 0..24),
    ) {
        let expected = reconcile(batch.clone());
        let actual = reconcile(reorder_by(&batch, &keys));

        prop_assert_eq!(
            actual.messages().len(),
            expected.messages().len(),
            "the number of surviving messages is order-independent"
        );
        for (left, right) in actual.messages().iter().zip(expected.messages()) {
            prop_assert_eq!(left.client_msg_id, right.client_msg_id);
        }
        prop_assert_eq!(disagreements(&actual), disagreements(&expected));
    }
}

/// Every disagreement in a result, as `(retained_the_incoming_copy, fields)`.
///
/// The retained flag is deliberately **excluded** from the value: it is the one
/// part of the report that a permutation may legitimately change, so including
/// it here would turn the property into an assertion about the one documented
/// limit.
fn disagreements(ordered: &OrderedMessages) -> Vec<Vec<&'static str>> {
    let mut found: Vec<Vec<&'static str>> = ordered
        .outcomes()
        .iter()
        .filter_map(|outcome| match outcome {
            IngestOutcome::Conflicting { fields, .. } => {
                Some(fields.iter().map(|field| field.as_str()).collect())
            }
            _ => None,
        })
        .collect();
    // Sorted because `outcomes` is in *input* order, and a permutation moves
    // the entries. What is being compared is the set of disagreements, not the
    // order they were noticed in.
    found.sort();
    found
}

/// A batch of messages with **distinct** `client_msg_id`s, packed with the
/// features that break naive comparators.
///
/// Timestamps come from a three-value range, so ties are the common case rather
/// than a rare one; `id` is empty about a third of the time, so the
/// `client_msg_id` tiebreaker is exercised; `edited_at` is sometimes set, so a
/// key that leaked it would be caught.
fn distinct_batch() -> impl Strategy<Value = Vec<Message>> {
    prop::collection::vec(
        (0i64..3, any::<bool>(), any::<bool>(), any::<bool>()),
        0..16,
    )
    .prop_map(|rows| {
        rows.into_iter()
            .enumerate()
            .map(|(index, (second, unacked, edited, reply))| {
                let client = u128::try_from(index).expect("a batch of 16 fits in u128") + 1;
                let id = if unacked {
                    String::new()
                } else {
                    format!("m_{index:03}")
                };
                let mut m = message(&id, client, second);
                if edited {
                    m.edited_at = Some(at(1_000 + second));
                }
                if reply {
                    m.thread_id = Some("m_000".to_owned());
                }
                m
            })
            .collect()
    })
}

/// Arbitrary messages, including duplicates and disagreements between copies of
/// one identity — which is what makes the no-loss and no-panic properties bite.
fn arbitrary_messages() -> impl Strategy<Value = Vec<Message>> {
    prop::collection::vec(
        (any::<u128>(), arbitrary_text(), 0i64..4, any::<u128>()),
        0..12,
    )
    .prop_map(|rows| {
        rows.into_iter()
            .map(|(client, content, second, id_seed)| {
                let mut m = message(&format!("m_{id_seed:08}"), client % 6, second);
                m.content = content;
                m.timestamp = at(second);
                // A second, usually different, id for the same client_msg_id,
                // so the generator produces genuine conflicts.
                m.id = format!("m_{:08}", (id_seed + 1) % 6);
                if id_seed % 3 == 0 {
                    m.edited_at = Some(at(2_000 + second));
                }
                m
            })
            .collect()
    })
}

/// Arbitrary text, including the empty string and non-alphabetic content.
///
/// `any::<String>()` is enough and is not a shortcut: a `Message`'s content is
/// never parsed here, so the interesting property is that *any* bytes are
/// accepted, which is exactly what this strategy generates.
fn arbitrary_text() -> impl Strategy<Value = String> {
    prop_oneof![
        any::<String>(),
        proptest::sample::select(vec![
            String::new(),
            " \u{1f44d}\n\u{0}".to_owned(),
            "x".repeat(4_096),
        ]),
    ]
}

/// One generated copy of a message: everything that can differ between two
/// copies of one identity.
type Copy = (u128, String, i64, bool);

/// A batch in which each `client_msg_id` appears **at most twice**.
///
/// One identity per row, and at most two copies within a row — that is the
/// condition the disagreement-permutation property is stated over, so the
/// generator has to guarantee it rather than merely make it likely. An earlier
/// version drew the identity from a small random space, which let two rows
/// produce four copies of one identity and made the property false; proptest
/// found it in three cases, which is what property tests are for.
///
/// The two copies of a row are generated independently, so they sometimes come
/// out identical — the collapse path, and worth generating — and sometimes
/// disagree about anything from one field to all nine.
fn pairs_of_disagreeing_copies() -> impl Strategy<Value = Vec<Message>> {
    prop::collection::vec(
        (
            (any::<u128>(), arbitrary_text(), 0i64..4, any::<bool>()),
            prop::option::of((any::<u128>(), arbitrary_text(), 0i64..4, any::<bool>())),
        ),
        0..8,
    )
    .prop_map(|rows| {
        rows.into_iter()
            .enumerate()
            .flat_map(|(index, (first, second))| {
                let client = u128::try_from(index).expect("eight rows fit in u128");
                let mut copies = vec![copy_of(client, first)];
                if let Some(second) = second {
                    copies.push(copy_of(client, second));
                }
                copies
            })
            .collect()
    })
}

/// Builds one copy of a message for a given identity.
fn copy_of(client: u128, (id_seed, content, second, edited): Copy) -> Message {
    let mut m = message(&format!("m_{id_seed:04}"), client, second);
    m.content = content;
    if edited {
        m.edited_at = Some(at(2_000 + second));
    }
    m
}

/// Reorders `items` into a permutation of **all** of them, using one key per
/// item and falling back to the original order when no keys are supplied.
///
/// Two properties matter here. The keys are **cycled** rather than zipped, so a
/// short key vector still reorders the whole batch — a permutation that quietly
/// dropped items would make the no-loss property test something weaker than it
/// looks. And the sort is on `(key, index)`, not on the key alone, so the
/// result is a function of the keys rather than of the sort's internal
/// behaviour, which is what lets proptest's shrinking converge.
fn reorder_by<T: Clone>(items: &[T], keys: &[u8]) -> Vec<T> {
    if keys.is_empty() {
        return items.to_vec();
    }
    let mut order: Vec<(u8, usize)> = (0..items.len())
        .map(|index| (keys[index % keys.len()], index))
        .collect();
    order.sort_unstable();
    order
        .into_iter()
        .filter_map(|(_, index)| items.get(index).cloned())
        .collect()
}

// ---------------------------------------------------------------------------
// Gap detection
// ---------------------------------------------------------------------------

/// No cursor: the set is a whole history, and continuity is not a question
/// about it.
#[test]
fn a_channel_with_no_cursor_has_never_been_synced() {
    let out = reconcile(vec![message("m_1", 1, 5)]);

    let status = assess_sync(&out, SyncExpectation::never_synced());

    assert_eq!(status, SyncStatus::NeverSynced);
    assert!(!status.is_healthy());
    assert_eq!(SyncExpectation::default(), SyncExpectation::never_synced());
}

/// A cursor and no watermark is `PLAN.md` §6's protocol as it stands, and the
/// answer is "cannot tell" — not "no gaps".
#[test]
fn a_cursor_without_a_watermark_is_unverifiable_and_not_healthy() {
    let out = reconcile(vec![message("m_1", 1, 5)]);
    let expectation = SyncExpectation {
        cursor: Some(at(0)),
        watermark: None,
    };

    let status = assess_sync(&out, expectation);

    assert_eq!(status, SyncStatus::Unverifiable { cursor: at(0) });
    assert!(
        !status.is_healthy(),
        "a client that accepted 'cannot tell' as 'no gaps' would be asserting \
         continuity it has no evidence for"
    );
}

/// The set reaches the watermark, so no gap is detectable.
#[test]
fn a_set_reaching_the_watermark_is_complete() {
    let out = reconcile(vec![message("m_1", 1, 5), message("m_2", 2, 9)]);
    let expectation = SyncExpectation {
        cursor: Some(at(0)),
        watermark: Some(at(9)),
    };

    let status = assess_sync(&out, expectation);

    assert_eq!(
        status,
        SyncStatus::Complete {
            newest: Some(at(9))
        }
    );
    assert!(status.is_healthy());
}

/// The set stops short of what the server said it sent. The interval is named
/// and nothing is invented.
#[test]
fn a_set_falling_short_of_the_watermark_names_the_interval_as_a_gap() {
    let out = reconcile(vec![message("m_1", 1, 5)]);
    let expectation = SyncExpectation {
        cursor: Some(at(0)),
        watermark: Some(at(9)),
    };

    let status = assess_sync(&out, expectation);

    assert_eq!(
        status,
        SyncStatus::Gap {
            after: Some(at(5)),
            before: at(9),
        }
    );
    assert!(!status.is_healthy());
}

/// Nothing at all, and the server said there was something: the whole interval
/// is missing.
#[test]
fn an_empty_set_with_a_watermark_beyond_the_cursor_is_a_gap() {
    let out = reconcile(Vec::new());
    let expectation = SyncExpectation {
        cursor: Some(at(0)),
        watermark: Some(at(9)),
    };

    assert_eq!(
        assess_sync(&out, expectation),
        SyncStatus::Gap {
            after: None,
            before: at(9)
        },
        "an empty response is only correct when the channel is quiet"
    );
}

/// Nothing at all, and the watermark is behind the cursor: a quiet channel is
/// an empty suffix, not a gap.
///
/// The case that stops the check above from crying wolf on every idle channel.
#[test]
fn an_empty_set_with_a_watermark_behind_the_cursor_is_complete() {
    let out = reconcile(Vec::new());
    let expectation = SyncExpectation {
        cursor: Some(at(9)),
        watermark: Some(at(0)),
    };

    let status = assess_sync(&out, expectation);

    assert_eq!(status, SyncStatus::Complete { newest: None });
    assert!(status.is_healthy());
}

/// A message at the cursor itself is outside the range, because `after` is
/// exclusive. Half-open boundaries are where gap detectors usually lie.
#[rstest]
#[case(-1, SyncStatus::Outside { oldest: at(-1), cursor: at(0) })]
#[case(0, SyncStatus::Outside { oldest: at(0), cursor: at(0) })]
fn a_message_at_or_before_the_cursor_is_reported_as_outside_not_as_a_gap(
    #[case] offset: i64,
    #[case] expected: SyncStatus,
) {
    let out = reconcile(vec![message("m_1", 1, offset)]);
    let expectation = SyncExpectation {
        cursor: Some(at(0)),
        watermark: Some(at(9)),
    };

    let status = assess_sync(&out, expectation);

    assert_eq!(
        status, expected,
        "a response that ignored `after` is not a gap, and is not a pass"
    );
    assert!(!status.is_healthy());
}

/// `Outside` takes precedence over the watermark checks, and says so.
///
/// A response that ignored `after` is not the response the watermark statements
/// reason about, so the more fundamental inconsistency is reported first.
#[test]
fn outside_takes_precedence_over_the_watermark_checks() {
    let out = reconcile(vec![message("m_1", 1, -100)]);
    let expectation = SyncExpectation {
        cursor: Some(at(0)),
        // A watermark that would otherwise produce a `Gap`.
        watermark: Some(at(9)),
    };

    assert!(matches!(
        assess_sync(&out, expectation),
        SyncStatus::Outside { .. }
    ));
}

/// A gap is a report. It does not add a row, renumber anything, or move the
/// cursor.
#[test]
fn a_gap_names_an_interval_and_fills_nothing() {
    let before = reconcile(vec![message("m_1", 1, 5)]);
    let expectation = SyncExpectation {
        cursor: Some(at(0)),
        watermark: Some(at(9)),
    };

    let status = assess_sync(&before, expectation);

    assert_eq!(
        status,
        SyncStatus::Gap {
            after: Some(at(5)),
            before: at(9)
        }
    );
    assert_eq!(
        before.messages().len(),
        1,
        "assessing a set must not mutate it -- no placeholder row, no \
         synthesised message, and the gap is not filled"
    );
    // The re-requestable interval is exactly what the caller needs, and it is
    // the server's own bounds rather than an estimate.
    let SyncStatus::Gap { after, before } = status else {
        panic!("expected a gap, got {status:?}");
    };
    assert_eq!(after, Some(at(5)));
    assert_eq!(before, at(9));
}

/// Only `Complete` is healthy. Every other status, including "cannot tell", is
/// not.
#[rstest]
#[case(SyncStatus::NeverSynced, false)]
#[case(SyncStatus::Unverifiable { cursor: at(0) }, false)]
#[case(SyncStatus::Complete { newest: None }, true)]
#[case(SyncStatus::Complete { newest: Some(at(0)) }, true)]
#[case(SyncStatus::Gap { after: None, before: at(0) }, false)]
#[case(SyncStatus::Gap { after: Some(at(0)), before: at(9) }, false)]
#[case(SyncStatus::Outside { oldest: at(0), cursor: at(9) }, false)]
fn is_healthy_is_false_for_every_status_but_complete(
    #[case] status: SyncStatus,
    #[case] expected: bool,
) {
    assert_eq!(status.is_healthy(), expected);
}

/// `SyncExpectation` carries the two `None`s as named states, not as positions.
#[test]
fn the_two_empty_expectations_mean_opposite_things() {
    let never = SyncExpectation::never_synced();
    let unverifiable = SyncExpectation {
        cursor: Some(at(0)),
        watermark: None,
    };

    assert_ne!(
        never, unverifiable,
        "the first means nothing is wrong, the second means continuity cannot \
         be established"
    );
}
