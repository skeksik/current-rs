# Reference: the earlier Kotlin Multiplatform project

Current was first started in Kotlin Multiplatform (`skeksik/Current`, main at `66452b2` on 2026-10-22). It was replaced by this
repo (ADR 0001). The files here are copied unchanged from that project because they hold research that does not depend on the
language: server API facts checked against live servers and sources, playback and HDR research, the desktop video spike's measurements
and the decisions made there.

**Nothing here is a decision for this repo.**
- The UI rules in these files (Esc and Back, focus rings, keyboard handling, player layout) were written to make one Compose UI imitate
  every platform. Native UIs follow their platform instead (ADR 0001). A rule from here applies only once a decision record in
  `docs/decisions/` adopts it.
- ADR numbers, module names (`core:network`, `feature:player`), Koin, Gradle and "the app" refer to the Kotlin project.
- Facts carry the date they were checked on. Re-check anything a task depends on, and quote the new source in that task's notes.
- Server versions move: Jellyfin 12.2, Seerr 3.5.0 and Stash v0.31.1 were current in October 2026.

| File | What it holds |
|---|---|
| `servers-and-network.md` | Jellyfin 12.x, Seerr and Stash API facts, sign-in shapes, server identity, Seerr sign-in through Jellyfin, local discovery, local network and plain HTTP on Apple and Android, network change detection, roaming on a Mac |
| `playback.md` | Apple playback with a custom UI and system integration, desktop and web playback options, engine licences, stores and patents, HDR and Dolby Vision on Apple, Android, libmpv and Jellyfin's HLS remux |
| `desktop.md` | Desktop settings storage, secrets on desktop and in other apps, desktop packaging and the per-user installer run, single instance |
| `player-ux.md` | The player UX rules |
| `not-verified.md` | Claims that were still unverified |
| `decisions/` | ADRs 0007 (playback architecture), 0009 (licences), 0010 (desktop video spike and measurements), 0013 (core model), 0015 (server clients, errors, redaction), 0016 (saved servers and settings), 0017 (secrets and device identity), 0018 (Jellyfin sign-in), 0019 (roaming between server addresses) |
