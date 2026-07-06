# Barnstormer Roadmap

Working dev roadmap. Last triaged: 2026-07-05.

**North star:** Barnstormer becomes the tool Harper reaches for on every new
project spec (**M1: Daily Driver**), then gets hardened so strangers can use it
safely (**M2: Public Beta**). An always-on code-health lane runs underneath both.

Re-triage this document when a milestone lands or when dogfooding teaches us
something. Each item becomes a GitHub issue when its turn comes up; work happens
branch → subagent development → PR to main.

Sources: [SPEC_GAPS.md](SPEC_GAPS.md) (2026-02-11, spot-checked 2026-07-05),
two-persona audit (2026-07-05), open GitHub issues.

---

## M1: Daily Driver

**Done when:** Harper specs three real projects start-to-finish in barnstormer
and the output feeds the actual dev loop (Claude Code / dippin) without manual
cleanup.

Themes are ordered so each unlocks the next: safe to run → useful output →
pleasant to use → smarter agents. The first three themes are small,
well-understood chunks; the spec-quality theme is fuzzy on purpose and needs
dogfooding feedback to tune against.

### Safe to dogfood (cost control)

- **M1.1** Hard ceiling on the swarm `run_loop` — step/token budget so a
  runaway session cannot burn money (`swarm.rs` run loop has no outer bound)
- **M1.2** Token/cost display in the UI, per spec session

### Output lands in the dev loop (workflow integration)

- **M1.3** Auto-write exports on events (SPEC gap 1.4) — keeps `exports/`
  always fresh on disk; resolves the orphaned "Export to Disk" button
  (issue #3)
- **M1.4** `barnstormer export <spec_id>` CLI subcommand, mirroring `import`
- **M1.5** Handoff CTA: when a spec stabilizes, a "hand off" action emits a
  coding-agent-ready pack (spec.md + plan seed). This is the piece that turns
  barnstormer output into dippin / Claude Code input — the daily-driver
  clincher.

### Daily use isn't annoying (UX friction)

- **M1.6** Edit spec core fields (title, one-liner, goal, constraints…) in the
  web UI (SPEC gap 1.1)
- **M1.7** Undo button in the UI (API endpoint exists, no template button) and
  fix the `apply_without_undo` catch-all in `state.rs` that can corrupt the
  undo stack for future event types
- **M1.8** Card `refs` editing in the card form (SPEC gap 1.14); spec
  search/filter in the left rail (SPEC gap 1.2)
- **M1.9** Wire up or remove the orphaned Artifacts tab (issue #4)

### Smarter specs (spec quality — continuous, fed by dogfooding)

- **M1.10** Enable the Critic agent (decide: default-on or per-spec toggle)
- **M1.11** Spec-quality eval loop: define "good enough to hand off," iterate
  agent prompts against the real projects spec'd during dogfooding
  (`scenarios.jsonl` is the seed)
- **M1.12** `read_recent_events` tool so agents can see what just changed
  (SPEC gap 1.9)

---

## M2: Public Beta

**Done when:** a stranger can install barnstormer, configure a provider through
the UI, build a spec, and hit no security holes or dead ends.

### Security (hard blockers for public — 2026-07-05 audit)

- **M2.1** Move `BARNSTORMER_ALLOW_REMOTE` validation to the real startup path
  (`RuntimeConfig::from_parts`); today the check lives only in dead
  `BarnstormerConfig::from_env`
- **M2.2** Auth middleware covers the web UI, not just `/api/*` — the HTML UI
  is currently never token-checked
- **M2.3** Constant-time auth token comparison
- **M2.4** `#[serde(other)]` on `EventPayload` so unknown event variants cannot
  abort log replay (forward-compat across upgrades)

### Onboarding (kill the dead ends)

- **M2.5** First-run setup flow in the web UI — the no-API-key state is
  currently a dead end; desktop already has a provider-settings bootstrap, web
  needs its equivalent
- **M2.6** Post-completion guidance: explain the DOT artifact, point to next
  steps (partially delivered by M1.5)
- **M2.7** Install-story polish: README + docs pass, close issue #11 (mux
  install instructions)

### Runs like a product (reliability)

- **M2.8** Periodic snapshot policy (event count and/or timer) plus snapshot on
  clean shutdown — today snapshots fire only on broadcast lag (SPEC gap 1.7)
- **M2.9** Agent-context persistence in snapshots — serialization exists, wire
  it up so agents resume with memory after restart (SPEC gap 1.6)
- **M2.10** `barnstormer stop` CLI + graceful shutdown endpoint (SPEC gap 1.11)
- **M2.11** Settings panel in the UI — read-only view of current config first;
  runtime-mutable only if dogfooding demands it (SPEC gap 1.3)
- **M2.12** *(stretch)* Windows/Linux Tauri builds — macOS is the proven path

---

## Always-on lane

Chip away between milestone items:

- Split the 8,158-line `crates/barnstormer-server/src/web/mod.rs`; move
  blocking `std::fs` calls out of async handlers
- Small open issues: #5 (DOT exporter test), #6 (slugify → core), #8 (tooltip
  rollout)
- Refresh `SPEC.md` to match reality (the "implemented differently" list in
  SPEC_GAPS section 3) and retire stale SPEC_GAPS entries as they close

## Deferred on purpose (YAGNI until proven otherwise)

- `ask_agent` direct agent-to-agent messaging — large, deadlock-prone, and
  indirect communication via shared state works (SPEC gap 1.8)
- Full-text search (SPEC gap 1.13) and SQLite runtime sync (SPEC gap 1.5) — no
  cross-spec query need yet
- Web-search tool for agents (issue #10) — revisit after M1.11 says whether
  spec quality actually needs it
