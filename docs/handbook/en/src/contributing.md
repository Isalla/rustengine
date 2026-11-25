# Contributing

## Ground rules

- `cargo test --workspace` must stay green; every feature ships with
  tests, and `cargo doc` stays warning-free (CI enforces both).
- Small commits. A feature commit flips its roadmap checklist box —
  in **both** `docs/roadmap.md` and `docs/roadmap.de.md`.
- User-facing docs are bilingual: English is the plain name, German
  gets the `.de.md` twin. Code comments may be German.
- `re-world` stays headless and deterministic; threads belong in
  `io.rs` only.

## Doc style

Public items carry rustdoc with a one-line summary and a `# Panics` /
safety note where relevant — the website's rustdoc layer and the
editor's future API browser are generated from exactly those comments,
so a missing doc comment is a missing feature.

## Proposing changes

Open an issue before larger changes. The roadmap's milestone sections
mark what is planned; anything else needs a conversation first.

## License

MIT — Copyright (c) 2026 Waldemar Friesen. Contributions arrive under
the same terms; the copyright and license text must be carried
unchanged when redistributing.
