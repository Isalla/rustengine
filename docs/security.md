# Security — threat model, audit checklist, findings log

English | [Deutsch](security.de.md)

Purpose: hunt security problems in the engine **on purpose**, on a
schedule, instead of waiting for someone external to find them. Like
every doc here: the finding is written down before the fix, and the fix
references the finding ID.

## Threat model (engine-specific attack surfaces)

Ordered by real exposure, not by theory:

1. **Untrusted asset parsing — the #1 surface.** glTF now; FBX/OBJ/USD,
   images and cell files later (roadmap M4). A malformed or hostile
   file must never panic, allocate unboundedly or loop forever:
   size caps before allocation, `Result` not panics for caller errors,
   every parser gets a fuzz corpus.
2. **Network (M9).** Netcode service, interest management, launcher
   downloads: validate every packet (length, enum ranges, ordering),
   rate-limit per connection, treat the wire like the asset parsers —
   hostile input.
3. **AI/agent integration (M11, [ki-integration](ki-integration.md)).**
   Engine content is *data*, never instructions for the agent: prompt
   injection via asset names/descriptions must not reach the registry
   as tool calls. ADR-0002 already fixes the frame: read freely, write
   only on click. Budget caps and audit trails per agent run.
4. **Scripting & plugins (M11).** Wasm plugins sandboxed by default;
   native plugins are trusted-code and say so in the manifest. The
   capability registry is the permission boundary: a plugin gets tools
   by grant, not by right.
5. **Supply chain.** `Cargo.lock` committed; dependency additions are a
   review event (why this crate, what does it pull in); `cargo audit` /
   `cargo deny` in CI once the tooling is approved for this machine.
6. **Memory safety.** Safe Rust everywhere in `re-core`/`re-world`/
   `re-render`; `unsafe` only at backend FFI boundaries, each block
   with a `// SAFETY:` comment stating the invariant.
7. **Secrets & NDA.** Console SDKs live in private repos (M10); no
   tokens, paths of private material, or vendor content in this repo —
   the publication-clean rule is also a security rule.

## Audit checklist (run per quarter or per milestone)

- [ ] Fuzz the parsers: corpus of truncated, oversized, spec-violating
      glTF files; no panic, no OOM, no timeout (needs cargo-fuzz —
      ask before installing)
- [ ] `cargo audit` + `cargo deny` over the lockfile
- [ ] Phone-home scan (`scripts/check-phone-home.sh`): no HTTP/TLS/
      websocket/analytics clients in the dependency graph (ADR-0004)
- [ ] `unsafe` audit: every block has a SAFETY comment; count must not
      grow silently between milestones
- [ ] Capability registry: list every tool, check input validation and
      undo-safety per tool (double addressability cuts both ways — an
      agent can drive a bad state as fast as a human)
- [ ] Repo hygiene scan: no absolute machine paths, no private SDK
      names, no vendor assets (publication-clean)
- [ ] Network (once M9 exists): malformed-packet tests per message type

## Findings log

| ID | Date | Area | Severity | Summary | Status | Fix |
|----|------|------|----------|---------|--------|-----|
| — | — | — | — | no findings recorded yet | — | — |

Severity scale: `critical` (remote code execution / data loss) ·
`high` (crash from untrusted input, permission escape) · `medium`
(bounded DoS, info leak) · `low` (hygiene).

## Reporting

Unfixed vulnerabilities do **not** get public issues — use GitHub
Security Advisories (private). Fixed findings get a row in the table
above in the same commit as the fix, with a regression test.

*Source material lives in the private reference library (LLM security /
OWASP-LLM, defensive security ops) — outside this repo, see the library
index.*
