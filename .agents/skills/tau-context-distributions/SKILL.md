---
name: tau-context-distributions
description: Generate one zoomable combined role/model chart of content-free context, segment sizes and compactions; optional filters and date comparisons.
---

# Context and compaction distributions

Run the checked-in helper; do not research journals or export raw `tau-jsonl`.
It invokes the validated `agent-context-jsonl` projection once per journal and
feeds every chart and comparison from that scalar evidence. No providers or
running harnesses are contacted.

```sh
cd "$(jj workspace root)"
nix shell .#context-diagnostics -c tau-context-diagnostics \
  .agents/skills/tau-context-distributions/chart_context.py \
  --out tmp/context-last14days
```

The default captures UTC now once, then selects **agents created in the
trailing fourteen days `[since, until)`**, observed throughout their lifetime
before that endpoint. It does not round to midnight or clip lifetime totals to
the range. `--cohort active` instead selects agents with ordinary prompt starts
or accepted compactions in range, including their earlier lifetime through the
endpoint; warn that those agents have different ages.

Use `--tau /path/to/current/tau` if installed Tau lacks the projection.
`--agents-dir` defaults to `$XDG_STATE_HOME/tau/agents` or
`$HOME/.local/state/tau/agents`. Scanning all history is necessary: date bounds
do not reduce journal bytes scanned. `--timeout 120` is a finite per-journal
subprocess timeout, not a whole-report deadline. Temporary traces are anonymous
private files, deleted automatically; memory holds current-journal scalar
prompt/segment state and aggregate support counts, not raw traces or histories.
The Nix runner uses Python's standard library and gnuplot from `flake.lock`;
there are no downloaded Python dependencies or persistent diagnostic caches.

## What the three rows mean

1. **Accumulated context per agent:** sum of the observed ordinary-inference
   input-token high-water in each accepted-compaction-delimited segment.
2. **Individual context segments:** each segment's observed ordinary-input
   peak, counted once independently.
3. **Successful compactions per agent:** accepted `AgentCompacted` boundaries,
   including real zero counts.

Never sum overlapping prompt sizes. These are **observed request-input workload
proxies**, not unique accumulated text, final context lengths, billed tokens, or
context-window limits. Cached inputs are included. Output tokens and compact
request inputs are excluded; compact requests can cover a different cut.
Carried summaries and fixed system/tool material repeat across segments.
Additions never included in an ordinary request remain invisible.
Different models tokenize differently.

Prompt **start** owns its segment even if its terminal crosses a boundary.
Only evidence strictly before the endpoint is visible; late terminals and
boundaries do not backfill historical observations. Failed/cancelled compaction
attempts and accounting corrections do not split segments.

Unknown nonempty segments omit the strict agent sum, not the known individual
segments or valid compaction count. Partially measured segments retain their
observed peaks with partial-coverage counts. Empty fresh tails do not add zero
samples or invalidate measured totals. Measured open tails are included;
“open” means no later boundary as of the endpoint, not currently running.
Agents with no inference have missing size, not zero.

## Filters, breakdowns, comparisons

The default is **one combined role/model chart**, one time window, all selected observed
role/model pairs together. Role hues identify roles; model families use
line-only patterns and numeric versions use lightness (newest darkest). No
geometric point/marker symbols appear on curves or legend swatches. Compact independent
role and model legends sit side by side, not a legend entry per pair. The PNG
is **3520×3600** (2× each dimension of the previous 1760×1800 dense chart);
fonts/strokes stay normal-sized so readers can zoom. Do not paginate the
combined view merely for density or generate a model-only comparison as a
standard second image.
Joint `group` cells in CSV/`pages.txt` are JSON `[role, model]` arrays,
preserving exact custom names without ambiguous display separators.

Other explicit views use `--breakdown none|role|model|provider|role-model`.
Repeat `--role` for exact immutable creation-role filters. Model categories
use all ordinary models within **each metric unit**, normalized to the exact
suffix after the **first `/`**: one exact model name,
`Mixed models`, or `Unknown model`. Repeat `--model` to select categories
**after** complete structural aggregation. A mixed-model agent can still
contribute a single-model individual segment. No model-row filtering, alias
mapping or last-model attribution occurs. Different providers with the same
exact suffix are one model, not Mixed. **Mixed models are excluded by default
in model and role-model views**, after normalization. `--include-mixed` includes
them; `--model 'Mixed models'` explicitly selects them. Mixed per-agent totals
are never allocated to individual models, but single-model segments from those
agents remain eligible. Other breakdowns do not exclude mixed models by default.
`summary.json` records effective inclusion policy and per-metric included,
excluded-mixed and excluded-model-filter unit counts; `selected_agents` is the
role/time cohort before model exclusions. CSVs contain only included units.
Provider breakdown uses the set of
ordinary provider prefixes; mixed models on one provider remain that provider.

