//! The property `AGENTS.md` §7.4 exists for: a send is idempotent under its
//! `client_msg_id`, so a client that reconnects mid-send and replays does not put
//! the same message in a transcript twice -- and does not make every other client
//! receive it twice either.

mod support;

use sh_nexus_server::db::DEFAULT_CHANNEL_ID;
use sh_nexus_wire::frame::{ClientEnvelope, ClientFrame, ServerEnvelope, ServerFrame};
use support::{TestServer, SILENCE_BUDGET};

/// The id both sends in this suite use, which is the point of the suite.
const REPLAYED_MSG_ID: &str = "5c8a1b2d-3e4f-4a5b-8c9d-0e1f2a3b4c5d";

/// A second, distinct id, for the case that proves the dedupe is keyed on the id
/// and not on "the last message".
const DISTINCT_MSG_ID: &str = "7d9b2c3e-4f5a-4b6c-9d0e-1f2a3b4c5d6e";

fn send(client_msg_id: &str, content: &str) -> ClientEnvelope {
    ClientEnvelope::new(
        client_msg_id,
        ClientFrame::MessageSend {
            channel_id: DEFAULT_CHANNEL_ID.to_owned(),
            content: content.to_owned(),
        },
    )
}

fn json(envelope: &ClientEnvelope) -> String {
    envelope
        .encode()
        .expect("a sh_nexus_wire client envelope always encodes")
}

/// Decodes the frame on a socket that is expected to carry an ack.
///
/// # Panics
///
/// If it is not a `message.ack`, or if the ack echoes a different id.
fn ack_of(text: &str, expected_id: &str) -> sh_nexus_wire::WireMessage {
    let envelope = ServerEnvelope::decode(text)
        .unwrap_or_else(|error| panic!("expected a message.ack, got {text:?}: {error}"));
    match envelope.frame {
        ServerFrame::MessageAck {
            client_msg_id,
            message,
        } => {
            assert_eq!(client_msg_id, expected_id);
            message
        }
        other => panic!("expected a message.ack, got {other:?}"),
    }
}

#[tokio::test]
async fn a_replayed_send_creates_no_second_row_and_no_second_broadcast() {
    let server = TestServer::start().await;
    let mut alice = server.connect().await;
    let mut bob = server.connect().await;

    alice
        .send_text(&json(&send(REPLAYED_MSG_ID, "hello team")))
        .await
        .expect("the original send");
    let first = ack_of(
        &alice.expect_text("the original ack").await,
        REPLAYED_MSG_ID,
    );
    bob.expect_text("the original broadcast").await;

    // The replay. Same id, same body, same socket -- the shape a client produces
    // when it reconnects and its outbox still holds the row.
    alice
        .send_text(&json(&send(REPLAYED_MSG_ID, "hello team")))
        .await
        .expect("the replayed send");
    let second = ack_of(
        &alice.expect_text("the ack for the replay").await,
        REPLAYED_MSG_ID,
    );

    // The ack is the proof that the replay resolved to the stored row rather than
    // to a second one: a fresh row would carry a fresh server id and a later
    // timestamp, and the client reconciling on `client_msg_id` would end up with
    // two rows for one send.
    assert_eq!(
        first, second,
        "the replay must be acknowledged with the row that was already stored"
    );
    assert_eq!(
        first.id, second.id,
        "one client_msg_id is one server-assigned message id"
    );

    // Positive first, negative second: the ack above proves the replay reached the
    // store, so the silence from bob is about the broadcast and nothing else.
    let rebroadcast = bob.next_text_within(SILENCE_BUDGET).await;
    assert!(
        rebroadcast.is_none(),
        "a replayed send must not be broadcast again; receiving clients have no \
         client_msg_id of their own to dedup against, so a second delivery is a \
         second message in a transcript, got {rebroadcast:?}"
    );

    assert_eq!(
        support::query_i64(
            server.database(),
            "SELECT COUNT(*) FROM messages WHERE client_msg_id = ?1",
            REPLAYED_MSG_ID,
        ),
        1,
        "a replayed send must not create a second row"
    );
    assert_eq!(
        support::query_i64(
            server.database(),
            "SELECT COUNT(*) FROM messages WHERE channel_id = ?1",
            DEFAULT_CHANNEL_ID,
        ),
        1,
        "one send must produce one row in the channel, not one row plus one duplicate"
    );

    // Teardown: the clients go first, then the server. `TestServer::shutdown` cannot
    // remove a database file the server still has open, and only a closed client
    // socket lets the server's per-connection task let go of it.
    drop(alice);
    drop(bob);
    server.shutdown().await;
}

