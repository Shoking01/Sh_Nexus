//! `AGENTS.md` §8.1's Real-time Flow, against a real socket.
//!
//! Two clients, one sends, the sender is acknowledged and the other is told. Plus
//! the two properties that make the round trip trustworthy rather than merely
//! working: the sender is **not** sent its own message (the echo decision
//! `sh_nexus_server::hub::Delivery` documents), and the acknowledgement carries the
//! *stored* message rather than the one that was sent.

mod support;

use sh_nexus_server::db::{DEFAULT_CHANNEL_ID, UNATTRIBUTED_USER_ID};
use sh_nexus_wire::frame::{ClientEnvelope, ClientFrame, ServerEnvelope, ServerFrame};
use support::{TestClient, TestServer};

/// Alice's send id. Fixed, so a duplicate that was not caught shows up as two
/// rows with a known id rather than as an anonymous second row.
const ALICE_MSG_ID: &str = "5c8a1b2d-3e4f-4a5b-8c9d-0e1f2a3b4c5d";

/// Bob's send id. Distinct from [`ALICE_MSG_ID`] on purpose: reusing one id across
/// two clients is exactly the replay this suite's sibling tests cover, and here it
/// would be a bug in the test rather than a property of the server.
const BOB_MSG_ID: &str = "7d9b2c3e-4f5a-4b6c-9d0e-1f2a3b4c5d6e";