By default, include this **exact standard roster** only: `coordinator`,
`engineer-junior`, `engineer`, `engineer-senior`, `researcher`,
`researcher-senior`, `researcher-ultra`, `reviewer` (from the built-in role
configuration and `docs/examples/harness.yaml`). Do not prefix-match custom
roles. This applies to every breakdown and both cohort modes. Explicit
`--role custom-name` replaces the default roster and can select nonstandard
roles; `--all-roles` includes every role when no `--role` is given.
`--role` takes precedence over `--all-roles`. Report JSON/README state the
effective role scope; selected counts reflect that scope.

```sh
# Optional only when requested: same combined chart, two time-window columns:
nix shell .#context-diagnostics -c tau-context-diagnostics \
  .agents/skills/tau-context-distributions/chart_context.py \
  --tau target/debug/tau \
  --since 2026-09-10T12:00:00Z --until 2026-09-24T12:00:00Z \
  --compare-since 2026-09-24T12:00:00Z --compare-until 2026-10-08T12:00:00Z \
  --role engineer --role reviewer \
  --out tmp/context-model-comparison

# Select single-model units and mixed units explicitly; substitute an exact
# normalized model name from your summary.csv, not a provider prefix or alias:
nix shell .#context-diagnostics -c tau-context-diagnostics \
  .agents/skills/tau-context-distributions/chart_context.py \
  --model model-version --model 'Mixed models' --breakdown role-model \
  --cohort active --out tmp/context-active
```

Both comparison bounds are required. Bounds require explicit timezones,
positive ranges after the Unix epoch, at most 366 days each. Reproducibility
requires fixed endpoints and the same snapshot evidence; running checkpoints
can advance or lag writes. Snapshots across agents are not globally atomic.
For future side-by-side requests, use the normal combined role/model encoding
with A/B time windows as left/right columns on one graph. Do not generate a
comparison now unless requested; a model-only layout is an explicit alternate
breakdown, not the standard comparison.

## Artifacts and visual delivery

The output directory must be new; its parent must exist. It is owner-only.
Keep `distribution.csv`, `summary.csv`, `summary.json`, `README.md`, `pages.txt`,
`model-styles.json` for model views,
and every `context-page-NN.{gnuplot,svg,png}` together. Do not commit artifacts.
Export contains no agent IDs, paths, prompt/transaction IDs, content or raw
errors, but aggregate activity and exact roles/models remain sensitive.

Read `summary.json` for skipped coverage; `summary.csv` includes eligible/n/
missing/zero, open/partial segments, measured inputs and nearest-rank
p50/p90/p95/max. `n + missing = eligible`; coverage fields refer to underlying
evidence, so do not add them across different metrics. CSV model/role cells are
literal text; do not let spreadsheet software interpret them as formulas.

Charts use empirical CDF steps: share at or below x, ties jump by multiplicity.
Tokens use zero-preserving **`log10(1+x/10000)`** for wide ranges, labeled
“shifted log; offset 10k” with original-unit ticks. This gives 10k–1M most
of the width for typical context workloads without clipping smaller values,
zeros or >1M tails. Small (maximum ≤10k) or narrow ranges stay linear;
extreme tails necessarily occupy more space. Compactions use `log10(1+x)`
with original integer-count ticks and explicit zero; all-zero/binary ranges
stay linear. Every page and comparison range shares each metric's axis.
No epsilon zeros, smoothing, fabricated missing samples or clipping.
The combined view includes every selected observed pair on one page, retaining
three metric rows; it never invents a cross-product from independent role/model
aggregates. A role-only aggregate cannot reconstruct the pairing: regenerate
via the validated scalar projection if joint supports are unavailable.
Other breakdowns put at most ten series on each page. All pages/ranges share metric X
limits and report-wide colors. Exact labels survive in CSV and `pages.txt`
when long display labels are shortened. Empty panels explicitly say no
measured observations. Re-render with `gnuplot context-page-01.gnuplot` from
the output directory; the saved program embeds only aggregate support.
Nonlinear token ticks include intermediate original-unit values such as
10k, 20k, 50k, 100k, 200k, 500k and 1M where supported by the range.
Labels are collision-pruned using panel width, including comparison columns
and extreme-tail ranges; values between ticks remain fully plotted.

