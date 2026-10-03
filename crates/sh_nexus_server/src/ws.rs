//! The transport: one WebSocket, one select loop, no futures dependency.
//!
//! # How a connection is served
//!
//! One task per connection, and one `tokio::select!` over two sources: frames from
//! the peer, and frames to deliver to it. Both halves of the fan-out and both
//! halves of the protocol live in that loop, so there is no writer task, no
//! outbound queue with a capacity to get wrong, and no place for a frame to be
//! dropped between two channels.
//!
//! That is only possible because `axum::extract::ws::WebSocket` has **inherent**
//! `recv` and `send` methods in axum 0.8. The usual axum example splits the
//! socket with `futures_util::StreamExt::split` so each half can be owned by a
//! task of its own, and `futures-util` is not a declared dependency of
//! `crates/sh_nexus_server/Cargo.toml`. A loop needs nothing extra, and adding a
//! dependency for it would need an `AGENTS.md` §7.2 audit this milestone does not
//! have.
//!
//! # The one borrow rule worth writing down
//!
//! `tokio::select!` builds every branch future before polling any of them, so two
//! branches may not both hold `&mut socket`. One may: the branch *bodies* run
//! after the futures tuple is dropped, which is why the receive branch below can
//! call `handle_frame(.., &mut socket, ..)`. axum's own WebSocket example relies
//! on the same property.
//!
//! # What this milestone does not handle, and why dropping is the protocol's answer
//!
//! `reaction.add`, `typing.start`, `typing.stop` and `resync` decode and are then
//! logged at `warn!`. They are ADR-010's next PR.
//!
//! A `warn!` and a kept connection, with no `error` frame, is the protocol's own
//! prescribed behaviour for a frame this end does not implement: `sh_nexus_wire`'s
//! compatibility policy drops an unknown advisory frame and keeps the socket open
//! (`crates/sh_nexus_wire/src/error.rs`, the table of which failures a caller must
//! act on). Sending an `error` frame instead would be actively harmful --
//! `crates/sh_nexus/src/network/mapping.rs` maps `ServerFrame::Error` onto
//! `ConnectionState::Rejected`, so a client that sent a `typing.start` would be
//! told its connection had been rejected over a frame whose loss costs nothing.
//!
//! `resync` is the one gap in that list with teeth, and it is named rather than
//! buried: a reconnecting client that asks to catch up will not be sent what it
//! missed. It is data-bearing, so it eventually deserves a frame that says so --
//! and that frame belongs to the milestone that implements resync, because
//! inventing a code now would be a protocol decision taken before anything can
//! send it.
//!
//! # Version negotiation
//!
//! [`ClientEnvelope::decode`] checks `v` before it reads a single field of the
//! body -- that ordering is `sh_nexus_wire`'s, not this module's -- so all this
//! layer has to do is honour the two outcomes. An unsupported major version gets
//! [`ServerEnvelope::version_rejection`] and then a close; every other failure is
//! one bad frame, logged at `warn!` with the connection kept, which is the exact
//! division [`WireError::is_fatal`] exists to make.

use axum::extract::ws::{close_code, CloseFrame, Message, Utf8Bytes, WebSocket, WebSocketUpgrade};
use axum::extract::State;
use axum::response::Response;
use sh_nexus_wire::{ClientEnvelope, ClientFrame, ServerEnvelope, WireError};
use tokio::sync::broadcast::error::RecvError;
use tokio::sync::broadcast::Receiver;
use tracing::{debug, error, info, warn};

use crate::hub::{ConnectionId, Delivery, CAPACITY};
use crate::message;
use crate::AppState;

