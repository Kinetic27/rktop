# Verification coverage

The project uses Rust unit tests plus a small integration/static contract test suite under `tests/`. Tests do not open live SSH connections or require remote credentials.

## Covered contracts

- Default/example config includes the expected host order and optional `game` behavior.
- Fixture data is generated from config and populates CPU, RAM, network, storage, disk, freshness, identity, group, and role fields.
- Mock snapshot output includes enabled host names and core metric labels.
- Config editing, init, setup guidance, doctor wiring, refresh controls, optional-host hiding, and SSH key-auth safety are covered by unit or contract tests.
- The fixed local/SSH collector emits CPU temperature when `coretemp`/`k10temp` exists, uptime, network counters, and multiple disk mounts while preferring ZFS pool summaries when available.
- Rendering contracts cover btop-like section rules, braille graphs for CPU/RAM/NET, disk block bars, grid-column-aligned disk mount labels, fixed-width disk capacity columns, minimal overview text, and terminal-size warnings.

## Manual verification used during development

```bash
cargo fmt --check
cargo check
cargo test
cargo build --release
scripts/install.sh --no-build
rktop --live --snapshot
```

Live SSH/manual checks are intentionally outside automated tests because they depend on the operator's homelab aliases and credentials. The automated contract suite instead verifies that live SSH monitoring stays read-only and non-interactive: it must use fixed collector commands, bounded SSH timeouts, `-n`, `BatchMode=yes`, password and keyboard-interactive auth disabled, one connection attempt, zero password prompts, and no setup/install/key-management commands. A fake-`ssh` doctor regression records runtime argv so the check does not require remote credentials.