Related roles use **OKLCH family base hues** and sort
**junior → normal → senior → ultra**. Base hue degrees are coordinator **55**
(orange), engineer **260** (blue), researcher **150** (green), reviewer **310**
(purple). Seniority hue offsets are **−12°, 0°, +12°, +24°**, respectively:
junior/senior derive symmetrically around normal, ultra extends the senior side.
These are real hue changes, not shade-only substitutes. Lightness is
0.64/0.58/0.52/0.46 and initial chroma0.15; reduce chroma at fixed hue/lightness
when necessary to fit sRGB gamut, then emit sRGB hex. Custom families get a
deterministic base hue and the same offsets. No added color dependency.
Explicit model-only presentation groups the same name around its numeric version,
sorts versions numerically (e.g. sol 5.6 → 6 → 6.1), and uses related
light-to-dark shades. Provider prefixes never affect model grouping, styles,
filters or legends; different exact model names are not aliases.
Unknown/custom names use deterministic
fallback colors and ordering; suffixes junior/senior/ultra also work for custom
role families. Numeric version recognition is a presentation heuristic only.
In the combined view, color uses the **role hue plus model-version lightness**.
Within each role and versioned family, sort exact numeric tuples (5.6 < 6 < 6.1;
6.1 < 6.10), then evenly space OKLCH lightness offsets from **+0.09 oldest**
to **−0.09 newest** relative to that role's base lightness. Singleton/unversioned
families use zero offset. This preserves role/seniority hue offsets and guarantees
the newest numeric version is darkest for the same role; it does not infer
release chronology. Gamut reduction retains the requested hue and lightness.
All versions of a model family share one pattern: **Astra solid**, **Sol long
dashes `(10,4)`**. Other families receive distinct remaining dash/dot patterns.
Numeric family recognition is presentation only and never changes aggregate
identities or filters. No marker fallback. Role legend swatches show base
hues/lightness; grayscale model swatches show actual family patterns and
version brightness offsets. `model-styles.json` records exact family, dash and
lightness-offset mappings.
Resolve other-family pattern collisions deterministically over the observed
report-wide family inventory.
Patterns are consistent across roles/pages/comparison columns within a report.
Filtering away a conflicting family can change a resolved pattern; filtering
versions can change rank-based lightness offsets. Role hues and the explicit
Astra/Sol patterns remain filter-independent. Do not claim stronger style
stability or silently conflate different model names.

**Inspect the real rendered PNGs with `read_image` before delivery.**
Keep chart surfaces human-facing and minimal: metric titles, series legends,
original-unit ticks, concise scale notation and readable dates such as
“Sep 25 – Oct 9, 2026”. These dates are UTC date-only shorthand; exact instants
stay in report files. Do not put redundant percentage-axis prose, sample counts
(including legend n), missing/open/partial diagnostics, Selection/role-filter
prose, cohort/cutoff narratives or coverage/audit footers on the images.
Preserve all exact ranges, role scope, cohort definitions and coverage caveats
in CSV/JSON/README and the delivery manifest, not on the chart surface.

Confirm legible titles, labels and legends, shared axes/colors, zero mass,
honest empty panels and enough room for model labels. Deliver inspected
PNG previews plus source artifacts/coverage caveats. Do not claim a visual
pass if inspection was unavailable. Use a fresh senior researcher for substantial
presentation changes, and send actual real outputs rather than synthetic-only
previews.

**Deliver every page**, not just page 1: enumerate `pages.txt` against the PNG
files, inspect each, and attach the complete role/model sets with captions naming
the breakdown and page N/total. Include a manifest of exact page groups and a
report archive. When a role appears missing, check every page, exact role filters,
selected-cohort summary evidence and skipped coverage before claiming data loss.
Researchers/reviewers may simply be on later pages. Never broaden a cohort or
silently remove filters to make a requested role appear.
For authorized Zulip delivery, consult `tau-self-knowledge-ext-zulip` for the
attachment contract, bare artifact keys, sequential uploads/posts and uncertain-send
recovery. Do not duplicate those operational instructions here. Verify actual
tool confirmation before claiming delivery; export alone is not a sent message.
Implementation/review agents without publication authority hand the complete
inspected PNG, archive and manifest to the authorized sender.

Unsupported/corrupt/timed-out journals and terminals without proven start
ownership are skipped, never zero agents.
`summary.json` reports fixed skip categories, including checkpoint failures,
reader decode/framing failures and new projection rejections. Decode failures
remain **incompatible-or-malformed**, since human CLI errors cannot prove legacy
schema incompatibility versus corruption. Unknown failures are not guessed.
The broad failed/unsupported count contains those subcategories; do not add it
to them. All scan counters cover discovered history, not proven eligible agents.
The report coverage section preserves that distinction from per-metric missingness.
The conservative clock policy skips any journal with missing or regressing
append timestamps, including unrelated events, to avoid ambiguous cuts.
Supported typed journal schema is the count authority: legacy serde defaults
cannot independently prove raw serialized presence for every decoded zero.
Do not promise arbitrary legacy fidelity. See `docs/agent-trace.md`.

## Maintenance checks

```sh
nix shell .#context-diagnostics -c tau-context-diagnostics \
  -m unittest discover -s .agents/skills/tau-context-distributions -p 'test_*.py'
cargo nextest run -p dpc-tau-session-inspect -p dpc-tau-cli
cargo check --workspace --all-targets
treefmt
selfci check --candidate <change-id>
```

Tests cover segment arithmetic, ownership, missing/zero fidelity, historical
endpoints, filters, support ties and label escaping. Synthetic edge renders
are private test artifacts, not source data or committed report outputs.