/// The largest frame this server will accept, in bytes.
///
/// The only content policy in this milestone, and it is a transport one. It is
/// what stops `AGENTS.md` §7.1's "no unbounded growth of in-memory state" from
/// being decided by how much a peer is willing to send: a frame up to 64 KiB is
/// read, a larger one ends the connection.
///
/// **What the peer sees is an *unclean* close, and that is worth stating rather
/// than discovering.** `tungstenite` 0.29 (which axum 0.8 drives) refuses an
/// oversized frame with `Error::Capacity(MessageTooLong)` and moves its state
/// straight to `Terminated`; it does not send RFC 6455's 1009 before dropping the
/// socket, and it cannot be made to, because a `send` after that state is
/// `AlreadyClosed`. So the peer observes the stream ending without a close frame
/// -- and on Windows, an RST rather than a FIN, because the socket is closed with
/// unread data still in its receive buffer. The server logs the limit and the
/// driver error, which is where the diagnosis lives.
///
/// **A per-message length policy is not here and does not belong here yet.**
/// `AGENTS.md` §4.3's table of deliberately-unvalidated fields says message
/// content length is "server policy, and the client's own limits would be a
/// second, disagreeing policy". The server's policy arrives with the auth and REST
/// milestone, where there is a user to attribute a rate limit to.
pub const MAX_FRAME_BYTES: usize = 64 * 1024;

/// Serves [`crate::WS_PATH`], upgrading to a WebSocket.
///
/// The route is a `get` and not an `any`: the upgrade is an HTTP/1.1 `GET`, and
/// axum's `WebSocketUpgrade` extractor rejects any other method. A plain
/// `GET /ws` without the upgrade headers is answered by axum with a 400 whose body
/// names the header that is missing, which is a better answer than this handler
/// pretending to be something it is not.
///
/// # Why the hub subscription happens here, before the upgrade response exists
///
/// A peer cannot send a frame before it has read the 101, so subscribing first
/// removes the window in which a connected client is not yet listening for
/// broadcasts -- a window in which a `message.new` sent by a second client would be
/// lost for the first with no error on either side. `AGENTS.md`'s first priority is
/// that a chat app never loses a message, and a message lost in a race between
/// "connected" and "listening" is lost silently.
///
/// The subscription is released by dropping it, which happens on every exit: the
/// upgrade task ends when the socket closes, when the peer initiates a close, and
/// when the peer vanishes during the handshake. `axum`'s spawned upgrade task
/// resolves in all three cases and drops the callback that holds the receiver.
pub async fn handler(upgrade: WebSocketUpgrade, State(state): State<AppState>) -> Response {
    let (connection_id, deliveries) = state.hub.connect();

    upgrade
        .max_message_size(MAX_FRAME_BYTES)
        .max_frame_size(MAX_FRAME_BYTES)
        .on_failed_upgrade(move |error| {
            // The refused path never reaches `on_upgrade`, so this closure is the
            // only thing that can say a connection id was issued and then not
            // used.
            warn!(
                connection = %connection_id,
                error = %error,
                "websocket upgrade refused"
            );
        })
        .on_upgrade(move |socket| serve(socket, state, connection_id, deliveries))
}

