//! Refusals, and the property that makes one worth implementing.
//!
//! ADR-010 records the shape: an opaque code the user can report beats a silently
//! dropped explanation. So every refusal names the field that broke the rule, in a
//! `code` the client can map and a `detail` a human can read -- and, because
//! `AGENTS.md` §7.5 forbids logging message content, no `detail` ever quotes the
//! value that was rejected.

mod support;

use sh_nexus_server::db::DEFAULT_CHANNEL_ID;
use sh_nexus_server::message::{
    BLANK_CHANNEL_ID, BLANK_CLIENT_MSG_ID, EMPTY_CONTENT, UNKNOWN_CHANNEL,
};
use sh_nexus_wire::frame::{ClientEnvelope, ClientFrame, ServerEnvelope, ServerFrame};
use support::TestServer;

/// A syntactically valid UUID, because `client_msg_id` validity is the client's
/// boundary's rule and this suite is not testing that.
const CLIENT_MSG_ID: &str = "5c8a1b2d-3e4f-4a5b-8c9d-0e1f2a3b4c5d";

fn send(client_msg_id: &str, channel_id: &str, content: &str) -> ClientEnvelope {
    ClientEnvelope::new(
        client_msg_id,
        ClientFrame::MessageSend {
            channel_id: channel_id.to_owned(),
            content: content.to_owned(),
        },
    )
}

fn json(envelope: &ClientEnvelope) -> String {
    envelope
        .encode()
        .expect("a sh_nexus_wire client envelope always encodes")
}

/// A `message.error` for `client_msg_id`, from the next frame on `text`.
///
/// # Panics
///
/// If the frame is not a `message.error` echoing that id.
fn refusal_of(text: &str, client_msg_id: &str) -> (String, String) {
    let envelope = ServerEnvelope::decode(text)
        .unwrap_or_else(|error| panic!("expected a message.error, got {text:?}: {error}"));
    match envelope.frame {
        ServerFrame::MessageError {
            client_msg_id: echoed,
            code,
            detail,
        } => {
            assert_eq!(
                echoed, client_msg_id,
                "a message.error must name the send it is about, or the client cannot \
                 move the right optimistic row to Failed"
            );
            (code, detail)
        }
        other => panic!("expected a message.error, got {other:?}"),
    }
}

#[tokio::test]
async fn a_blank_channel_id_is_refused_with_a_reason() {
    let server = TestServer::start().await;
    let mut alice = server.connect().await;

    alice
        .send_text(&json(&send(CLIENT_MSG_ID, "   ", "hello team")))
        .await
        .expect("the send");

    let (code, detail) = refusal_of(&alice.expect_text("the refusal").await, CLIENT_MSG_ID);
    assert_eq!(code, BLANK_CHANNEL_ID);
    assert!(
        detail.contains("channel_id"),
        "the detail must name the field that broke the rule: {detail:?}"
    );
    assert_eq!(
        support::query_i64(
            server.database(),
            "SELECT COUNT(*) FROM messages WHERE client_msg_id = ?1",
            CLIENT_MSG_ID,
        ),
        0,
        "a refused send must not leave a row"
    );

    // Teardown: the clients go first, then the server. `TestServer::shutdown` cannot
    // remove a database file the server still has open, and only a closed client
    // socket lets the server's per-connection task let go of it.
    drop(alice);
    server.shutdown().await;
}

#[tokio::test]
async fn an_empty_content_is_refused_with_a_reason() {
    let server = TestServer::start().await;
    let mut alice = server.connect().await;

    alice
        .send_text(&json(&send(CLIENT_MSG_ID, DEFAULT_CHANNEL_ID, "")))
        .await
        .expect("the send");

    let (code, detail) = refusal_of(&alice.expect_text("the refusal").await, CLIENT_MSG_ID);
    assert_eq!(code, EMPTY_CONTENT);
    assert!(
        detail.contains("content"),
        "the detail must name the field that broke the rule: {detail:?}"
    );
    assert_eq!(
        support::query_i64(
            server.database(),
            "SELECT COUNT(*) FROM messages WHERE client_msg_id = ?1",
            CLIENT_MSG_ID,
        ),
        0
    );

    drop(alice);
    server.shutdown().await;
}

