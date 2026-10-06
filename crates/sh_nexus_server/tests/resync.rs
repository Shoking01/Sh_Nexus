//! `AGENTS.md` §7.4's *"resume from the last known `last_message_at` cursor — never
//! rely solely on 'live' delivery during a gap"*, and §8.1's Reconnect Flow's
//! *"no duplicates, no gaps"*, against a real socket.
//!
//! # What is new here
//!
//! Until this file existed a `resync` decoded, was logged at `warn!`, and was
//! dropped — so a reconnecting client asked to catch up, recovered nothing, and
//! **was not told so**. `sh_nexus_server/src/ws.rs`'s module docs call that the one
//! unimplemented frame with teeth, and it is why the client half of this work
//! could not land first.
//!
//! | Test | What it proves |
//! |---|---|
//! | [`a_resync_from_a_cursor_replays_exactly_the_newer_messages_in_acceptance_order`] | the ordinary case, through the real accept path |
//! | [`the_boundary_millisecond_is_not_skipped_because_the_bound_is_inclusive`] | `>=` and not `>`, at a millisecond two messages share |
//! | [`an_unknown_channel_is_refused_exactly_as_a_send_is`] | the refusal `message.send` gives, with the same code |
//! | [`a_non_member_is_refused_the_channels_entire_history`] | reading a channel is a membership question, not a connection one |
//! | [`a_full_batch_means_there_is_more_and_a_short_one_means_caught_up`] | the bound is honoured and a truncated batch is distinguishable |
//! | [`an_empty_range_returns_nothing_and_is_not_an_error`] | caught up is an answer, not a failure |
//! | [`a_catch_up_reaches_only_the_connection_that_asked_for_it`] | it is per-connection, so the echo question does not arise |
//!
//! # Why some rows are planted rather than sent
//!
//! **Two tests cannot be written through the socket at all, and the reason is the
//! clock.** `Store::accept_message` stamps `accepted_at_unix_ms` from
//! `time::now_unix_millis()`, so two sends *might* share a millisecond — and a test
//! that depends on "might" is a test that passes on a fast machine and fails on a
//! slow one. The boundary case and the full-batch case are both about millisecond
//! arithmetic and batch arithmetic, so both plant rows directly, with ids packed the
//! way [`Store::accept_message`] packs them.
//!
//! That packing is not incidental to the fixture: `mint_message_id` writes
//! `(accepted_at_unix_ms << 64) | sequence`, so a row's id sorts by its clock and
//! then by acceptance order **within** the millisecond. Planting rows with ids built
//! the same way is what makes `ORDER BY accepted_at_unix_ms, id` produce the order a
//! real accept path would have produced — so these tests assert the ordering
//! property rather than a fixture's invention.
//!
//! Everything else goes through the socket, because everything else is about the
//! frame, the refusal and the socket's survival, and a mock would answer none of
//! those three.

mod support;

use chrono::{DateTime, Utc};
use rusqlite::Connection;
use sh_nexus_server::db::DEFAULT_CHANNEL_ID;
use sh_nexus_wire::frame::{ClientEnvelope, ClientFrame, ServerEnvelope, ServerFrame};
use sh_nexus_wire::WireMessage;
use support::{TestClient, TestServer, SILENCE_BUDGET};

/// How many messages the planted rows are, for the batch-bound test.
///
/// **Deliberately more than one batch and not a round number of them.** 300 is
/// `RESYNC_BATCH_LIMIT` (256) plus 44, so the first batch is exactly full and the
/// second is short — which is the only pair of lengths that exercises both halves of
/// "ask again only while a batch comes back full".
const PLANTED: usize = 300;

/// One instant, in the storage clock's own units.
///
/// `Store::accept_message` reads `time::now_unix_millis`, so the reader's bound is
/// an `i64` of milliseconds and a test that plants rows has to speak that dialect.
const BASE_UNIX_MS: i64 = 1_789_000_000_000;

/// A `DateTime` for a millisecond offset from [`BASE_UNIX_MS`].
///
/// # Panics
///
/// If the offset is out of range, which no call site here can produce.
fn at(offset_ms: i64) -> DateTime<Utc> {
    DateTime::from_timestamp_millis(BASE_UNIX_MS + offset_ms)
        .expect("a fixed base plus a small millisecond offset is in range")
}