/// A `message.send` envelope the client's own boundary would accept.
fn send(client_msg_id: &str, channel_id: &str, content: &str) -> ClientEnvelope {
    ClientEnvelope::new(
        client_msg_id,
        ClientFrame::MessageSend {
            channel_id: channel_id.to_owned(),
            content: content.to_owned(),
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

/// Decodes a server frame, naming the expectation in the panic.
fn decode(text: &str, expected: &str) -> ServerEnvelope {
    ServerEnvelope::decode(text)
        .unwrap_or_else(|error| panic!("expected {expected}, got {text:?}: {error}"))
}

/// Takes the message out of a `message.ack` or a `message.new`.
///
/// # Panics
///
/// If the frame is neither.
fn message_of(envelope: ServerEnvelope, expected: &str) -> sh_nexus_wire::WireMessage {
    match envelope.frame {
        ServerFrame::MessageAck {
            client_msg_id,
            message,
        } => {
            assert_eq!(
                client_msg_id, ALICE_MSG_ID,
                "the ack must echo the id the client minted, or the client cannot \
                 reconcile it with its optimistic row"
            );
            message
        }
        ServerFrame::MessageNew { message } => message,
        other => panic!("expected a message frame for {expected}, got {other:?}"),
    }
}

#[tokio::test]
async fn two_clients_exchange_a_message_and_the_sender_is_acknowledged() {
    let server = TestServer::start().await;
    let mut alice = server.connect().await;
    let mut bob = server.connect().await;

    // Both connections are subscribed before their handshakes completed, which is
    // the ordering `ws::handler` exists to guarantee. Asserting it here means a
    // change that moves the subscription after the upgrade fails this test,
    // rather than making every other test in the suite intermittently miss a
    // broadcast.
    assert_eq!(
        server.connection_count(),
        2,
        "both clients must be listening by the time their handshake returned"
    );

    alice
        .send_text(&json(&send(ALICE_MSG_ID, DEFAULT_CHANNEL_ID, "hello team")))
        .await
        .expect("alice's send to reach the server");

    let acked = message_of(
        decode(
            &alice.expect_text("alice's message.ack").await,
            "a message.ack for alice",
        ),
        "alice's ack",
    );
    let delivered = message_of(
        decode(
            &bob.expect_text("bob's message.new").await,
            "a message.new for bob",
        ),
        "bob's broadcast",
    );

    assert_eq!(
        acked, delivered,
        "the ack and the broadcast must carry the same stored row, not two readings"
    );
    assert_eq!(acked.client_msg_id, ALICE_MSG_ID);
    assert_eq!(acked.channel_id, DEFAULT_CHANNEL_ID);
    assert_eq!(acked.content, "hello team");
    assert_eq!(
        acked.user_id,
        server.admin_user_id(),
        "the author comes from the handshake's session, so the transcript names the \
         account that sent the message rather than a reserved row"
    );
    assert_ne!(
        acked.user_id, UNATTRIBUTED_USER_ID,
        "the reserved row is not the author of anything this build accepts; `db.rs` \
         keeps the row only so a transcript written before this milestone still \
         renders an author"
    );
    assert!(
        !acked.id.is_empty(),
        "the server mints the id; a blank one cannot be addressed"
    );
    assert_ne!(
        acked.id, ALICE_MSG_ID,
        "the server id must not be the client's id: `PLAN.md` §5 makes them different \
         things, and the client reconciles on the second while addressing by the first"
    );
    assert!(
        acked.edited_at.is_none() && acked.thread_id.is_none(),
        "this milestone has no edits and no threads"
    );
    assert!(
        acked.reactions.is_empty() && acked.attachments.is_empty(),
        "this milestone has no reaction or attachment storage at all, so an empty list \
         claims a fact it cannot know"
    );

    // Exactly one row, and it is the one both clients were told about.
    assert_eq!(
        support::query_i64(
            server.database(),
            "SELECT COUNT(*) FROM messages WHERE client_msg_id = ?1",
            ALICE_MSG_ID,
        ),
        1,
        "one send must produce exactly one row"
    );

    // The channel's resume cursor moved, because `last_message_at` is the upper
    // bound a resync will use and `PLAN.md` §6 requires it to track the newest
    // message.
    assert_eq!(
        support::query_i64(
            server.database(),
            "SELECT COUNT(*) FROM channels WHERE id = ?1 AND last_message_at_unix_ms IS NOT NULL",
            DEFAULT_CHANNEL_ID,
        ),
        1,
        "an accepted message advances the channel's cursor"
    );

    // Teardown: the clients go first, then the server. `TestServer::shutdown` cannot
    // remove a database file the server still has open, and only a closed client
    // socket lets the server's per-connection task let go of it.
    drop(alice);
    drop(bob);
    server.shutdown().await;
}

#[tokio::test]
async fn the_sender_does_not_receive_its_own_message() {
    let server = TestServer::start().await;
    let mut alice = server.connect().await;
    let mut bob = server.connect().await;

    alice
        .send_text(&json(&send(ALICE_MSG_ID, DEFAULT_CHANNEL_ID, "hello team")))
        .await
        .expect("alice's send to reach the server");

    // The positive half first, so the absence below is a fact about the broadcast
    // and not about a send that never worked.
    alice.expect_text("alice's message.ack").await;
    bob.expect_text("bob's message.new").await;

    let echoed = alice.next_text_within(support::SILENCE_BUDGET).await;
    assert!(
        echoed.is_none(),
        "the sender already holds the row in its ack; an echo would deliver the same \
         message twice to the one client that definitely has it, got {echoed:?}"
    );

    drop(alice);
    drop(bob);
    server.shutdown().await;
}

#[tokio::test]
async fn every_other_connected_client_receives_every_message() {
    let server = TestServer::start().await;
    let mut alice = server.connect().await;
    let mut bob = server.connect().await;
    let mut carol = server.connect().await;

    alice
        .send_text(&json(&send(ALICE_MSG_ID, DEFAULT_CHANNEL_ID, "first")))
        .await
        .expect("alice's send");
    bob.send_text(&json(&send(BOB_MSG_ID, DEFAULT_CHANNEL_ID, "second")))
        .await
        .expect("bob's send");

    alice.expect_text("alice's ack").await;
    bob.expect_text("bob's ack").await;

    // The counts are the interesting part and they are not all the same. Alice
    // hears Bob's message and not her own; Bob hears Alice's and not his own;
    // Carol, who sent nothing, hears both. Each sender's own message reached it as
    // an ack instead, which is the echo decision `Delivery` documents -- so a
    // client that sent nothing is the only one that sees every message.
    assert_eq!(
        body_of(&mut alice, "alice's one message.new").await,
        "second",
        "alice must hear the message she did not send"
    );
    assert_eq!(
        body_of(&mut bob, "bob's one message.new").await,
        "first",
        "bob must hear the message he did not send"
    );
    assert_eq!(
        body_of(&mut carol, "carol's first message.new").await,
        "first",
        "a client that sent nothing hears every message, in acceptance order"
    );
    assert_eq!(
        body_of(&mut carol, "carol's second message.new").await,
        "second"
    );

    // And nobody heard a third thing: exactly one message.new each for the two
    // senders, and none at all beyond carol's two.
    assert_eq!(alice.next_text_within(support::SILENCE_BUDGET).await, None);
    assert_eq!(bob.next_text_within(support::SILENCE_BUDGET).await, None);
    assert_eq!(carol.next_text_within(support::SILENCE_BUDGET).await, None);

    assert_eq!(
        support::query_i64(
            server.database(),
            "SELECT COUNT(*) FROM messages WHERE channel_id = ?1",
            DEFAULT_CHANNEL_ID,
        ),
        2,
        "two distinct client ids produce two rows"
    );

    drop(alice);
    drop(bob);
    drop(carol);
    server.shutdown().await;
}

/// The body of the next `message.new` on `client`.
///
/// # Panics
///
/// If the next frame is not a `message.new`.
async fn body_of(client: &mut TestClient, expected: &str) -> String {
    let envelope = ServerEnvelope::decode(&client.expect_text(expected).await)
        .unwrap_or_else(|error| panic!("expected {expected}: {error}"));
    match envelope.frame {
        ServerFrame::MessageNew { message } => message.content,
        other => panic!("expected a message.new for {expected}, got {other:?}"),
    }
}
