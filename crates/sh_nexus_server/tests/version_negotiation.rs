//! Version negotiation, as `AGENTS.md` §7.4 states it: *"reject explicitly"*.
//!
//! The distinction under test is `sh_nexus_wire`'s -- [`WireError::is_fatal`] is
//! true for exactly one variant, and the server must honour both halves. A version
//! rejection is a statement about the connection and ends it; every other
//! unreadable frame is one frame, and the connection survives it. A server that
//! dropped the socket on malformed JSON would be unusable, and a server that kept
//! the socket on a version mismatch would leave a client in a reconnect loop
//! against a server that refuses it identically every time.

mod support;

use sh_nexus_server::db::DEFAULT_CHANNEL_ID;
use sh_nexus_wire::frame::{ClientEnvelope, ClientFrame, ServerEnvelope, ServerFrame};
use sh_nexus_wire::{PROTOCOL_VERSION, UNSUPPORTED_VERSION_CODE};
use support::TestServer;

/// A syntactically valid UUID; its validity is the client's rule, not this suite's.
const CLIENT_MSG_ID: &str = "5c8a1b2d-3e4f-4a5b-8c9d-0e1f2a3b4c5d";

/// A major version this build does not speak.
///
/// `PROTOCOL_VERSION + 1` rather than a literal, so the test cannot accidentally
/// agree with the server by naming today's version as "unsupported".
fn unsupported_major() -> u16 {
    PROTOCOL_VERSION + 1
}

fn send_at_version(version: u16) -> ClientEnvelope {
    ClientEnvelope {
        v: version,
        client_msg_id: CLIENT_MSG_ID.to_owned(),
        frame: ClientFrame::MessageSend {
            channel_id: DEFAULT_CHANNEL_ID.to_owned(),
            content: "hello team".to_owned(),
        },
    }
}

fn json(envelope: &ClientEnvelope) -> String {
    envelope
        .encode()
        .expect("a sh_nexus_wire client envelope always encodes")
}

#[tokio::test]
async fn an_unsupported_major_version_is_rejected_with_an_error_frame() {
    let server = TestServer::start().await;
    let mut alice = server.connect().await;

    alice
        .send_text(&json(&send_at_version(unsupported_major())))
        .await
        .expect("the mismatched frame");

    let envelope = ServerEnvelope::decode(&alice.expect_text("the rejection").await)
        .expect("the rejection to be a decodable server frame");

    match envelope.frame {
        ServerFrame::Error { code, detail } => {
            assert_eq!(code, UNSUPPORTED_VERSION_CODE);
            // The detail is compared against the wire crate's own sentence, byte
            // for byte, rather than against a substring of it. The server must not
            // write its own explanation here: `UnsupportedVersion::detail` already
            // names the peer's version, names the set this build speaks, and states
            // that reconnecting will not help. An exact comparison is what proves the
            // server delegated instead of paraphrasing.
            assert_eq!(
                detail,
                sh_nexus_wire::negotiate(unsupported_major())
                    .expect_err("a version this build does not speak must be rejected")
                    .detail(),
                "the server must send the wire crate's own detail verbatim"
            );
        }
        other => panic!("expected an error frame, got {other:?}"),
    }

    // `PLAN.md` §6: the server responds with an `error` frame *and closes*. Both
    // halves, and the close is what stops a reconnect loop.
    alice.expect_closed("the close after the rejection").await;

    assert_eq!(
        support::query_i64(
            server.database(),
            "SELECT COUNT(*) FROM messages WHERE client_msg_id = ?1",
            CLIENT_MSG_ID,
        ),
        0,
        "a frame whose version was refused must not be interpreted at all -- the \
         wire crate checks `v` before it reads a single field of the body"
    );

    drop(alice);
    server.shutdown().await;
}

#[tokio::test]
async fn a_zero_major_version_is_rejected_too() {
    let server = TestServer::start().await;
    let mut alice = server.connect().await;

    // Zero rather than "a big number": a build that defaulted `v` to zero, or a
    // peer that omitted it in a way that parsed. `SUPPORTED_MAJOR_VERSIONS` is a
    // list, so both directions have to be refused by the same rule.
    alice
        .send_text(&json(&send_at_version(0)))
        .await
        .expect("the zero-version frame");

    let envelope = ServerEnvelope::decode(&alice.expect_text("the rejection").await)
        .expect("a decodable server frame");
    match envelope.frame {
        ServerFrame::Error { code, .. } => assert_eq!(code, UNSUPPORTED_VERSION_CODE),
        other => panic!("expected an error frame, got {other:?}"),
    }
    alice.expect_closed("the close after the rejection").await;

    drop(alice);
    server.shutdown().await;
}