/// A server-assigned id, packed exactly the way `mint_message_id` packs it.
///
/// **This is the fixture's load-bearing detail.** The production id is
/// `Uuid::from_u128((accepted_at_unix_ms << 64) | sequence)`, and that is why
/// `ORDER BY accepted_at_unix_ms, id` agrees with acceptance order inside one
/// millisecond. A planted row with an arbitrary id would not have that property, so
/// a test asserting replay order would be asserting the fixture rather than the
/// ordering guarantee.
fn planted_id(unix_ms: i64, sequence: u64) -> String {
    // `unsigned_abs` mirrors `mint_message_id`, which uses it for the same reason:
    // a total function that is wrong for an impossible negative clock beats one that
    // panics on it.
    uuid::Uuid::from_u128((u128::from(unix_ms.unsigned_abs()) << 64) | u128::from(sequence))
        .hyphenated()
        .to_string()
}

/// Writes one row straight into the database file, at a chosen instant.
///
/// WAL is what makes this possible while the server holds the file open, and
/// `support`'s own readers rely on exactly that.
///
/// # Panics
///
/// If the file cannot be opened or the insert fails. Both are fixture faults: the
/// foreign keys are satisfied by the seeded channel and the bootstrapped
/// administrator, so a failure means the test asked for something impossible.
fn plant(
    database: &std::path::Path,
    user_id: &str,
    channel_id: &str,
    unix_ms: i64,
    sequence: u64,
    content: &str,
) -> String {
    let id = planted_id(unix_ms, sequence);
    let connection = Connection::open(database).expect("the database file the server created");
    connection
        .execute(
            "INSERT INTO messages
                 (id, client_msg_id, channel_id, user_id, content, accepted_at_unix_ms)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
            rusqlite::params![
                &id,
                format!("planted-{unix_ms}-{sequence}"),
                channel_id,
                user_id,
                content,
                unix_ms
            ],
        )
        .expect("a planted row to satisfy the schema");
    id
}

/// Plants `count` rows, one per millisecond from the base, and returns their ids.
fn plant_run(
    database: &std::path::Path,
    user_id: &str,
    channel_id: &str,
    count: usize,
) -> Vec<String> {
    (0..count)
        .map(|index| {
            plant(
                database,
                user_id,
                channel_id,
                BASE_UNIX_MS + i64::try_from(index).expect("a small index fits an i64"),
                0,
                "planted",
            )
        })
        .collect()
}

/// A `resync` envelope asking for everything at or after `after`.
fn resync(channel_id: &str, after: DateTime<Utc>) -> ClientEnvelope {
    ClientEnvelope::new(
        "3f6c1d90-0000-4000-8000-0000000000aa",
        ClientFrame::Resync {
            channel_id: channel_id.to_owned(),
            after,
        },
    )
}

/// The JSON text for an envelope.
///
/// # Panics
///
/// If encoding fails, which `ClientEnvelope::encode` documents it cannot for any
/// value its types can hold. `AGENTS.md` §2.1 permits a panic in a test.
fn json(envelope: &ClientEnvelope) -> String {
    envelope
        .encode()
        .expect("a sh_nexus_wire client envelope always encodes")
}

/// The next replayed message on `client`.
///
/// # Panics
///
/// If the next frame is not a `message.new`. A `message.error` reaching this helper
/// means the server refused a catch-up the test expected to be answered, and the
/// panic says so with the frame.
async fn next_replayed(client: &mut TestClient, expected: &str) -> WireMessage {
    let text = client.expect_text(expected).await;
    let envelope = ServerEnvelope::decode(&text)
        .unwrap_or_else(|error| panic!("expected {expected}, got {text:?}: {error}"));
    match envelope.frame {
        ServerFrame::MessageNew { message } => message,
        other => panic!("expected a replayed message for {expected}, got {other:?}"),
    }
}

/// Takes the next frame, naming the expectation in the panic.
async fn next_of_kind(client: &mut TestClient, expected: &str) -> ServerEnvelope {
    let text = client.expect_text(expected).await;
    ServerEnvelope::decode(&text)
        .unwrap_or_else(|error| panic!("expected {expected}, got {text:?}: {error}"))
}

