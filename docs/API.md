# Sh_Nexus — API and Format Reference

Formats a client or a third party has to agree with. The authority for each
schema is `AGENTS.md`; this file records the schemas **as implemented**, and
`PLAN.md` §9 requires it to be updated whenever one changes.

| Section | Status |
|---|---|
| [1. Theme files](#1-theme-files) | **implemented** — work unit 1D, `core/theme.rs` |
| [2. Wire protocol and version negotiation](#2-wire-protocol-and-version-negotiation) | **partly implemented** — the types and the version gate were always in `sh_nexus_wire`; the transport and the message path arrived with `sh_nexus_server`. §2.6 records exactly what is missing. |

Nothing here is a promise about a format that does not exist yet. §2.6 lists what
the server does not yet do, so its boundary is visible from the same page as its
behaviour.

---

## 1. Theme files

A theme is a **single shareable JSON file** (`AGENTS.md` §10.2). Sh_Nexus ships
three; a fourth is a file you write and drop in `~/.config/sh_nexus/themes/`.

### 1.1 A complete, valid theme

This is valid as written. Copy it and change the values.

```json
{
  "name": "Midnight Terminal",
  "author": "username",
  "version": 1,
  "colors": {
    "background": "#1e1e2e",
    "surface": "#313244",
    "sidebar": "#181825",
    "text": "#cdd6f4",
    "text_muted": "#7f849c",
    "accent": "#89b4fa",
    "accent_hover": "#b4befe",
    "danger": "#f38ba8",
    "success": "#a6e3a1",
    "mention": "#f9e2af",
    "code_block_bg": "#11111b",
    "bubble_self": "#45475a",
    "bubble_other": "#313244"
  },
  "spacing": { "xs": 4, "sm": 8, "md": 16, "lg": 24 },
  "radii": { "sm": 4, "md": 8, "lg": 16 },
  "typography": {
    "family": "Inter",
    "sizes": { "caption": 11, "timestamp": 10, "body": 14, "title": 16 }
  }
}
```

### 1.2 The rules, in the order a validator applies them

If you are debugging a rejected file, work down this list. The first rule that
your file breaks is the one that produced the message.

1. The file is **valid JSON**.
2. The file is at most **65,536 bytes** (64 KiB).
3. The root is a **JSON object**.
4. The root has **exactly** these seven keys: `name`, `author`, `version`,
   `colors`, `spacing`, `radii`, `typography`. No more, no fewer.
5. Each object has **exactly** the keys listed in §1.3 for it.
6. Each colour matches **`#rrggbb`** (§1.4).
7. Each number is an **integer** inside its documented range (§1.5).
8. `version` is exactly **1** (§1.6).
9. Each string is **not blank** and at most **128 characters** (§1.7).

### 1.3 The keys, and what each one is for

**Root** — all seven are required.

| Key | Type | Meaning |
|---|---|---|
| `name` | string | Shown in the theme picker. |
| `author` | string | Who wrote it. |
| `version` | integer | Theme **format** version. Always `1` today. |
| `colors` | object | Thirteen colours. See below. |
| `spacing` | object | Four spacing steps, in logical pixels. |
| `radii` | object | Three corner radii, in logical pixels. |
| `typography` | object | A font family and four size steps. |

**`colors`** — all thirteen required.

| Key | Drawn on |
|---|---|
| `background` | The window background, behind everything. |
| `surface` | Raised panels: the message list, cards, menus. |
| `sidebar` | The channel rail. |
| `text` | Body text, on `background` and on `surface`. |
| `text_muted` | Secondary text: timestamps, counts, placeholders. |
| `accent` | Interactive colour: focus rings, links, the send affordance. |
| `accent_hover` | `accent` under the pointer. Stated explicitly; the schema has no derived colours. |
| `danger` | Destructive actions, and the failed-send state. |
| `success` | Confirmations, presence, the delivered state. |
| `mention` | A message that names the reader. |
| `code_block_bg` | Behind a fenced code block. |
| `bubble_self` | Behind the reader's own messages. |
| `bubble_other` | Behind everyone else's messages. |

**`spacing`** — `xs`, `sm`, `md`, `lg`. Inside a control, between a control's
parts, between siblings, between groups.

**`radii`** — `sm`, `md`, `lg`. Small controls, cards and panels, dialogs.

**`typography`** — `family` (a string) and `sizes`, itself an object of
`caption`, `timestamp`, `body`, `title`.

> The JSON key is spelled `colors` because `AGENTS.md` §10.1 spells it that way.
> Prose in the source and in this file says "colour"; the key is the key.

### 1.4 Colour format

**`#` followed by exactly six hexadecimal digits. Nothing else.**

Accepted: `#1e1e2e`, `#1E1E2E` (either case; the app stores lower case), and
`#000000` — pure black is a real colour, not "unset".

Refused, and why:

| Refused | Reason |
|---|---|
| `1e1e2e` (no `#`) | The `#` is part of the format. |
| `#fff`, `#fff0` (shorthand) | Silently expanding it reinterprets your bytes. |
| `#1e1e2eff` (alpha) | The schema has no alpha slot; dropping it loses information invisibly. |
| `red`, `rgb(0,0,0)` | A colour name needs a name table the schema does not have. |
| `#gggggg` | Not hexadecimal. |
| `#1e 1e2e` | No whitespace, anywhere in the value. |
| `#+f1e2e`, `#-f1e2e` | A sign is not a hex digit. |

### 1.5 Numbers, and why each bound is that bound

The policy behind all of them: **reject what the renderer cannot recover from, and
do not enforce taste.** A validator that refused `spacing.xs = 2` because it
disliked the rhythm would be a validator nobody could satisfy.

| Field | Range | Reasoning |
|---|---|---|
| `spacing.*` | **0 – 256** | **0 is legal** — a deliberate flush layout is a real design decision, and forbidding it would forbid a legitimate theme. The ceiling is roughly a display: a spacing wider than a screen is a typo, not a scale. |
| `radii.*` | **0 – 512** | **0 is legal** and means square corners; the shipped high-contrast theme uses it. A *negative* radius describes no corner at all, so it is refused — twice over, by the range and by the integer type. The ceiling is arbitrary but bounded. |
| `typography.sizes.*` | **1 – 512** | **0 is refused, and this one is a design decision rather than a convention.** A zero font size has zero extent: the text cannot be measured, cannot be hit-tested, cannot be read — and unlike a zero spacing it is never anybody's design. **1** is the floor, not 12: the schema expresses no legibility policy, and inventing one would reject themes you can read perfectly well. |

All numbers must be **integers**. `14.0` is refused along with `14.5`: it is not a
different value, but it is evidence the author is writing numbers loosely.

### 1.6 `version`

`"version": 1` and nothing else. `0`, `2` and `-1` are each refused with a message
saying which version this build speaks.

A theme file is versioned so that a future release can change the format, and a
client that accepted an unknown version would be reading fields whose meaning had
already changed.

### 1.7 Strings

`name`, `author` and `typography.family` must each be:

- **not blank** — empty or whitespace-only is refused;
- at most **128 characters** — counted in characters, not bytes, so an accented
  label gets the full 128.

Control characters are *accepted* in a label and are escaped correctly when the
app re-emits the theme, so a round trip preserves them.

### 1.8 Unknown keys are an error

A key the schema does not define is **rejected**, not ignored. `"colour"` in place
of `"accent"` is the most likely hand-editing mistake there is, and the lenient
outcome — ignore it, use the default accent, look wrong with no explanation — is
precisely what the error message exists to prevent.

**What this costs, stated plainly:** the schema has no forward compatibility. A
theme carrying a key a given build has never heard of is rejected outright rather
than partially applied, so a future release that adds a key will not work on
older clients. That trade is deliberate: a rejected theme produces a message
naming the key, while a partially applied one looks like a rendering bug with no
symptom at all.

### 1.9 What a rejection looks like

Every message names the **exact JSON path** of the offending value. These are
real outputs, not illustrations:

```text
`accent` is not a key in this schema; expected one of: background, surface, sidebar, text,
  text_muted, accent, accent_hover, danger, success, mention, code_block_bg, bubble_self,
  bubble_other

`colors.accent` must be #rrggbb (six hex digits after `#`), found `#5b8defg`

`typography.sizes.body` is 0, outside the accepted range 1..=512

`name` is empty or whitespace only

`version` is 2; this build speaks theme format version 1

`<document>` must be an object, found an array
```

One rejection has no path, by necessity: a **JSON syntax error** has a line and
column rather than a path, and that is what the message carries.

**Behaviour on rejection:** the app keeps running on its default theme and shows
the message (`AGENTS.md` §10.2). It never falls back to a blank UI and never
crashes.

### 1.10 The three built-in themes

Shipped inside the binary, ids stable because they are written to a saved
preference:

| `id` | `name` |
|---|---|
| `dark` | Sh_Nexus Dark |
| `light` | Sh_Nexus Light |
| `high-contrast` | Sh_Nexus High Contrast |

Files: `crates/sh_nexus/themes/{dark,light,high-contrast}.json`.

### 1.11 Known limits of this schema

Two things a theme author may reasonably want are **not expressible in
`AGENTS.md` §10.1**, and neither is a bug in the validator:

- **No borders.** There is no border colour and no border width, so a theme cannot
  draw a 1px outline. The practical consequence lands on the high-contrast theme:
  with a pure black `background`, a message bubble has to be separated from the
  page by fill alone, and the WCAG 1.4.11 3:1 non-text contrast is not reachable
  for a bubble fill that still reads as a surface. Text-on-surface contrast in the
  shipped high-contrast theme exceeds 8:1 throughout, which is the half of the
  requirement a fill-only palette can meet.
- **No alpha.** Every colour is opaque. A translucent overlay cannot be expressed.

Both are schema defects to resolve by amending `AGENTS.md` §10.1 through the
process `docs/ARCHITECTURE.md`'s Appendix describes, **not** by adding a key
here — a version-1 schema change without a version bump is exactly what §1.6's
`version` field exists to prevent.

### 1.12 Checklist before you share a theme

- [ ] It parses as JSON, and the root is an object.
- [ ] Seven root keys, thirteen colours, four spacings, three radii, `family` plus
      four sizes.
- [ ] Every colour is `#rrggbb`, with no shorthand and no alpha.
- [ ] Every number is an integer in range; no font size is `0`.
- [ ] `"version": 1`.
- [ ] No key is spelled differently from §1.3 — `text_muted`, not `textMuted` or
      `text-muted`; `code_block_bg`, not `codeBlockBg`.
- [ ] `name` and `author` are not blank.
- [ ] It is under 64 KiB.

---

## 2. Wire protocol and version negotiation

The frames, DTOs and version gate are defined **once**, in `crates/sh_nexus_wire`,
and both sides compile against them (ADR-002). Nothing in this section is
re-specified here: `PLAN.md` §6 is the specification and
`crates/sh_nexus_wire/src/frame.rs` is its implementation. What follows is the
transport the frames now travel over, which `PLAN.md` §6 could not describe because
it did not exist when it was written.

### 2.1 The endpoint

Two kinds of route: one WebSocket, and the HTTP routes §3 adds for authentication.

```
GET /ws HTTP/1.1
Upgrade: websocket
Connection: Upgrade
Sec-WebSocket-Key: <base64 of 16 bytes>
Sec-WebSocket-Version: 13
Authorization: Bearer <session token>
```

**The `Authorization` header is required.** A handshake without a valid one is
refused with **`401` before the upgrade**, so it costs no subscription, no task and
no row — see §3.2. The upgrade response for an authenticated peer is axum's `101`.
**No subprotocol is negotiated** and no `Sec-WebSocket-Protocol` is echoed, because
every frame carries its own `v` and the version lives on the envelope rather than in
a handshake header — which is the decision `sh_nexus_wire`'s frame module documents
and the reason there is no negotiation step at all.

The server's configuration is four environment variables, all optional:

| Variable | Default | Meaning |
|---|---|---|
| `SH_NEXUS_BIND` | `127.0.0.1:8484` | The socket address to bind, as `host:port`. A malformed value is refused at startup, naming the variable — never silently replaced by the default. |
| `SH_NEXUS_DB` | `sh_nexus.db` | The SQLite file. The parent directory must already exist. |
| `SH_NEXUS_ADMIN_USERNAME` | — | The login handle for the instance's first administrator. Read **only** when no account can log in. |
| `SH_NEXUS_ADMIN_PASSWORD` | — | That account's password. **Never logged, never echoed, and never required on a second start.** |

The bind default is **loopback**, not `0.0.0.0`, and it stayed loopback when
authentication arrived: a `401` is not encryption, and a self-hosted instance exposed
to a network without TLS has its bearer tokens readable on the wire. The loopback
default is what makes the current state defensible, and an operator who wants it
exposed has to say so **and** terminate TLS in front of it. ADR-010 records the bind
interface as a decision it does **not** make, which is why the startup log names
the interface actually bound rather than the one that was configured.

### 2.2 The message round trip

This is the one path the server implements. Everything else is §2.6.

```text
client -> server   {"v":1,"type":"message.send","client_msg_id":"…","channel_id":"c_general","content":"hi"}
server -> sender   {"v":1,"type":"message.ack","client_msg_id":"…","message":{…}}
server -> others   {"v":1,"type":"message.new","message":{…}}
```

Three properties a client may rely on, each of which is enforced by the database
rather than by the code around it:

1. **A send is idempotent under `client_msg_id`.** A replay — after a reconnect,
   from a retrying client — creates no second row and produces no second
   `message.new`. The `message.ack` is sent again, carrying the row that already
   existed, so a client that reconnects mid-send still reconciles.
2. **The ack is the authority.** It carries the stored `WireMessage`, not the one
   that was sent. The sender's own `message.new` is **not** echoed to it: the ack
   already gave it the row, and the echo's only consumer would be the client's own
   dedup. Every *other* connection receives the `message.new`.
3. **`last_message_at` advances only for a new message.** It is the upper bound a
   `resync` asks for, and a replay moving it would make a client that resumed from
   it skip the message the replay was about.

### 2.3 Refusals

Every rejection is a `message.error` carrying a machine-readable `code` and a
human-readable `detail`, and the connection **stays open** — `PLAN.md` §7 is
explicit that failures are never silently dropped, and the client's boundary turns
`message.error` into `DeliveryState::Failed` with the row still visible for retry.

| `code` | `detail` | Cause |
|---|---|---|
| `blank_client_msg_id` | names the missing `client_msg_id` and that it cannot be deduplicated | the envelope's id is present but blank |
| `blank_channel_id` | `channel_id must not be blank` | whitespace-only channel |
| `empty_content` | `content must not be blank` | whitespace-only body |
| `unknown_channel` | quotes the channel id | no such channel on this instance |
| `storage_failure` | `the server could not store this message; please retry` | the server's fault, not the sender's |

`code` is a `String` on the wire and not an enum, so a client too old to know a
code can still read the frame that explains the failure. **No `detail` ever quotes
message content** — `AGENTS.md` §7.5 forbids logging it, and a `detail` is shown to
a user and lands in their client's log. A `channel_id` is the one value that is
quoted, because it is server-issued bounded text and naming it is the whole
diagnostic.

A **bare `error` frame is not used for any of the above.** `error` is a statement
about the connection; the client's boundary maps it to `ConnectionState::Rejected`,
so using it for a blank field would tell a client its connection had been rejected
over a typo.

### 2.4 Version negotiation

A peer whose `v` is not in `SUPPORTED_MAJOR_VERSIONS` gets, in this order:

1. an `error` frame with `code: "unsupported_version"` and a `detail` naming both
   the version it announced and the set this build speaks — and stating that
   reconnecting will not resolve it, because a version mismatch is not transient;
2. a close frame, code **1002** (protocol error).

The rejection carries **this build's** `v`, never the peer's, so a client too old
to read the frame can still decode it with the version it already has. The
rejection's `detail` is `UnsupportedVersion::detail()` verbatim — the server does
not write its own sentence, and `tests/version_negotiation.rs` asserts the exact
string for that reason.

Every *other* unreadable frame — malformed JSON, an unknown `type`, a server frame
arriving on a client socket — is dropped with a `warn!` naming the error kind and
the frame's size, and **the connection stays open**. That is
`sh_nexus_wire`'s own compatibility policy: a frame this build cannot decode is a
frame it cannot render, and within one major version the only such frames are
advisory ones whose loss costs nothing.

**The frame size ceiling is 65,536 bytes.** A larger frame ends the connection
without a close frame, because `tungstenite` 0.29 refuses it with
`Error::Capacity(MessageTooLong)` and terminates; RFC 6455 §7.4.1's 1009 is
therefore **not** sent, and the peer observes an unclean close (an RST on Windows).
This is a transport limit, not a content policy: a per-message length limit belongs
to the milestone that has accounts to attribute one to.

### 2.5 What the server stores

SQLite, WAL, one file, **schema version 2** in `PRAGMA user_version`. Version 2 is
this milestone's migration and is additive; §3.1 names the two tables it adds.

| Table | Exercised | Note |
|---|---|---|
| `users` | yes | **A real account, authenticated.** `password_hash` is an Argon2id hash; `is_admin` is what "administrator" means, rather than creation order — inferring it from the earliest `created_at` would make a privilege grant depend on a tie-break, and a tie broken by id is a privilege decided by a UUID. The reserved `u_unattributed` row is **retained**: ADR-010 keeps authorship on removal, and a database migrated from schema 1 has message rows pointing at it under a foreign key. It carries no `password_hash`, so it can never log in. |
| `channels` | one seeded row | `c_general`. A send naming any other channel is **refused**, not silently created — channel provisioning is a later milestone. |
| `messages` | the whole round trip | `client_msg_id` is globally `UNIQUE`, which is what makes dedupe a single lookup. `user_id` is now the authenticated account, never a frame-supplied value. |
| `read_cursors` | **no** | ADR-010 requires per-`(user, channel)` read state "from the first migration". Deliberately unread here. |
| `channel_members` | yes | Checked on the send. A non-member is refused with `not_a_member`; the socket stays open, because one socket serves every channel the client can type into. |

Opening a file whose `user_version` is above this build's, or that has tables but
no version stamp, is **refused** rather than migrated. Both refusals are
`AGENTS.md`'s "never destroy user data silently" with teeth, and both are tested.

### 2.6 What is not implemented

Named so its absence is a recorded decision rather than a discovery. All of these
decode and are then **logged at `warn!` with the connection kept open**, which is
`sh_nexus_wire`'s own prescription for a frame this end does not implement.

| Missing | Consequence for a client |
|---|---|
| Authentication, registration, tokens | `message.send` has no author; every message is `u_unattributed`. |
| REST, channel and user provisioning | Only `c_general` exists. |
| Presence | No `presence.update` is ever sent. |
| Typing | `typing.start`/`typing.stop` are accepted and discarded. No `typing.update` is ever sent. |
| Reactions | `reaction.add` is accepted and discarded. No `reaction.update` is ever sent. |
| `resync` | **A reconnecting client that asks to catch up is not sent what it missed.** This is the one gap in the list with teeth, and it is data-bearing. It deserves a frame that says so; that frame belongs to the milestone that implements resync, because inventing a code now would be a protocol decision taken before anything can send it. |

### 2.7 Frame types the server does not send

`message.ack`, `message.new` and `message.error`, plus the `error` frame of §2.4.
`ServerFrame`'s remaining four variants — `reaction.update`, `typing.update`,
`presence.update` and any future one — are defined in `sh_nexus_wire` and
unreachable from this build, which is a property the type system holds rather than
a convention.

## 3. Authentication

Added with the server's auth milestone. ADR-010 decided the shape; this section is
what actually got built.

### 3.1 Why opaque tokens and not a JWT

`PLAN.md` assumed `jsonwebtoken`. **This is not that.** Two reasons, one practical and
one that would have been a defect:

- **A JWT cannot be revoked.** In a per-team chat, expelling a member must kill their
  session immediately. A signature that is valid until it expires means the only way
  to stop it is to wait. The revocation column exists here for that reason.
- **A JWT needs a crypto backend.** `ring` and `rustls` are not anywhere in this
  workspace's tree, so a JWT would have dragged a signature-verification stack into a
  project with no crypto dependency at all, on a crate the client never links.

Tokens are opaque and random, and **only their hash is stored** — `sha2` of the token,
as the `sessions` primary key. A stolen database file therefore yields no usable
session. `argon2` is used for passwords only, where slowness is the feature.

The **client** holds no crypto dependency and never hashes or mints anything: it
sends the token it was given.

### 3.2 The 401 happens before the upgrade

An unauthenticated handshake is refused at `authenticate`, which runs **before**
`WebSocketUpgrade` is consumed. So a refused peer costs **no subscription, no task and
no row** in the connection count — not merely a closed socket. `ws.rs` calls
`hub.connect()` only after the identity resolves, and that ordering is asserted
twice in `tests/auth.rs`: the status line reads `401` and never `101`, and
`connection_count()` does not move.

One deliberate placement decision: **"this user may not post in this channel" is
enforced on the send, not the handshake.** One socket serves every channel a client
can type into, so a handshake has no channel to be refused *for*. A non-member's send
is a `message.error` with code `not_a_member` and the socket stays open. An unknown
channel is a different refusal — `unknown_channel` — so a typo never reads as "ask an
administrator".

### 3.3 Registration is closed

There is no `POST /auth/register`. An administrator creates accounts. The **first**
administrator is bootstrapped from `SH_NEXUS_ADMIN_USERNAME` /
`SH_NEXUS_ADMIN_PASSWORD`, and only when the instance has no account that can log in.
A second start with an account present re-reads nothing and overwrites nothing.

"Cannot log in" means `password_hash IS NOT NULL`, not "a row exists in `users`" —
because a database migrated from schema 1 holds the reserved historical-author row,
and counting rows would refuse to bootstrap exactly the instances that most need it.

### 3.4 What is deliberately not here

- **No keychain.** The client accepts a token and loses it at process exit. `platform/`
  does not exist yet, so nothing writes a token to disk — and saying so is better than
  a plaintext file that a later milestone has to unpick.
- **No TLS.** Hence the loopback bind default in §2.1.
- **No rate limiting**, which ADR-010 explicitly does not decide.
- **Revocation does not kill a live socket.** The next handshake is refused; `PLAN.md`
  §6 has no frame for terminating an established connection.
- **A "revoke every session for this user" operation** is not built. The mechanism
  (`sessions.revoked_at_unix_ms`) is.
