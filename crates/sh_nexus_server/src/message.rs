//! The message path: authorize, validate, persist, acknowledge, broadcast.
//!
//! # One working path, on purpose
//!
//! `AGENTS.md` §8.1 lists ten mandatory integration flows. This milestone
//! implements two of them -- the Real-time Flow and the login half of the Auth
//! Flow -- and ADR-010 names the rest (REST, presence, typing, reactions, resync)
//! as the next PR. There is no `todo!()` for them here, and that is a decision
//! rather than an omission: a `todo!()` compiles, looks like coverage, and panics
//! on the first user who reaches it. A frame type that is simply not handled is
//! visible.
//!
//! # Where authorization sits, and why it is not in `db.rs`
//!
//! **Channel membership is checked here and nowhere else**, and the placement is
//! argued rather than convenient: `AGENTS.md` §2.1 makes the wire/domain boundary
//! the place where untrusted text becomes trusted data, and `ws.rs` decides the
//! *connection's* authorization from the handshake's `Authorization` header. What
//! it cannot decide is whether this particular account may write in this particular
//! channel, because the frame names a channel and the connection does not -- one
//! socket serves every channel the client can type into. `db.rs` then checks only
//! what it alone can know, which is whether the channel and the user exist.
//!
//! **A non-member's send is a `message.error`, not a 401**, and the two must not
//! be confused: the connection's authorization already succeeded, so ending it --
//! or telling the client its *session* was refused -- would be a statement about
//! something that was fine. The client keeps the optimistic row and marks it
//! failed, which is what `AGENTS.md` §8.1's Optimistic Send Flow requires.
//!
//! # What a refusal looks like, and why it is a `message.error`
//!
//! Every rejection is a [`sh_nexus_wire::ServerFrame::MessageError`] carrying a
//! machine-readable `code` and a human-readable `detail`. ADR-010 records why
//! that shape is worth the effort: an opaque code the user can report beats a
//! silently dropped explanation, and `PLAN.md` §7 is explicit that failures are
//! never silently dropped. The client's boundary turns `message.error` into
//! `DeliveryState::Failed` and keeps the row visible for retry
//! (`crates/sh_nexus/src/network/mapping.rs`), so a refused send is something the
//! user can see and act on rather than something that vanishes.
//!
//! A **bare `error` frame is not used for any of this.** `network/mapping.rs`
//! maps `ServerFrame::Error` onto `ConnectionState::Rejected`, i.e. the client
//! treats it as a statement about the connection. Sending one for a blank
//! `channel_id` would tell the client its connection had been rejected over a typo
//! in a field. `error` is reserved for what it is for: a statement about the
//! connection, such as the version rejection in `ws.rs` and the `401` on the
//! handshake.
//!
//! # Why `content` is never echoed
//!
//! `AGENTS.md` §7.5 forbids logging message content, and the rule extends to the
//! `detail` string that goes into a `message.error`, because that string is shown
//! to a user and lands in their client's log. So every `detail` here names the
//! *field* and the *rule*, never the value. The same reasoning, for the same
//! reason, applies to `client_msg_id`: it is peer-supplied and length-unbounded, so
//! logs carry its length. A `channel_id` is the exception that proves the rule --
//! it is server-issued, bounded, and the whole diagnostic -- so the one refusal
//! about it quotes it, and the tests say so.

use sh_nexus_wire::{ServerEnvelope, ServerFrame, WireMessage};

use crate::db::{AcceptOutcome, ChannelAccess};
use crate::error::ServerError;
use crate::hub::ConnectionId;
use crate::AppState;

/// `message.send` carried a `client_msg_id` that is empty or whitespace.
///
/// The field is *present* by construction -- `ClientEnvelope` requires it, so a
/// frame without one fails to decode -- which makes blankness a separate rule and
/// not a redundant one.
pub const BLANK_CLIENT_MSG_ID: &str = "blank_client_msg_id";

/// `message.send` named no channel.
pub const BLANK_CHANNEL_ID: &str = "blank_channel_id";

/// `message.send` carried no message body.
pub const EMPTY_CONTENT: &str = "empty_content";

/// `message.send` named a channel this instance does not have.
pub const UNKNOWN_CHANNEL: &str = "unknown_channel";

