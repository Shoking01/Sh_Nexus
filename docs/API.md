# Sh_Nexus — API and Format Reference

Formats a client or a third party has to agree with. The authority for each
schema is `AGENTS.md`; this file records the schemas **as implemented**, and
`PLAN.md` §9 requires it to be updated whenever one changes.

| Section | Status |
|---|---|
| [1. Theme files](#1-theme-files) | **implemented** — work unit 1D, `core/theme.rs` |
| [2. Wire protocol and version negotiation](#2-wire-protocol-and-version-negotiation) | **not yet implemented** — `sh_nexus_wire` exists, the transport does not (`PLAN.md` Phase 4) |

Nothing here is a promise about a format that does not exist yet. Section 2 is
listed so its absence is visible rather than assumed.

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

**Not implemented.** `sh_nexus_wire` holds the frame and DTO types and is
`serde`-derived, but there is no transport, no envelope versioning and no
handshake: `PLAN.md` §2 places all of that in Phase 4, and `src/network/` today
holds only the DTO-to-domain mapping.

There is nothing to document yet, and this section exists so that its absence is
a recorded decision rather than an oversight. It is written when the transport
lands.