/// The per-connection task.
///
/// # Arguments
///
/// * `socket` - the upgraded connection.
/// * `state` - the store and the hub.
/// * `connection_id` - assigned by [`handler`] before the upgrade response existed.
/// * `deliveries` - this connection's subscription, dropped with this future.
async fn serve(
    mut socket: WebSocket,
    state: AppState,
    connection_id: ConnectionId,
    mut deliveries: Receiver<Delivery>,
) {
    info!(connection = %connection_id, "websocket connection open");

    loop {
        tokio::select! {
            frame = socket.recv() => {
                match frame {
                    None => {
                        debug!(connection = %connection_id, "peer closed the connection");
                        return;
                    }
                    Some(Err(error)) => {
                        // A frame the transport could not read. The one this
                        // server provokes is an oversized frame, which tungstenite
                        // refuses with `Capacity(MessageTooLong)` and terminates
                        // without a close frame -- so the limit is named in the log,
                        // because the peer sees an unclean close and nothing else.
                        // The payload is never named; §7.5.
                        warn!(
                            connection = %connection_id,
                            max_frame_bytes = MAX_FRAME_BYTES,
                            error = %error,
                            "could not read a websocket frame"
                        );
                        return;
                    }
                    Some(Ok(frame)) => {
                        if !handle_frame(&state, connection_id, &mut socket, frame).await {
                            return;
                        }
                    }
                }
            }
            delivery = deliveries.recv() => {
                match delivery {
                    // The origin skip is the echo decision, documented on
                    // `Delivery`: the sender has the row already, in its ack.
                    Ok(delivery) if delivery.origin != connection_id => {
                        if let Err(error) = send(&mut socket, &delivery.envelope, connection_id).await {
                            debug!(
                                connection = %connection_id,
                                error = %error,
                                "connection closed while delivering a broadcast frame"
                            );
                            return;
                        }
                    }
                    Ok(_) => {}
                    Err(RecvError::Lagged(missed)) => {
                        // A visible gap, not a silent one. `hub.rs` explains why a
                        // dropped frame beats a fabricated one; this is where the
                        // gap becomes visible in the log.
                        warn!(
                            connection = %connection_id,
                            missed_frames = missed,
                            capacity = CAPACITY,
                            "connection fell behind the broadcast channel"
                        );
                    }
                    Err(RecvError::Closed) => {
                        debug!(connection = %connection_id, "broadcast channel closed");
                        return;
                    }
                }
            }
        }
    }
}

/// Decides what one received frame means, and whether the connection survives.
///
/// # Returns
///
/// `false` to end the connection. The only frame that produces `false` is one
/// whose major version this build refuses.
async fn handle_frame(
    state: &AppState,
    connection_id: ConnectionId,
    socket: &mut WebSocket,
    frame: Message,
) -> bool {
    match frame {
        Message::Text(text) => handle_text(state, connection_id, socket, text.as_str()).await,
        Message::Binary(bytes) => {
            // The protocol is JSON in text frames. A binary frame is not a frame
            // this protocol has: `ClientEnvelope::decode_bytes` would report
            // non-UTF-8 as `InvalidUtf8`, but a binary frame that *is* valid UTF-8
            // is a peer speaking the wrong protocol, and the size is the whole
            // useful fact about it.
            warn!(
                connection = %connection_id,
                bytes = bytes.len(),
                "binary frame on a text-only protocol"
            );
            true
        }
        // axum answers a ping with a pong on this connection already, and this
        // server never initiates one: `AGENTS.md` §7.4's keepalive is the client's
        // to want.
        Message::Ping(_) | Message::Pong(_) => true,
        Message::Close(frame) => {
            debug!(
                connection = %connection_id,
                with_reason = frame.is_some(),
                "peer initiated the websocket close handshake"
            );
            false
        }
    }
}

/// Decodes one client frame and acts on it.
///
/// # Returns
///
/// `false` to end the connection -- see [`handle_frame`].
async fn handle_text(
    state: &AppState,
    connection_id: ConnectionId,
    socket: &mut WebSocket,
    payload: &str,
) -> bool {
    let envelope = match ClientEnvelope::decode(payload) {
        Ok(envelope) => envelope,
        Err(error) => {
            if error.is_fatal() {
                return reject_version(socket, connection_id, error).await;
            }
            // Everything else is one frame this server cannot read: a peer bug, a
            // proxy in the middle, or an attack. Log the kind and the size, keep
            // the connection.
            warn!(
                connection = %connection_id,
                bytes = payload.len(),
                error = %error,
                "dropped an unreadable client frame"
            );
            return true;
        }
    };

    let client_msg_id = envelope.client_msg_id;
    match envelope.frame {
        ClientFrame::MessageSend {
            channel_id,
            content,
        } => {
            let outcome =
                message::accept(state, connection_id, &client_msg_id, &channel_id, &content).await;
            if let Err(error) = send(
                socket,
                &outcome.into_envelope(&client_msg_id),
                connection_id,
            )
            .await
            {
                debug!(
                    connection = %connection_id,
                    error = %error,
                    "connection closed before the send outcome could be written"
                );
                return false;
            }
            true
        }
        unimplemented_frame => {
            // The module docs explain why this is a `warn!` and an open connection
            // rather than an `error` frame.
            warn!(
                connection = %connection_id,
                frame = %unimplemented_frame.kind(),
                client_msg_id_len = client_msg_id.len(),
                "dropped a client frame this milestone does not implement"
            );
            true
        }
    }
}