/// `message.send` named a channel the authenticated account is not a member of.
///
/// **The authorization refusal, and the only one in the message path.** It exists
/// because `PLAN.md` §6's `message.send` carries a `channel_id` and nothing else
/// about *where* the sender may write, and the socket does not either: one
/// connection carries sends for every channel the client can type into. So the
/// check belongs here, on the one frame that names a channel.
///
/// It is a `message.error` rather than a `401` on the handshake, and the reason is
/// in the module docs: the connection's authorization succeeded, so telling the
/// client its *session* was refused would be a statement about something that was
/// fine. It is also not [`UNKNOWN_CHANNEL`], because "you are not a member" and
/// "that channel does not exist" are different facts and a user who needs to ask
/// an administrator for access must be able to tell which one they hit.
pub const NOT_A_MEMBER: &str = "not_a_member";

/// The server could not persist the send.
///
/// Distinct from every code above on purpose: the four describe something the
/// sender did, this one describes something the server did. A user who sees
/// `storage_failure` should retry, not retype.
pub const STORAGE_FAILURE: &str = "storage_failure";

/// A send the server declined, with the reason it can report to a user.
///
/// `detail` is a `String` and not a `&'static str` because one refusal has to name
/// the value that broke the rule: `unknown_channel` quotes the channel id. That is
/// safe where a message body is not -- a channel id is server-issued, bounded text
/// with no free-form content in it -- and it is the whole diagnostic, because
/// "that channel does not exist" sent to a user who is looking at a sidebar leaves
/// them to guess which of eleven channels they got wrong.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Refusal {
    /// The machine-readable code, one of the constants in this module.
    pub code: &'static str,
    /// The human-readable reason, shown to the user.
    pub detail: String,
}

/// What accepting a send did, from the sender's point of view.
///
/// The sender is the only party that needs to be told, and it is told once: an
/// acknowledgement for a stored message, or a refusal with a reason. Delivery to
/// everybody else has already happened by the time this is returned.
#[derive(Debug, Clone)]
pub enum Outcome {
    /// The send was new. The message is the stored row.
    Accepted(WireMessage),
    /// The send had been accepted before under the same `client_msg_id`. The
    /// message is the row that already existed, and **nothing was broadcast**.
    Duplicate(WireMessage),
    /// The send was declined.
    Refused(Refusal),
}

impl Outcome {
    /// The frame this outcome is answered with, on the sender's socket.
    ///
    /// A single place that turns an outcome into a frame, so the mapping cannot
    /// differ between the ack path and the refusal path.
    pub fn into_envelope(self, client_msg_id: &str) -> ServerEnvelope {
        match self {
            Self::Accepted(message) | Self::Duplicate(message) => {
                ServerEnvelope::new(ServerFrame::MessageAck {
                    client_msg_id: client_msg_id.to_owned(),
                    message,
                })
            }
            Self::Refused(refusal) => ServerEnvelope::new(ServerFrame::MessageError {
                client_msg_id: client_msg_id.to_owned(),
                code: refusal.code.to_owned(),
                detail: refusal.detail,
            }),
        }
    }
}