#[tokio::test]
async fn a_replayed_send_after_a_reconnect_is_acknowledged_again() {
    let server = TestServer::start().await;

    let first_message_id = {
        let mut alice = server.connect().await;
        alice
            .send_text(&json(&send(REPLAYED_MSG_ID, "hello team")))
            .await
            .expect("the original send");
        let acked = ack_of(
            &alice.expect_text("the original ack").await,
            REPLAYED_MSG_ID,
        );
        acked.id
    };

    // The connection is dropped and a new one opened: `AGENTS.md` §8.1's Reconnect
    // Flow's starting position, with the client's outbox still holding the row.
    // No sleep and no waiting -- the new connection is a new socket, and the ack
    // below can only come from the server having processed the replay.
    let mut alice_again = server.connect().await;
    alice_again
        .send_text(&json(&send(REPLAYED_MSG_ID, "hello team")))
        .await
        .expect("the replay after reconnecting");

    let reconnected = ack_of(
        &alice_again.expect_text("the ack after reconnecting").await,
        REPLAYED_MSG_ID,
    );
    assert_eq!(
        reconnected.id, first_message_id,
        "a replay across a reconnect is the same message, so it is the same row"
    );

    assert_eq!(
        support::query_i64(
            server.database(),
            "SELECT COUNT(*) FROM messages WHERE client_msg_id = ?1",
            REPLAYED_MSG_ID,
        ),
        1
    );

    drop(alice_again);
    server.shutdown().await;
}

#[tokio::test]
async fn a_distinct_client_msg_id_is_not_deduped() {
    let server = TestServer::start().await;
    let mut alice = server.connect().await;
    let mut bob = server.connect().await;

    alice
        .send_text(&json(&send(REPLAYED_MSG_ID, "same body")))
        .await
        .expect("the first send");
    alice
        .send_text(&json(&send(DISTINCT_MSG_ID, "same body")))
        .await
        .expect("the second send");

    // Two acknowledgements, with two different server ids: the dedupe is keyed on
    // `client_msg_id`, not on the body. A dedupe on content would silently drop a
    // user's second "yes", and a user cannot tell the difference.
    let first = ack_of(&alice.expect_text("the first ack").await, REPLAYED_MSG_ID);
    let second = ack_of(&alice.expect_text("the second ack").await, DISTINCT_MSG_ID);
    assert_ne!(
        first.id, second.id,
        "two client ids must produce two server ids"
    );
    assert_eq!(first.content, second.content);

    bob.expect_text("the first broadcast").await;
    bob.expect_text("the second broadcast").await;

    assert_eq!(
        support::query_i64(
            server.database(),
            "SELECT COUNT(*) FROM messages WHERE channel_id = ?1",
            DEFAULT_CHANNEL_ID,
        ),
        2
    );

    drop(alice);
    drop(bob);
    server.shutdown().await;
}

#[tokio::test]
async fn a_replay_does_not_advance_the_channel_cursor() {
    let server = TestServer::start().await;
    let mut alice = server.connect().await;

    alice
        .send_text(&json(&send(REPLAYED_MSG_ID, "hello team")))
        .await
        .expect("the original send");
    alice.expect_text("the original ack").await;

    let cursor = |server: &TestServer| {
        support::query_i64(
            server.database(),
            "SELECT last_message_at_unix_ms FROM channels WHERE id = ?1",
            DEFAULT_CHANNEL_ID,
        )
    };
    let before = cursor(&server);

    alice
        .send_text(&json(&send(REPLAYED_MSG_ID, "hello team")))
        .await
        .expect("the replayed send");
    alice.expect_text("the ack for the replay").await;

    // `last_message_at` is the upper bound a resync asks for. If a replay moved it
    // forward, a client that resumed from it would skip the very message the
    // replay was about -- a gap, which `AGENTS.md` §8.1 forbids outright.
    assert_eq!(
        cursor(&server),
        before,
        "a replayed send must not move the channel's resume cursor"
    );

    drop(alice);
    server.shutdown().await;
}