/// The `code` of the next refusal on `client`.
///
/// # Panics
///
/// If the next frame is not a `message.error`.
async fn next_refusal_code(client: &mut TestClient, expected: &str) -> String {
    let envelope = next_of_kind(client, expected).await;
    match envelope.frame {
        ServerFrame::MessageError { code, .. } => code,
        other => panic!("expected a refusal for {expected}, got {other:?}"),
    }
}

/// A `message.send` the client's own boundary would accept.
fn send(client_msg_id: &str, channel_id: &str, content: &str) -> ClientEnvelope {
    ClientEnvelope::new(
        client_msg_id,
        ClientFrame::MessageSend {
            channel_id: channel_id.to_owned(),
            content: content.to_owned(),
        },
    )
}

// ---------------------------------------------------------------------------
// 1. The ordinary case
// ---------------------------------------------------------------------------

/// A resync from a cursor replays exactly the newer messages, in acceptance order.
///
/// **Sent, not planted, on purpose.** This is the flow a real client performs and
/// it is the one that has to work end to end: the rows were accepted through
/// `Store::accept_message`, broadcast to nobody (the sender does not get its own
/// echo), and then read back by the *same* connection through a catch-up. If the
/// per-connection write went through the hub, the sender would have seen each
/// message twice and this assertion on the ids would show it.
#[tokio::test]
async fn a_resync_from_a_cursor_replays_exactly_the_newer_messages_in_acceptance_order() {
    let server = TestServer::start().await;
    let mut alice = server.connect().await;

    let identities = [
        "5c8a1b2d-3e4f-4a5b-8c9d-0e1f2a3b4c5d",
        "7d9b2c3e-4f5a-4b6c-9d0e-1f2a3b4c5d6e",
        "9a0c3d4e-5f6b-4c7d-8e1f-2a3b4c5d6e7f",
    ];
    let mut stored_ids = Vec::new();
    for identity in identities {
        alice
            .send_text(&json(&send(identity, DEFAULT_CHANNEL_ID, "hello team")))
            .await
            .expect("the send to reach the server");
        let envelope = next_of_kind(&mut alice, "the acknowledgement").await;
        match envelope.frame {
            ServerFrame::MessageAck { message, .. } => stored_ids.push(message.id),
            other => panic!("expected a message.ack, got {other:?}"),
        }
    }

    alice
        .send_text(&json(&resync(DEFAULT_CHANNEL_ID, at(-1_000))))
        .await
        .expect("the resync to reach the server");

    let mut replayed = Vec::new();
    for expected in ["the first replay", "the second replay", "the third replay"] {
        replayed.push(next_replayed(&mut alice, expected).await.id);
    }
    assert_eq!(
        replayed, stored_ids,
        "the catch-up returns exactly what the client missed, in the order the server \
         accepted it -- ORDER BY accepted_at_unix_ms, id is a total order because \
         mint_message_id packs the clock into the id's high bits"
    );

    // And nothing beyond them: a channel with three messages and a cursor before all
    // three is caught up once they are gone.
    assert_eq!(
        alice.next_text_within(SILENCE_BUDGET).await,
        None,
        "three messages were stored and three were replayed; a fourth would mean the \
         read ignored its limit or its channel"
    );

    drop(alice);
    server.shutdown().await;
}

// ---------------------------------------------------------------------------
// 2. The boundary millisecond
// ---------------------------------------------------------------------------