#[tokio::test]
async fn a_whitespace_only_body_is_refused_and_the_detail_never_quotes_it() {
    let server = TestServer::start().await;
    let mut alice = server.connect().await;

    // Whitespace, not empty: `PLAN.md` §5 renders content as markdown, and a
    // whitespace-only message renders as an empty bubble, which is not something a
    // user can have meant to send.
    alice
        .send_text(&json(&send(CLIENT_MSG_ID, DEFAULT_CHANNEL_ID, "  \t\n ")))
        .await
        .expect("the send");

    let (code, detail) = refusal_of(&alice.expect_text("the refusal").await, CLIENT_MSG_ID);
    assert_eq!(code, EMPTY_CONTENT);
    assert!(
        !detail.contains("  \t"),
        "the detail reached a user and a client log, so it must not quote the body: \
         {detail:?}"
    );

    drop(alice);
    server.shutdown().await;
}

#[tokio::test]
async fn an_unknown_channel_is_refused_rather_than_created() {
    let server = TestServer::start().await;
    let mut alice = server.connect().await;

    alice
        .send_text(&json(&send(
            CLIENT_MSG_ID,
            "c_does_not_exist",
            "hello team",
        )))
        .await
        .expect("the send");

    let (code, detail) = refusal_of(&alice.expect_text("the refusal").await, CLIENT_MSG_ID);
    assert_eq!(code, UNKNOWN_CHANNEL);

    // Channel provisioning is the REST milestone's job. Creating a channel because
    // somebody typed its id would make any typo a new channel, and would do it
    // without the name, description and membership a channel needs.
    assert!(
        detail.contains("c_does_not_exist"),
        "the detail names the channel, which is server-issued bounded text and the \
         whole diagnostic: {detail:?}"
    );
    assert_eq!(
        support::query_i64(
            server.database(),
            "SELECT COUNT(*) FROM channels WHERE id = ?1",
            "c_does_not_exist",
        ),
        0,
        "a refused send must not create the channel it named"
    );

    drop(alice);
    server.shutdown().await;
}

#[tokio::test]
async fn a_blank_client_msg_id_is_refused_because_it_cannot_be_deduped() {
    let server = TestServer::start().await;
    let mut alice = server.connect().await;

    // `ClientEnvelope` makes the field *present*; blankness is a separate rule, and
    // this is the case where it earns its keep: a blank id is one every send
    // shares, so accepting it would make every send a replay of the first.
    alice
        .send_text(&json(&send("   ", DEFAULT_CHANNEL_ID, "hello team")))
        .await
        .expect("the send");

    let (code, detail) = refusal_of(&alice.expect_text("the refusal").await, "   ");
    assert_eq!(code, BLANK_CLIENT_MSG_ID);
    assert!(
        detail.contains("deduplicated"),
        "the detail must say what went wrong in terms the user can act on: {detail:?}"
    );
    assert_eq!(
        support::query_i64(
            server.database(),
            "SELECT COUNT(*) FROM messages WHERE client_msg_id = ?1",
            CLIENT_MSG_ID,
        ),
        0,
        "a send whose id is blank cannot be stored, because the column that dedupes \
         on it would then hold a value every other blank send shares"
    );

    drop(alice);
    server.shutdown().await;
}

#[tokio::test]
async fn a_refusal_keeps_the_connection_open_and_the_next_send_works() {
    let server = TestServer::start().await;
    let mut alice = server.connect().await;
    let mut bob = server.connect().await;

    alice
        .send_text(&json(&send(CLIENT_MSG_ID, "", "hello team")))
        .await
        .expect("the refused send");
    refusal_of(&alice.expect_text("the refusal").await, CLIENT_MSG_ID);

    // The refusal is per-send, not per-connection: `AGENTS.md` §3.3 requires
    // network failures to be recoverable states rather than disconnects, and a user
    // who mistyped a channel must not lose the socket to find out.
    let valid = "7d9b2c3e-4f5a-4b6c-9d0e-1f2a3b4c5d6e";
    alice
        .send_text(&json(&send(valid, DEFAULT_CHANNEL_ID, "hello team")))
        .await
        .expect("the valid send after the refusal");

    let envelope = ServerEnvelope::decode(&alice.expect_text("the ack for the valid send").await)
        .expect("a decodable server frame");
    match envelope.frame {
        ServerFrame::MessageAck {
            client_msg_id,
            message,
        } => {
            assert_eq!(client_msg_id, valid);
            assert_eq!(message.content, "hello team");
        }
        other => panic!("expected the ack for the valid send, got {other:?}"),
    }

    bob.expect_text("the broadcast of the valid send").await;

    drop(alice);
    drop(bob);
    server.shutdown().await;
}
