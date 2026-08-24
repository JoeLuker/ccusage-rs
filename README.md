# ccusage-rs

Fast, dependency-light Rust CLI that reports **Claude Code token usage and
costs** from the local JSONL transcripts Claude Code already writes. A
from-scratch sibling of the excellent TypeScript
[ccusage](https://github.com/ccusage/ccusage) — same data source, same
report shapes, single static binary, fully offline.

```
$ ccusage-rs daily --since 20260801
Period              Input        Output    Cache Create        Cache Read           Total  Cost (USD)
-----------------------------------------------------------------------------------------------------
2026-08-01        204,133        88,462       1,943,221        61,205,844      63,441,660      $41.87
2026-08-02        512,077       141,209       3,102,933        88,411,020      92,167,239      $63.14
-----------------------------------------------------------------------------------------------------
Total             716,210       229,671       5,046,154       149,616,864     155,608,899     $105.01
```

## Install

```sh
cargo install --git https://github.com/JoeLuker/ccusage-rs
# or from a clone:
cargo build --release   # -> target/release/ccusage-rs
```

## Usage

```sh
ccusage-rs                  # daily report (default)
ccusage-rs monthly
ccusage-rs session          # per-session, ordered by last activity
ccusage-rs daily --breakdown        # per-model rows under each period
ccusage-rs daily --since 2026-08-01 --until 2026-08-23
ccusage-rs daily --json
```

Transcripts are discovered under `<config-dir>/projects/**/*.jsonl`.
Config dirs: `$CLAUDE_CONFIG_DIR` (comma-separated list — multiple
accounts aggregate into one report), else `~/.claude` and
`~/.config/claude`.

## Correctness notes

These are the transcript quirks that make naive summing wrong; each was
verified against real data and is covered by tests:

- **Streamed duplicate writes.** The same API response is appended more
  than once (same `message.id` + `requestId`). Records are deduplicated —
  and the **last** write wins, because duplicates are not identical: a
  turn that is still accumulating server-side iterations logs pass 1
  first, then passes 1+2+…, so keeping the first write undercounts.
- **Multi-pass turns (`usage.iterations`).** For server-side tool
  iterations the top-level cache fields aggregate all passes, but
  top-level `input_tokens` / `output_tokens` reflect only the final pass.
  Billing is the per-iteration sum (observed: `input_tokens: 4` with an
  iteration sum of 142,478) — `ccusage-rs` sums the iterations.
- **Synthetic rows.** `model: "<synthetic>"` entries carry zeroed usage
  and are skipped.
- **Cache-write tiers priced exactly.** Anthropic bills 5-minute cache
  writes at 1.25× the input rate and 1-hour writes at 2×. Transcripts
  carry the `ephemeral_5m` / `ephemeral_1h` breakdown, and `ccusage-rs`
  prices each tier at its real rate. (This is the one deliberate
  divergence from the TypeScript ccusage, which prices all cache writes
  at 1.25× — on heavy 1h-cache workloads its cost figure is an
  underestimate. Token counts match it exactly: verified identical across
  all four token classes on a closed >100M-token day of real transcripts.)
- **Cost mode "auto".** A recorded `costUSD` on an entry is trusted
  as-is; otherwise cost is computed from tokens × a static price table
  (Anthropic first-party API rates, baked in — no network). Tokens from
  models missing in the table are counted and reported as unpriced,
  never silently priced at zero.

## Non-goals (for now)

5-hour billing-block reports, live watch mode, and per-project rollups.
The TypeScript ccusage does all of these well; this tool exists for the
"single static binary, no runtime, byte-verifiable" niche.

## License

MIT. Not affiliated with Anthropic or with the TypeScript ccusage
project (which deserves the credit for mapping this territory first).