/// The boundary millisecond is not skipped, because the bound is inclusive.
///
/// **This is the test the wire comment used to be wrong about.**
/// `sh_nexus_wire/src/frame.rs` documented `Resync::after` as an exclusive lower
/// bound and justified it with no-duplicates. `accepted_at_unix_ms` is millisecond
/// resolution, so a strict bound read from a cursor pointing at the first of two
/// messages accepted in the same millisecond skips the second **forever** — the
/// cursor has moved past it and nothing will ask again. Silent, unrecoverable, and
/// exactly what `AGENTS.md`'s second priority forbids.
///
/// Three rows share one millisecond here and a fourth is a millisecond later, and
/// the cursor names the shared instant. An inclusive bound returns all four in
/// acceptance order; an exclusive one returns only the fourth.
#[tokio::test]
async fn the_boundary_millisecond_is_not_skipped_because_the_bound_is_inclusive() {
    let server = TestServer::start().await;
    let admin = server.admin_user_id().to_owned();

    let first = plant(
        server.database(),
        &admin,
        DEFAULT_CHANNEL_ID,
        BASE_UNIX_MS,
        0,
        "first",
    );
    let second = plant(
        server.database(),
        &admin,
        DEFAULT_CHANNEL_ID,
        BASE_UNIX_MS,
        1,
        "second",
    );
    let third = plant(
        server.database(),
        &admin,
        DEFAULT_CHANNEL_ID,
        BASE_UNIX_MS,
        2,
        "third",
    );
    let later = plant(
        server.database(),
        &admin,
        DEFAULT_CHANNEL_ID,
        BASE_UNIX_MS + 1,
        0,
        "later",
    );

    let mut alice = server.connect().await;
    alice
        .send_text(&json(&resync(DEFAULT_CHANNEL_ID, at(0))))
        .await
        .expect("the resync to reach the server");

    let mut replayed = Vec::new();
    for expected in [
        "the first row of the shared millisecond",
        "the second row of the shared millisecond",
        "the third row of the shared millisecond",
        "the row after the shared millisecond",
    ] {
        replayed.push(next_replayed(&mut alice, expected).await.id);
    }

    assert_eq!(
        replayed,
        vec![first, second, third, later],
        "a cursor naming an instant gets that instant back. `>` would return only \
         `later`, and the three rows it dropped would be gone for good: the cursor \
         has already moved past them and nothing asks again."
    );

    drop(alice);
    server.shutdown().await;
}

// ---------------------------------------------------------------------------
// 3. Refusals
// ---------------------------------------------------------------------------

/// An unknown channel is refused the way a send naming one is refused.
///
/// **The same code, not a second vocabulary.** `unknown_channel` is already the
/// server's answer to "you named a channel this instance does not have", and a
/// client that had to learn a second spelling for the same fact would be a protocol
/// with two rules where there is one. The socket also survives, which is the other
/// half: a refused catch-up is one bad request, not a broken connection.
#[tokio::test]
async fn an_unknown_channel_is_refused_exactly_as_a_send_is() {
    let server = TestServer::start().await;
    let mut alice = server.connect().await;

    alice
        .send_text(&json(&resync("c_this_instance_does_not_have", at(0))))
        .await
        .expect("the resync to reach the server");

    assert_eq!(
        next_refusal_code(&mut alice, "the refusal of an unknown channel").await,
        sh_nexus_server::message::UNKNOWN_CHANNEL,
        "a catch-up that named nothing gets the code a send naming nothing gets"
    );
    assert_eq!(
        alice.next_text_within(SILENCE_BUDGET).await,
        None,
        "a refusal is one frame and no replay: answering with an empty catch-up as \
         well would tell the client it was caught up with a channel that does not \
         exist, which is the invisible failure this whole path exists to remove"
    );

    // The socket is still usable, which is what separates this from a 401.
    alice
        .send_text(&json(&send(
            "0f1e2d3c-4b5a-4968-8778-665544332211",
            DEFAULT_CHANNEL_ID,
            "still here",
        )))
        .await
        .expect("the send to reach a socket that should still be open");
    let envelope = next_of_kind(&mut alice, "the acknowledgement after a refusal").await;
    assert!(
        matches!(envelope.frame, ServerFrame::MessageAck { .. }),
        "a refused catch-up must not cost the connection its authorization, which was \
         already decided at the handshake"
    );

    drop(alice);
    server.shutdown().await;
}