/// Accepts one `message.send` and does everything that follows from it.
///
/// In order, because the order is the contract:
///
/// 1. **Validate.** A blank field is refused before any storage is touched, so a
///    refused send cannot leave a row, cannot move a channel's resume cursor, and
///    cannot be half-applied.
/// 2. **Authorize.** The account must be a member of the channel. Refused before
///    storage for the same reason, and separately from validation because it is a
///    different question with a different answer for the user.
/// 3. **Persist**, on a blocking thread. The store is synchronous
///    (`db.rs`'s module docs), and this is a tokio worker, so
///    `spawn_blocking` is where the call belongs. `AGENTS.md` §2.3's rule is that
///    blocking work does not run on a thread with something else to do.
/// 4. **Acknowledge the sender**, including for a duplicate. A replay after a
///    reconnect has to be answered or the client leaves the message pending
///    forever; `AGENTS.md` §8.1's Optimistic Send Flow requires the ack to be the
///    point where a message becomes real, and a client that reconnects mid-send
///    gets no other signal.
/// 5. **Broadcast, only for a new message.** This is the property `AGENTS.md`
///    §7.4 asks for and the one this milestone most needs to get right: a
///    replayed `message.send` reaching other clients twice would put the same
///    message in a transcript twice, and the receiving clients have no
///    `client_msg_id` dedup to catch it -- they never sent it.
///
/// The hub delivers to every subscriber including this one, and [`crate::ws`] drops
/// the origin's copy; see [`crate::hub::Delivery`] for why that split exists and
/// why the sender should not be echoed.
///
/// # Arguments
///
/// * `state` - the store and the hub. Cheap to pass by reference.
/// * `origin` - the sending connection, so the broadcast can skip its own echo.
/// * `user_id` - the authenticated account, from the handshake. **Not from the
///   frame**, which has no author field, and not from anything a peer supplied.
/// * `is_admin` - whether that account may create others. Carried for the
///   authorization step's log line; a message send never creates an account.
/// * `client_msg_id` - the envelope's id, as untrusted text.
/// * `channel_id` - the envelope's target channel.
/// * `content` - the envelope's body. Never logged.
///
/// # Returns
///
/// What the sender should be told. Delivery to other connections has already
/// happened.
pub async fn accept(
    state: &AppState,
    origin: ConnectionId,
    user_id: &str,
    is_admin: bool,
    client_msg_id: &str,
    channel_id: &str,
    content: &str,
) -> Outcome {
    if let Some(refusal) = validate(client_msg_id, channel_id, content) {
        return Outcome::Refused(refusal);
    }

    // Authorization is a *read*, so it does not need the write transaction
    // `accept_message` opens, and it is asked before that transaction exists --
    // which is what makes "a refused send leaves nothing behind" true of the
    // authorization refusal too and not only of the validation ones. One
    // `spawn_blocking` for both halves of the decision, because `db.rs` is the only
    // component that knows what a channel is.
    let store = state.store.clone();
    let owner = user_id.to_owned();
    let target = channel_id.to_owned();
    let access = tokio::task::spawn_blocking(move || store.channel_access(&owner, &target)).await;

    match access {
        Ok(Ok(ChannelAccess::Allowed)) => {}
        Ok(Ok(ChannelAccess::UnknownChannel)) => {
            tracing::info!(
                user_id = %user_id,
                channel_id = %channel_id,
                client_msg_id_len = client_msg_id.len(),
                "refused a send naming a channel that does not exist"
            );
            return Outcome::Refused(Refusal {
                code: UNKNOWN_CHANNEL,
                detail: format!("there is no channel with the id {channel_id:?} on this server"),
            });
        }
        Ok(Ok(ChannelAccess::NotAMember)) => {
            // The channel id is **not** quoted, unlike `unknown_channel`: the user
            // already has this channel in their sidebar, and `not_a_member` is a
            // statement they can act on without being told which id they typed.
            tracing::info!(
                user_id = %user_id,
                administrator = is_admin,
                channel_id_len = channel_id.len(),
                client_msg_id_len = client_msg_id.len(),
                "refused a send into a channel the sender is not a member of"
            );
            return Outcome::Refused(Refusal {
                code: NOT_A_MEMBER,
                detail: String::from(
                    "your account is not a member of that channel; ask an administrator \
                     for access",
                ),
            });
        }
        // A driver failure here is a server fault, not a peer's, and it lands in
        // the same arm as a storage failure -- but its `Display` is not discarded
        // into the log, because "the membership check could not run" and "the
        // database refused the message" have different fixes.
        Ok(Err(error)) => {
            tracing::error!(
                user_id = %user_id,
                channel_id_len = channel_id.len(),
                error = %error,
                "could not check channel access"
            );
            return Outcome::Refused(Refusal {
                code: STORAGE_FAILURE,
                detail: String::from("the server could not store this message; please retry"),
            });
        }
        Err(join_error) => {
            tracing::error!(
                user_id = %user_id,
                channel_id_len = channel_id.len(),
                error = %join_error,
                "the channel access check's storage task did not complete"
            );
            return Outcome::Refused(Refusal {
                code: STORAGE_FAILURE,
                detail: String::from("the server could not store this message; please retry"),
            });
        }
    }

    // `spawn_blocking` needs `'static` data, so the three untrusted strings and the
    // server-issued author are moved across. Cloning here is the cost of not
    // blocking a runtime worker, and it is bounded by the frame ceiling `ws.rs`
    // sets -- see that module on why this is the only content policy this
    // milestone has.
    let store = state.store.clone();
    let client_msg_id_owned = client_msg_id.to_owned();
    let channel_id_owned = channel_id.to_owned();
    let content_owned = content.to_owned();
    let author = user_id.to_owned();

    let stored = tokio::task::spawn_blocking(move || {
        store.accept_message(
            &client_msg_id_owned,
            &channel_id_owned,
            &author,
            &content_owned,
        )
    })
    .await;

    // `JoinError` means the blocking task panicked or was cancelled. It is a
    // server fault, not a peer's, and it lands in the same arm as a driver error
    // -- but its `Display` is not silently discarded into the log, because
    // "the task panicked" and "the database refused" have different fixes.
    let stored = match stored {
        Ok(result) => result,
        Err(join_error) => {
            tracing::error!(
                client_msg_id_len = client_msg_id.len(),
                channel_id_len = channel_id.len(),
                error = %join_error,
                "the send's storage task did not complete"
            );
            return Outcome::Refused(Refusal {
                code: STORAGE_FAILURE,
                detail: String::from("the server could not store this message; please retry"),
            });
        }
    };

    match stored {
        Ok(AcceptOutcome::Inserted(message)) => {
            let reached = state.hub.broadcast(
                origin,
                ServerEnvelope::new(ServerFrame::MessageNew {
                    message: message.clone(),
                }),
            );
            // The author id **by value**, and this is the one place in the server
            // where a user id reaches a log line. It is not a credential, it is
            // server-issued and bounded, and `AGENTS.md` §7.5 asks for ids. Content
            // is still only ever a byte count.
            tracing::info!(
                message_id = %message.id,
                user_id = %user_id,
                channel_id_len = channel_id.len(),
                content_bytes = content.len(),
                recipients = reached,
                "message accepted"
            );
            Outcome::Accepted(message)
        }
        Ok(AcceptOutcome::Duplicate(message)) => {
            // Logged at `info!` and not `warn!`: a replay is what `AGENTS.md` §7.4
            // says must work, so it is a normal event. The message id is logged by
            // value because this server minted it.
            tracing::info!(
                message_id = %message.id,
                client_msg_id_len = client_msg_id.len(),
                "duplicate send acknowledged without rebroadcast"
            );
            Outcome::Duplicate(message)
        }
        Err(ServerError::UnknownChannel { channel_id: named }) => {
            tracing::info!(
                channel_id = %named,
                client_msg_id_len = client_msg_id.len(),
                "refused a send naming a channel that does not exist"
            );
            Outcome::Refused(Refusal {
                code: UNKNOWN_CHANNEL,
                detail: format!("there is no channel with the id {named:?} on this server"),
            })
        }
        Err(error) => {
            tracing::error!(
                client_msg_id_len = client_msg_id.len(),
                channel_id_len = channel_id.len(),
                error = %error,
                "could not store a message"
            );
            Outcome::Refused(Refusal {
                code: STORAGE_FAILURE,
                detail: String::from("the server could not store this message; please retry"),
            })
        }
    }
}

/// The three rules a send has to satisfy, in the order they are checked.
///
/// The order is fixed and observable: `AGENTS.md` §2.1 asks for typed errors, and
/// a caller that fixed two fields at once gets one of them named rather than a
/// list. Blankness is `str::trim`, so a body of spaces is refused: `PLAN.md` §5
/// renders content as markdown, and a whitespace-only message renders as an empty
/// bubble, which is not something a user can have meant to send.
///
/// Returns `None` when the send is valid. A `&str` on the way in, so a blank
/// value costs no allocation to detect.
fn validate(client_msg_id: &str, channel_id: &str, content: &str) -> Option<Refusal> {
    if client_msg_id.trim().is_empty() {
        return Some(Refusal {
            code: BLANK_CLIENT_MSG_ID,
            detail: String::from(
                "this send carried no client_msg_id, so it cannot be deduplicated; \
                 the client's id generator did not run",
            ),
        });
    }
    if channel_id.trim().is_empty() {
        return Some(Refusal {
            code: BLANK_CHANNEL_ID,
            detail: String::from("channel_id must not be blank"),
        });
    }
    if content.trim().is_empty() {
        return Some(Refusal {
            code: EMPTY_CONTENT,
            detail: String::from("content must not be blank"),
        });
    }
    None
}