/// Sends one frame.
///
/// # Returns
///
/// `Err` with the reason the connection is finished. Encoding cannot fail for any
/// value `sh_nexus_wire` can hold -- see `ClientEnvelope::encode` for why it is a
/// `Result` anyway -- so an encode failure here means the server's own state is
/// unencodable, which is worth an `error!` and a closed connection and is not
/// worth a panic in a network send path (`AGENTS.md` §2.1).
async fn send(
    socket: &mut WebSocket,
    envelope: &ServerEnvelope,
    connection_id: ConnectionId,
) -> Result<(), String> {
    let payload = envelope.encode().map_err(|error| {
        error!(
            connection = %connection_id,
            frame = %envelope.kind(),
            error = %error,
            "could not encode a server frame"
        );
        error.to_string()
    })?;

    socket.send(Message::text(payload)).await.map_err(|error| {
        warn!(
            connection = %connection_id,
            frame = %envelope.kind(),
            error = %error,
            "could not write a server frame"
        );
        error.to_string()
    })
}

/// Answers a peer whose major version this build does not speak, then hangs up.
///
/// `PLAN.md` §6: *"the server responds with an `error` frame and closes."* Both
/// halves, in that order. The close is what makes the rejection unmissable to a
/// client that would otherwise sit in a reconnect loop against a server that
/// refuses it identically every time.
///
/// **Close code 1002, protocol error**, rather than 1003. 1003 is "received a type
/// of data it cannot accept", which describes a payload the endpoint cannot
/// interpret; this frame was a frame of a known type in a dialect this build does
/// not speak, which is what 1002 names. What the *user* reads is the `error`
/// frame, not the code -- and `ServerEnvelope::version_rejection` carries the
/// peer-facing sentence while stamping this build's version rather than the peer's.
///
/// # Returns
///
/// Always `false`: a rejected version ends the connection, and the two
/// `match` arms below exist to make an impossible state a compile-time concern
/// rather than a runtime panic.
async fn reject_version(
    socket: &mut WebSocket,
    connection_id: ConnectionId,
    error: WireError,
) -> bool {
    let rejection = match error {
        WireError::UnsupportedVersion(rejection) => rejection,
        other => {
            // Unreachable by construction: `is_fatal` is true for exactly one
            // variant and the caller checked it. Matched rather than unwrapped so
            // that adding a second fatal variant is a compile error here instead of
            // a panic on a network path.
            error!(
                connection = %connection_id,
                error = %other,
                "a non-version wire error was reported as fatal; keeping the connection"
            );
            return true;
        }
    };

    info!(
        connection = %connection_id,
        received = rejection.received,
        supported = ?rejection.supported,
        "rejecting a peer whose protocol major version this build does not speak"
    );

    let envelope = ServerEnvelope::version_rejection(&rejection);
    if let Err(error) = send(socket, &envelope, connection_id).await {
        debug!(
            connection = %connection_id,
            error = %error,
            "the version rejection could not be delivered; closing anyway"
        );
        return false;
    }

    let _ = socket
        .send(Message::Close(Some(CloseFrame {
            code: close_code::PROTOCOL,
            reason: Utf8Bytes::from_static("unsupported protocol major version"),
        })))
        .await;
    false
}