/// A non-member is refused the channel's entire history.
///
/// **This is the step a catch-up could most easily have skipped, and skipping it
/// would be a read-access hole.** A replay hands over everything a channel has said
/// recently, so a client that could ask for a channel it is not in would be able to
/// read a private conversation by asking the right question — the precise thing
/// ADR-010 decides against when it refuses open self-registration.
///
/// **It is refused with `not_a_member` rather than `unknown_channel`,** because the
/// two are different facts and a user who needs to ask an administrator for access
/// must be able to tell which one they hit. `Store::channel_access` decides
/// existence before membership, so the ordering is the one `message.rs` already
/// uses.
#[tokio::test]
async fn a_non_member_is_refused_the_channels_entire_history() {
    let server = TestServer::start().await;
    // Rows the outsider must not be able to read, in a channel they are not in.
    plant_run(
        server.database(),
        server.admin_user_id(),
        DEFAULT_CHANNEL_ID,
        3,
    );

    let outsider = server.create_account("outsider", false);
    let token = server.session_for(&outsider);
    let mut stranger = TestClient::connect(server.address(), Some(&token))
        .await
        .expect("the outsider's authenticated upgrade");

    stranger
        .send_text(&json(&resync(DEFAULT_CHANNEL_ID, at(-1))))
        .await
        .expect("the resync to reach the server");

    assert_eq!(
        next_refusal_code(&mut stranger, "the refusal of a non-member's catch-up").await,
        sh_nexus_server::message::NOT_A_MEMBER,
        "membership is a question about the channel, and it is asked before the \
         history is read rather than after"
    );
    assert_eq!(
        stranger.next_text_within(SILENCE_BUDGET).await,
        None,
        "not one message of a channel this account is not in may reach it"
    );

    drop(stranger);
    server.shutdown().await;
}

/// A blank channel is refused before any storage is touched.
///
/// **The same rule `message.rs` checks, for the same reason:** a refused request
/// leaves nothing behind, cannot move a channel's resume cursor, and cannot be
/// half-applied.
#[tokio::test]
async fn a_blank_channel_is_refused_before_any_storage_is_touched() {
    let server = TestServer::start().await;
    let mut alice = server.connect().await;

    alice
        .send_text(&json(&resync("   ", at(0))))
        .await
        .expect("the resync to reach the server");

    assert_eq!(
        next_refusal_code(&mut alice, "the refusal of a blank channel").await,
        sh_nexus_server::message::BLANK_CHANNEL_ID,
        "a whitespace-only channel id names no channel, and the refusal says which \
         field was wrong rather than reporting a server fault"
    );

    drop(alice);
    server.shutdown().await;
}

// ---------------------------------------------------------------------------
// 4. The bound
// ---------------------------------------------------------------------------

/// A full batch means there is more, and a short one means caught up.
///
/// **This is the property the client's counting rule rests on.** With no
/// `resync.result` carrying a watermark — `PLAN.md` §6 describes one and
/// `core/ordering.rs` §4 says its absence is why every resync reports
/// `Unverifiable` — the client decides "is there more" by asking whether the batch
/// came back full. If a full batch were indistinguishable from a short one, the
/// client would either stop early and lose messages or never stop.
///
/// **The second batch is [`RESYNC_BATCH_LIMIT`] + 44, not 44.** The bound is
/// inclusive, so asking again from the last row of the first batch returns *that row
/// again* plus the 44 that follow. **That overlap is the price of `>=`, and it is
/// bounded: one row per reconnect, which the client collapses against what it
/// already holds.** An exclusive bound would return 44 here and would have lost the
/// last row of the first batch in every case where two messages shared a
/// millisecond.
#[tokio::test]
async fn a_full_batch_means_there_is_more_and_a_short_one_means_caught_up() {
    let server = TestServer::start().await;
    let ids = plant_run(
        server.database(),
        server.admin_user_id(),
        DEFAULT_CHANNEL_ID,
        PLANTED,
    );
    let batch = sh_nexus_server::ws::RESYNC_BATCH_LIMIT;
    assert!(
        PLANTED > batch && PLANTED < 2 * batch,
        "the fixture needs one full batch and one short one, so `PLANTED` must sit \
         between one and two bounds; it is {PLANTED} against a bound of {batch}"
    );

    let mut alice = server.connect().await;
    alice
        .send_text(&json(&resync(DEFAULT_CHANNEL_ID, at(-1))))
        .await
        .expect("the first resync to reach the server");

    let mut first_batch = Vec::new();
    for index in 0..batch {
        first_batch.push(
            next_replayed(&mut alice, "a row of the first batch")
                .await
                .id,
        );
        // The message is the assertion, not the count, so name the index.
        let _ = index;
    }
    assert_eq!(
        first_batch,
        ids[..batch],
        "the first batch is exactly the bound, in acceptance order, and stops there"
    );
    assert_eq!(
        alice.next_text_within(SILENCE_BUDGET).await,
        None,
        "a full batch is {batch} rows and not {PLANTED}: the reader honours its limit, \
         which is what makes the limit the only thing standing between a catch-up and \
         an unbounded write loop on a socket nobody may be reading"
    );

    // Ask again from the *last row of the first batch*, which is what the client's
    // advancement rule does: the batch maximum, duplicates included.
    let boundary = first_batch.last().expect("a full batch is not empty");
    let boundary_instant = planted_instant(boundary);
    alice
        .send_text(&json(&resync(DEFAULT_CHANNEL_ID, boundary_instant)))
        .await
        .expect("the second resync to reach the server");

    let mut second_batch = Vec::new();
    for expected in ["the overlap row", "a row after the overlap"] {
        second_batch.push(next_replayed(&mut alice, expected).await.id);
    }
    while second_batch.len() < PLANTED - batch + 1 {
        second_batch.push(
            next_replayed(&mut alice, "a row of the second batch")
                .await
                .id,
        );
    }

    assert_eq!(
        second_batch,
        ids[batch - 1..],
        "the second batch overlaps by exactly one row -- the boundary -- and then \
         carries every remaining message. Inclusive is what makes that row reachable \
         at all, and the client's dedup on client_msg_id is what makes it harmless."
    );
    assert_eq!(
        alice.next_text_within(SILENCE_BUDGET).await,
        None,
        "a short batch is caught up: the client stops asking here, and this is the \
         property that keeps the loop finite"
    );

    drop(alice);
    server.shutdown().await;
}