#[tokio::test]
async fn a_frame_with_this_builds_version_is_accepted() {
    let server = TestServer::start().await;
    let mut alice = server.connect().await;

    alice
        .send_text(&json(&send_at_version(PROTOCOL_VERSION)))
        .await
        .expect("the version-matched frame");

    let envelope = ServerEnvelope::decode(&alice.expect_text("the ack").await)
        .expect("a decodable server frame");
    match envelope.frame {
        ServerFrame::MessageAck {
            client_msg_id,
            message,
        } => {
            assert_eq!(client_msg_id, CLIENT_MSG_ID);
            assert_eq!(message.content, "hello team");
        }
        other => panic!("expected an ack for the accepted frame, got {other:?}"),
    }

    // And the connection is still up, because nothing about an accepted version
    // closes a socket.
    assert_eq!(alice.next_text_within(support::SILENCE_BUDGET).await, None);

    drop(alice);
    server.shutdown().await;
}

#[tokio::test]
async fn malformed_json_is_dropped_without_closing_the_connection() {
    let server = TestServer::start().await;
    let mut alice = server.connect().await;

    // Three different ways to be unreadable, none of them a version problem:
    // not JSON at all, JSON with no frame type, and a frame type that belongs to
    // the other direction.
    for garbage in [
        "{not json at all",
        r#"{"v":1,"client_msg_id":"x"}"#,
        r#"{"v":1,"type":"message.ack","client_msg_id":"x","message":{}}"#,
    ] {
        alice
            .send_text(garbage)
            .await
            .expect("the unreadable frame to reach the server");
    }

    // The connection is the assertion. A server that dropped the socket on one bad
    // frame would be unusable against any proxy that ever mangles a byte, and
    // `crates/sh_nexus_wire/src/error.rs` states the same division for the decoder.
    assert_eq!(
        alice.next_text_within(support::SILENCE_BUDGET).await,
        None,
        "an unreadable frame is dropped silently to the peer and loudly to the log; \
         the peer is told nothing because there is nothing it could do"
    );

    let valid = "7d9b2c3e-4f5a-4b6c-9d0e-1f2a3b4c5d6e";
    alice
        .send_text(&json(&ClientEnvelope::new(
            valid,
            ClientFrame::MessageSend {
                channel_id: DEFAULT_CHANNEL_ID.to_owned(),
                content: "still here".to_owned(),
            },
        )))
        .await
        .expect("a valid frame after three unreadable ones");

    let envelope = ServerEnvelope::decode(&alice.expect_text("the ack").await)
        .expect("a decodable server frame");
    match envelope.frame {
        ServerFrame::MessageAck { message, .. } => assert_eq!(message.content, "still here"),
        other => panic!("expected the connection to still work, got {other:?}"),
    }

    drop(alice);
    server.shutdown().await;
}

#[tokio::test]
async fn a_frame_over_the_size_ceiling_ends_the_connection_and_stores_nothing() {
    let server = TestServer::start().await;
    let mut alice = server.connect().await;

    // `AGENTS.md` §7.1 forbids unbounded growth of in-memory state, and a peer
    // decides how much it sends. The ceiling is the boundary.
    let oversized = "x".repeat(sh_nexus_server::ws::MAX_FRAME_BYTES + 1024);
    alice
        .send_text(&json(&ClientEnvelope::new(
            CLIENT_MSG_ID,
            ClientFrame::MessageSend {
                channel_id: DEFAULT_CHANNEL_ID.to_owned(),
                content: oversized,
            },
        )))
        .await
        .expect("the oversized frame to reach the server");

    // **No close code is asserted, and the reason is worth stating.** RFC 6455
    // §7.4.1 says 1009, and `tungstenite` 0.29 -- the engine axum 0.8 drives --
    // does not send it: an oversized frame is refused with
    // `Error::Capacity(MessageTooLong)`, which moves its state to `Terminated`, and
    // a `send` after that state is `AlreadyClosed`. So the peer observes the stream
    // ending without a close frame, and on Windows an RST rather than a FIN.
    // Asserting 1009 here would be asserting a behaviour the dependency does not
    // have; asserting nothing about *how* it ends would let a server that simply
    // hung pass. This asserts it ended.
    alice
        .expect_closed("the stream to end after an oversized frame")
        .await;

    assert_eq!(
        support::query_i64(
            server.database(),
            "SELECT COUNT(*) FROM messages WHERE client_msg_id = ?1",
            CLIENT_MSG_ID,
        ),
        0,
        "an oversized frame must not be partially stored"
    );

    drop(alice);
    server.shutdown().await;
}
