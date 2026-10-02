# Security Policy

## Reporting a vulnerability

**Report privately.** Do not open a public issue for a security problem.

GitHub's private vulnerability reporting is enabled for this repository:

> **https://github.com/Shoking01/Sh_Nexus/security/advisories/new**

That form opens a private advisory visible only to you and the maintainer. There
is no email address to write to and no separate process to follow — the link above
is the whole route.

If the form is unavailable, open a regular issue titled `security` with no
technical detail and a maintainer will move it into a private advisory. That
fallback is worse for you, not better; prefer the advisory.

## What is in scope

This is a native desktop chat client — a GPU-rendered UI, an in-memory state
layer, and a wire-protocol crate. In scope:

- **The client** — anything that executes on a user's machine: the GPUI
  application, the state layer, the markdown and cache layers.
- **The wire protocol crate** (`sh_nexus_wire`) — the bytes on the network and how
  they are parsed. A malformed frame that can be made to panic the client or read
  out of bounds is the clearest example.
- **Anything persisted on disk** — the SQLite database and any theme file, once
  those exist.
- **The build** — a compromised dependency, a build script doing something
  unexpected.

## What is not a vulnerability

Stated plainly so the distinction does not have to be argued:

- **Missing features.** The sidebar, search, attachments and thread replies are
  `PLAN.md` items, not defects. See [`PLAN.md`](PLAN.md).
- **Absence of features the plan defers.** Hot reload, a channel list, offline
  backfill — all deliberately later work, documented as such.
- **Performance below a documented budget.** `docs/BASELINES.md` records what was
  measured and what is still owed. A figure that does not meet its target is a
  known gap, not an attack.
- **A message rendered as plain text.** Markdown is parsed and styled; it is
  never executed. There is no HTML renderer and none is planned.
- **The deliberate absence of hardened branch protection requirements.**
  Required review counts are zero because this repository has exactly one
  maintainer. Setting it higher would block all work rather than protect it.

## What to expect

- **Acknowledgement within a few days.** This is a one-person project; it is not
  a staffed response desk.
- **No SLA.** Nothing here promises a fix window, and no commitment is made that a
  report will be actioned on a particular date.
- **Fixes land in `main` through a pull request**, which is the only way anything
  reaches this repository — enforced by branch protection.
- **Disclosure is the maintainer's call**, coordinated with you. There is no
  pre-arranged embargo process here, and pretending otherwise would be a promise
  that would not be kept.

## Automated analysis

This repository runs GitHub CodeQL (Rust and Actions) and GitHub's secret
scanning with push protection on every push, plus the CI suite from
`AGENTS.md` §5.1. Findings are triaged alongside manual review.

Automated analysis is a second pair of eyes, not the only one. It does not share
the blind spots of the person who wrote the code — which is the reason it runs.