/// The instant a planted id encodes, recovered from the id itself.
///
/// **Recovering rather than keeping a parallel array**, because the client under
/// test does exactly that: it holds a row, reads its `timestamp`, and asks again
/// from it. A fixture that handed the second request a pre-computed constant would
/// not be testing the rule the client follows.
fn planted_instant(id: &str) -> DateTime<Utc> {
    let hex: String = id.chars().filter(|character| *character != '-').collect();
    let high = u128::from_str_radix(&hex[..16], 16).expect("a planted id is hexadecimal");
    at(i64::try_from(high).expect("the packed clock fits an i64") - BASE_UNIX_MS)
}

// ---------------------------------------------------------------------------
// 5. Caught up, and addressed to one connection
// ---------------------------------------------------------------------------

/// An empty range returns nothing and is not an error.
///
/// **A caught-up channel is an ordinary answer to the question that was asked**, and
/// the difference between "nothing was newer" and "something went wrong" is the
/// difference between a client that stops asking and a client that retries forever.
/// The socket surviving is asserted too, because an empty answer is the *only*
/// answer this path can give that carries no frame at all.
#[tokio::test]
async fn an_empty_range_returns_nothing_and_is_not_an_error() {
    let server = TestServer::start().await;
    plant_run(
        server.database(),
        server.admin_user_id(),
        DEFAULT_CHANNEL_ID,
        2,
    );
    let mut alice = server.connect().await;

    alice
        .send_text(&json(&resync(DEFAULT_CHANNEL_ID, at(60_000))))
        .await
        .expect("the resync to reach the server");
    assert_eq!(
        alice.next_text_within(SILENCE_BUDGET).await,
        None,
        "a cursor a minute past the newest row has nothing to replay, and silence is \
         the whole answer"
    );

    // Still open: silence is not a close.
    alice
        .send_text(&json(&send(
            "1a2b3c4d-5e6f-4071-8283-9495a6a7c8d9",
            DEFAULT_CHANNEL_ID,
            "after an empty catch-up",
        )))
        .await
        .expect("the send to reach a socket that should still be open");
    let envelope = next_of_kind(&mut alice, "the acknowledgement after an empty catch-up").await;
    assert!(
        matches!(envelope.frame, ServerFrame::MessageAck { .. }),
        "an empty catch-up cost the connection nothing"
    );

    drop(alice);
    server.shutdown().await;
}

