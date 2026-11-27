# ADR-0004: No telemetry, no phone-home

English | [Deutsch](0004-no-telemetry-no-phone-home.de.md)

- **Status:** Accepted
- **Date:** 2026-11-27
- **Deciders:** Waldemar Friesen (product), RE engine team
- **Relates to:** [architecture.md](../architecture.md),
  [security.md](../security.md), [ki-integration.md](../ki-integration.md),
  [ADR-0002](0002-authoring-surfaces-and-ai-assistant.md) (assistant panel)

## Context

Commercial engines and editors routinely phone home: usage data, crash
reports and session beacons (session IDs, app IDs, versions) travel to
vendor endpoints unasked — Unreal Editor's log carries `datarouter` POSTs
to this day. This engine ships as a self-hosted MMO client and runs on
people's own hardware (down to a Raspberry Pi at home). Trust is a
feature in its own right. The product rule is blunt: **ask, don't
collect.** The software never learns anything about anyone on its own;
when an operator wants to know something about a person, they ask that
person, and the person decides whether to answer.

## Decision

1. **Zero phone-home.** Engine binaries (editor, client, tools) never
   open network connections on their own initiative. Outbound
   connections happen only toward endpoints a human explicitly supplied
   in this run (server address, project settings, CLI flags). Never
   allowed: telemetry/analytics, crash reporting, license or activation
   pings, update checks, presence beacons, install-unique
   identifiers/fingerprints.
2. **Offline is a supported state.** Editor and tools must be fully
   usable with no network at all; a missing network is never an error
   condition (the headless and `--ui-png` paths already prove it for
   rendering).
3. **External tools are their own business.** MCP adapters, AI
   providers and build services run as separate processes the user
   starts and configures. Whatever those processes do over the network
   is theirs — disclosed in the integration docs, invisible to the
   engine when they are not running. If the assistant panel (ADR-0002)
   connects a provider, the human supplies the endpoint and the
   credentials, and every outbound call is visible in the console.
4. **"Telemetry" in this repo means local metrics.** Wherever the word
   appears (e.g. `re-world` IO telemetry) it names local performance
   counters that feed streaming/quality decisions and land in logs —
   nothing leaves the machine. New counters follow the same rule.
5. **Dependency gate.** No HTTP/TLS/websocket/analytics client crates
   enter the engine's dependency graph; `scripts/check-phone-home.sh`
   fails the build if one appears. A future transport crate for real
   game networking (M6+) requires an explicit amendment to this ADR and
   stays bound to rule 1 (user-supplied endpoints only).
6. **Crashes write local artifacts.** Crash logs and minidumps land in
   the project workspace, where the human reads them and decides —
   sharing is the person's call, never automatic.

## Consequences

- The engine runs on a Pi with the network cable unplugged, and CI
  keeps the dependency graph honest.
- The audit checklist gains a per-milestone phone-home scan.
- Player-facing services (M6+) must implement knowledge as explicit
  per-item consent (a form, a visible field, an opt-in) — never as
  background collection.