/// A catch-up reaches only the connection that asked for it.
///
/// **This is the decision `hub.rs` left open, and the test that settles it.** A
/// replay is addressed to one connection, so it is read from the store and written
/// to that socket — never fanned out. If it went through [`Hub::broadcast`] instead,
/// every other client on the instance would receive a conversation it was not part
/// of, each of which would then deduplicate it against what it already held.
///
/// [`Hub::broadcast`]: sh_nexus_server::Hub::broadcast
#[tokio::test]
async fn a_catch_up_reaches_only_the_connection_that_asked_for_it() {
    let server = TestServer::start().await;
    plant_run(
        server.database(),
        server.admin_user_id(),
        DEFAULT_CHANNEL_ID,
        4,
    );

    let mut asker = server.connect().await;
    let mut bystander = server.connect().await;

    asker
        .send_text(&json(&resync(DEFAULT_CHANNEL_ID, at(-1))))
        .await
        .expect("the resync to reach the server");

    // The asker gets all four. The positive half first, so the absence below is a
    // fact about the fan-out rather than about a catch-up that returned nothing.
    let mut replayed = Vec::new();
    for expected in ["replay one", "replay two", "replay three", "replay four"] {
        replayed.push(next_replayed(&mut asker, expected).await.id);
    }
    assert_eq!(replayed.len(), 4, "the asker received its catch-up");

    assert_eq!(
        bystander.next_text_within(SILENCE_BUDGET).await,
        None,
        "a catch-up is addressed to the connection that asked. Broadcasting it would \
         deliver another conversation to every client on the instance, and would \
         reintroduce the echo question `hub::Delivery` left open -- by answering a \
         question that cannot arise for a per-connection write."
    );

    drop(asker);
    drop(bystander);
    server.shutdown().await;
}

/// A catch-up and a live broadcast do not confuse each other.
///
/// **The asker's own earlier messages come back through the catch-up while a second
/// client's new message arrives live**, and the asker must be able to tell them
/// apart — or rather, must not have to, because both are the same frame type and the
/// client's `core::ordering` reconciles them by `client_msg_id`. What this asserts
/// is that the interleaving loses nothing: every id arrives exactly once.
#[tokio::test]
async fn a_catch_up_and_a_live_broadcast_both_arrive_exactly_once() {
    let server = TestServer::start().await;
    let planted = plant_run(
        server.database(),
        server.admin_user_id(),
        DEFAULT_CHANNEL_ID,
        3,
    );

    let mut asker = server.connect().await;
    let mut other = server.connect().await;

    // Ask for the planted rows, and send a live message from the other connection
    // while the catch-up is on the wire. `asker` receives the catch-up; `other`
    // receives nothing for it (the previous test) and the ack for its own send.
    asker
        .send_text(&json(&resync(DEFAULT_CHANNEL_ID, at(-1))))
        .await
        .expect("the resync to reach the server");
    other
        .send_text(&json(&send(
            "7f8e9d0c-1b2a-4394-8576-655443332211",
            DEFAULT_CHANNEL_ID,
            "live",
        )))
        .await
        .expect("the live send to reach the server");

    let mut seen: Vec<String> = Vec::new();
    // The catch-up's three rows, then the live broadcast, in whichever order the
    // socket produced them.
    for expected in ["a row of the catch-up", "another row"] {
        seen.push(next_replayed(&mut asker, expected).await.id);
    }
    while seen.len() < 4 {
        seen.push(next_replayed(&mut asker, "the live broadcast").await.id);
    }

    let live = {
        let envelope = next_of_kind(&mut other, "the acknowledgement of the live send").await;
        match envelope.frame {
            ServerFrame::MessageAck { message, .. } => message.id,
            other => panic!("expected a message.ack, got {other:?}"),
        }
    };

    assert_eq!(
        seen,
        vec![
            planted[0].clone(),
            planted[1].clone(),
            planted[2].clone(),
            live
        ],
        "the catch-up and the live broadcast are both `message.new`, and every row \
         arrives exactly once: a replay is not broadcast, and a broadcast is not a \
         replay"
    );

    drop(asker);
    drop(other);
    server.shutdown().await;
}
