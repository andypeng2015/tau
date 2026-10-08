#!/usr/bin/env python3
"""Content-free context ECDFs from Tau's validated, finite scalar projection.

Only the current journal's prompt/segment scalar state and aggregate support
counts survive parsing. No journal content or correlation identities are exported.
"""

import argparse
import collections
import colorsys
import csv
import datetime as dt
import json
import math
import os
from pathlib import Path
import subprocess
import tempfile
import textwrap

DAY = 86_400_000_000
U64 = 2**64 - 1
METRICS = ("accumulated", "segments", "compactions")
TITLES = (
    "Accumulated context per agent",
    "Individual context segments",
    "Successful compactions per agent",
)
UNITS = ("Sum of observed segment peaks", "Observed ordinary-input peak", "Accepted boundaries")
MIXED = "Mixed models"
UNKNOWN = "Unknown model"


def checked(value):
    """Keep arithmetic in the journal's unsigned token/count domain."""
    if type(value) is not int or not 0 <= value <= U64:
        raise ValueError("invalid or overflowing scalar count")
    return value


def instant(text):
    """Parse an explicitly zoned ISO timestamp without floating-point rounding."""
    parsed = dt.datetime.fromisoformat(text.replace("Z", "+00:00"))
    if parsed.tzinfo is None:
        raise ValueError("timestamps require a UTC offset")
    delta = parsed.astimezone(dt.timezone.utc) - dt.datetime(1970, 1, 1, tzinfo=dt.timezone.utc)
    return delta.days * DAY + delta.seconds * 1_000_000 + delta.microseconds


def iso(value):
    return (dt.datetime(1970, 1, 1, tzinfo=dt.timezone.utc)
            + dt.timedelta(microseconds=value)).isoformat().replace("+00:00", "Z")


def select_range(since, until, now):
    end = instant(until) if until else now
    begin = instant(since) if since else end - 14 * DAY
    if begin < 0 or not begin < end or end - begin > 366 * DAY:
        raise ValueError("ranges must be after epoch, positive and at most 366 days")
    return begin, end


class Segment:
    """One accepted-compaction-delimited lifetime, owned by prompt-start order."""

    def __init__(self):
        self.models = set()
        self.starts = 0
        self.measured = 0
        self.peak = None
        self.open = True


class Agent:
    """Scalar reconstruction through one endpoint, independent of other ranges."""

    def __init__(self, created, role, bounds):
        self.created = created
        self.role = role
        self.bounds = bounds
        self.segments = [Segment()]
        self.prompts = {}
        self.compactions = 0
        self.compact_measured = 0
        self.active = False

    def observe(self, row):
        at = checked(row["recorded_at_unix_micros"])
        if not at or at >= self.bounds[1]:
            return
        kind = row["record_type"]
        if kind in ("inference_started", "compacted") and at >= self.bounds[0]:
            self.active = True
        if kind == "inference_started":
            prompt = row["prompt_id"]
            if prompt in self.prompts:
                raise ValueError("duplicate prompt")
            segment = self.segments[-1]
            segment.starts = checked(segment.starts + 1)
            model = row["model"]
            if not isinstance(model, str) or "/" not in model:
                raise ValueError("noncanonical model")
            segment.models.add(model)
            self.prompts[prompt] = (segment, False)
        elif kind == "inference_finished":
            prompt = row["prompt_id"]
            if prompt not in self.prompts:
                raise ValueError("terminal without selected start")
            segment, finished = self.prompts[prompt]
            if finished:
                raise ValueError("duplicate terminal")
            self.prompts[prompt] = (segment, True)
            if row.get("input_tokens") is not None:
                value = checked(row["input_tokens"])
                segment.measured = checked(segment.measured + 1)
                segment.peak = value if segment.peak is None else max(segment.peak, value)
        elif kind == "compacted":
            self.segments[-1].open = False
            self.segments.append(Segment())
            self.compactions = checked(self.compactions + 1)
            if row.get("compact_input_tokens") is not None:
                checked(row["compact_input_tokens"])
                self.compact_measured = checked(self.compact_measured + 1)

    def selected(self, cohort):
        return (self.bounds[0] <= self.created < self.bounds[1]
                if cohort == "created" else self.created < self.bounds[1] and self.active)


def read_trace(stream, ranges):
    """Read once; reject incomplete/ambiguous clocks instead of inventing history."""
    header = json.loads(next(stream))
    if (header.get("schema") != "tau.agent_context" or header.get("schema_version") != 0
            or header.get("content_included") is not False or header.get("record_type") != "header"):
        raise ValueError("unsupported context projection")
    agents = None
    owner = None
    last_seq = -1
    last_time = 0
    transactions = set()
    summarized = False
    for line in stream:
        row = json.loads(line)
        kind = row["record_type"]
        if kind == "journal_summary":
            if agents is None or row["agent_id"] != owner or summarized:
                raise ValueError("invalid journal summary")
            if checked(row["clock_regressions"]):
                raise ValueError("regressing_clock")
            if checked(row["missing_timestamps"]):
                raise ValueError("missing_clock")
            summarized = True
            continue
        if summarized:
            raise ValueError("root-only projection required")
        seq = checked(row["seq"])
        at = checked(row["recorded_at_unix_micros"])
        if seq <= last_seq or not at or at < last_time:
            raise ValueError("invalid evidence order or clock")
        last_seq, last_time = seq, at
        if kind == "agent_created":
            if agents is not None or seq != 0 or not isinstance(row["role"], str):
                raise ValueError("invalid creation")
            owner = row["agent_id"]
            agents = [Agent(at, row["role"], bounds) for bounds in ranges]
        elif agents is None or row["agent_id"] != owner:
            raise ValueError("missing creation or mismatched owner")
        elif kind == "compacted":
            identity = row["transaction_id"]
            if identity in transactions:
                raise ValueError("duplicate compaction")
            transactions.add(identity)
        elif kind not in ("inference_started", "inference_finished"):
            raise ValueError("unsupported evidence record")
        for agent in agents:
            agent.observe(row)
    if not summarized:
        raise ValueError("missing journal summary")
    return agents


def model_category(models):
    return next(iter(models)) if len(models) == 1 else MIXED if models else UNKNOWN


def group_for(role, models, breakdown):
    model = model_category(models)
    if breakdown == "none":
        return "All selected"
    if breakdown == "role":
        return role
    if breakdown == "model":
        return model
    if breakdown == "provider":
        providers = {model.split("/", 1)[0] for model in models}
        return next(iter(providers)) if len(providers) == 1 else "Mixed providers" if providers else "Unknown provider"
    return role + " | " + model


class Support:
    """Lossless aggregate support and coverage; no per-agent identifiers."""

    def __init__(self):
        self.values = collections.Counter()
        self.eligible = 0
        self.missing = 0
        self.open = 0
        self.partial = 0
        self.starts = 0
        self.measured = 0
        self.empty_tails = 0
        self.compact_measured = 0

    def add(self, value, segments, empty_tails=0, compact_measured=0):
        self.eligible = checked(self.eligible + 1)
        if value is None:
            self.missing = checked(self.missing + 1)
        else:
            self.values[value] = checked(self.values[value] + 1)
        self.open += sum(segment.open for segment in segments)
        self.partial += int(any(segment.measured < segment.starts for segment in segments))
        self.starts += sum(segment.starts for segment in segments)
        self.measured += sum(segment.measured for segment in segments)
        self.empty_tails += empty_tails
        self.compact_measured += compact_measured
        for count in (self.open, self.partial, self.starts, self.measured, self.empty_tails, self.compact_measured):
            checked(count)

    @property
    def n(self):
        return checked(sum(self.values.values()))

    def quantile(self, share):
        if not self.n:
            return None
        rank = math.ceil(self.n * share)
        seen = 0
        for value, count in sorted(self.values.items()):
            seen += count
            if seen >= rank:
                return value
        raise AssertionError("unreachable quantile")


def aggregate(agents, args, supports, counts):
    for index, agent in enumerate(agents):
        if not agent.selected(args.cohort) or args.role and agent.role not in args.role:
            continue
        counts[f"{chr(65 + index)}_selected_agents"] += 1
        segments = [segment for segment in agent.segments if segment.starts]
        models = set().union(*(segment.models for segment in segments))
        empty = sum(not segment.starts for segment in agent.segments)
        total = (checked(sum(segment.peak for segment in segments))
                 if segments and all(segment.peak is not None for segment in segments) else None)
        units = [("accumulated", total, models, segments, empty, 0),
                 ("compactions", agent.compactions, models, segments, empty, agent.compact_measured)]
        units.extend(("segments", segment.peak, segment.models, [segment], 0, 0)
                     for segment in segments)
        for metric, value, unit_models, evidence, tails, compact_measured in units:
            if args.model and model_category(unit_models) not in args.model:
                continue
            group = group_for(agent.role, unit_models, args.breakdown)
            supports.setdefault((index, metric, group), Support()).add(
                value, evidence, tails, compact_measured)


def trace_failure_category(stderr):
    """Classify bounded diagnostics into fixed labels, never export raw errors.

    The CLI has human-readable errors, not structured failure codes. Decode
    failures cannot distinguish incompatible legacy shapes from malformed CBOR;
    keep that uncertainty explicit instead of declaring every failure corrupt.
    """
    text = stderr.decode("utf-8", errors="replace")
    if "ambiguous context evidence: terminal without start" in text:
        return "unowned_terminal_journals"
    if "ambiguous context evidence:" in text:
        return "context_projection_rejected_journals"
    if "unsupported agent checkpoint schema" in text:
        return "unsupported_checkpoint_journals"
    if "failed to read agent store" in text and "meta.json:" in text:
        return "checkpoint_unavailable_or_invalid_journals"
    if "failed to decode agent store record" in text:
        return "decode_incompatible_or_malformed_journals"
    if any(phrase in text for phrase in (
            "invalid agent event sequence", "snapshot boundary ends inside a record",
            "journal ended before the selected snapshot boundary", "agent store record for")):
        return "framing_or_sequence_journals"
    if "agent journal not found at" in text:
        return "missing_journal_artifact_journals"
    if "failed to open agent store" in text or "failed to read agent store" in text:
        return "io_or_snapshot_journals"
    return "other_trace_failure_journals"


def scan(args, ranges, counts):
    """Discover once, project each journal once, feed both endpoints from one file."""
    supports = {}
    for entry in sorted(args.agents_dir.iterdir()):
        if not entry.is_dir() or not (entry / "events.cbor").is_file():
            continue
        counts["scanned_journals"] += 1
        with tempfile.TemporaryFile(mode="w+", encoding="utf-8") as trace, tempfile.TemporaryFile() as errors:
            try:
                result = subprocess.run(
                    [str(args.tau), "agent", "trace", entry.name,
                     "--format", "agent-context-jsonl", "--agents-dir", str(args.agents_dir)],
                    stdout=trace, stderr=errors, timeout=args.timeout, check=False)
            except subprocess.TimeoutExpired:
                counts["timed_out_journals"] += 1
                continue
            if result.returncode:
                # Retain the broad total and disjoint fixed diagnostic categories.
                # Bounded text is transient; no raw stderr/paths enter artifacts.
                counts["failed_or_unsupported_journals"] += 1
                errors.seek(0)
                counts[trace_failure_category(errors.read(65_536))] += 1
                continue
            trace.seek(0)
            try:
                agents = read_trace(trace, ranges)
                # Validate arithmetic before mutating shared report aggregates.
                local_support, local_counts = {}, collections.Counter()
                aggregate(agents, args, local_support, local_counts)
            except (ValueError, KeyError, TypeError, StopIteration) as error:
                reason = str(error) if str(error) in ("regressing_clock", "missing_clock") else "invalid_projection"
                counts[reason + "_journals"] += 1
                continue
            counts["validated_journals"] += 1
            counts.update(local_counts)
            for key, source in local_support.items():
                target = supports.setdefault(key, Support())
                target.values.update(source.values)
                for field in ("eligible", "missing", "open", "partial", "starts",
                              "measured", "empty_tails", "compact_measured"):
                    setattr(target, field, checked(getattr(target, field) + getattr(source, field)))
                checked(target.n)
    return supports


def gp(text):
    """Quote literal labels, including gnuplot's in-string command substitution."""
    return '"' + text.replace("\\", "\\\\").replace('"', '\\"').replace("`", "\\`").replace("\n", "\\n").replace("\r", " ").replace("\t", " ") + '"'


def ticks(maximum, transformed):
    if transformed:
        values = [0] + [10**power for power in range(int(math.log10(maximum)) + 1)]
        # Keep labels sparse when tails span many orders of magnitude.
        if len(values) > 8:
            values = [0] + values[3::2]
        def label(value):
            for unit, suffix in ((10**12, "T"), (10**9, "B"), (10**6, "M"), (10**3, "k")):
                if value >= unit:
                    return f"{value / unit:g}{suffix}" if value < 10**15 else f"{value:.0e}"
            return str(value)
        return ", ".join(f'{gp(label(value))} {math.log10(1 + value):.12g}' for value in values)
    return None


def step_data(support, transform, upper):
    """Ties jump by multiplicity; zero/singleton and continuation remain faithful."""
    if not support.n:
        return []
    points = [(0.0, 0.0)]
    previous = 0
    for value, count in sorted(support.values.items()):
        x = math.log10(1 + value) if transform else value
        points.extend(((x, 100 * previous / support.n),
                       (x, 100 * (previous + count) / support.n)))
        previous += count
    points.append((upper, 100.0))
    return points


def render(out, supports, ranges, args, counts, *, synthetic=False):
    """One deterministic inventory supplies every page's colors and shared axes."""
    groups = sorted({key[2] for key in supports}) or ["All selected"]
    palette = ["#2563eb", "#d97706", "#16866b", "#a044b6"]
    colors = {}
    for index, group in enumerate(groups):
        if index < len(palette):
            colors[group] = palette[index]
        else:
            rgb = colorsys.hsv_to_rgb((index * 0.61803398875) % 1, 0.72, 0.64)
            colors[group] = "#" + "".join(f"{round(channel * 255):02x}" for channel in rgb)
    axes = {}
    for metric in METRICS:
        values = [value for (_, kind, _), support in supports.items() if kind == metric for value in support.values]
        maximum = max(values, default=0)
        positive = min((value for value in values if value), default=1)
        transformed = maximum > max(1000 if metric != "compactions" else 100, positive * 30)
        upper = (math.log10(1 + maximum) if transformed else maximum) * 1.04 or 1
        axes[metric] = maximum, transformed, upper
    page_index = []
    for page, offset in enumerate(range(0, len(groups), 4), 1):
        selected = groups[offset:offset + 4]
        basename = f"context-page-{page:02}"
        page_index.append(f"{basename}: " + json.dumps(selected, ensure_ascii=False))
        lines = [
            'set encoding utf8',
            'set border 3 lc rgb "#aab5c3"',
            'set tics nomirror textcolor rgb "#475569"',
            'set grid ytics lc rgb "#e2e8f0"',
            'set yrange [0:100]',
            'set ytics ("0%%" 0,"25%%" 25,"50%%" 50,"75%%" 75,"100%%" 100)',
            'unset key',
        ]
        data = []
        plots = []
        for metric_index, metric in enumerate(METRICS):
            maximum, transformed, upper = axes[metric]
            for range_index, bounds in enumerate(ranges):
                panel = f"p{metric_index}{range_index}"
                samples = sum(supports.get((range_index, metric, group), Support()).n for group in selected)
                missing = sum(supports.get((range_index, metric, group), Support()).missing for group in selected)
                opened = sum(supports.get((range_index, metric, group), Support()).open for group in selected)
                left = 0.105 + (0.455 * range_index if len(ranges) == 2 else 0)
                right = left + (0.365 if len(ranges) == 2 else 0.815)
                top = 0.785 - 0.232 * metric_index
                bottom = top - 0.156
                plot = [
                    f"set lmargin at screen {left}", f"set rmargin at screen {right}",
                    f"set tmargin at screen {top}", f"set bmargin at screen {bottom}",
                    f"set title {gp(TITLES[metric_index])} font ',14' offset 0,.4",
                    f"set xlabel {gp(UNITS[metric_index] + (' · input tokens' if metric != 'compactions' else '') + (' (log1p scale)' if transformed else ''))} font ',10'",
                    "set ylabel 'Share at or below x' font ',10'" if range_index == 0 else "unset ylabel",
                    f"set xrange [0:{upper:.12g}]",
                    (f"set xtics ({ticks(maximum, True)})" if transformed else
                     "set xtics 1" if metric == "compactions" and maximum <= 10 else "set xtics autofreq"),
                    f"set label 200 {gp(f'n={samples:,}  ·  missing={missing:,}  ·  open segments={opened:,}')} at graph 0,1.045 front font ',10' tc rgb '#64748b'",
                ]
                terms = []
                for group_index, group in enumerate(selected):
                    support = supports.get((range_index, metric, group), Support())
                    if not support.n:
                        continue
                    block = f"{panel}g{group_index}"
                    data.append(f"${block} << EOD")
                    data.extend(f"{x:.15g} {y:.15g}" for x, y in step_data(support, transformed, upper))
                    data.append("EOD")
                    terms.append(f"${block} using 1:2 with lines lw 2.6 lc rgb {gp(colors[group])} notitle")
                if not terms:
                    plot.append("set label 201 'No measured observations' at graph .5,.5 center font ',14' tc rgb '#64748b'")
                    plot.append("plot NaN notitle")
                    plot.append("unset label 201")
                else:
                    plot.append("plot " + ", ".join(terms))
                plot.append("unset label 200")
                plots.extend(plot)
        layout = [
            "set multiplot",
            f"set label 100 {gp('Context & compaction distributions' + (' | SYNTHETIC TEST DATA' if synthetic else ''))} at screen .06,.975 font ',23' tc rgb '#0f172a'",
            f"set label 101 {gp(f'{args.breakdown} breakdown · ECDF steps · legend n: accumulated / segments / compactions · page {page}/{math.ceil(len(groups)/4)}')} at screen .06,.946 font ',12' tc rgb '#64748b'",
        ]
        for index, group in enumerate(selected):
            display = textwrap.shorten(group.replace("\n", " "), width=84, placeholder="…")
            wrapped = "\\n".join(textwrap.wrap(display, width=46))
            # gp handles literal newlines, not a model-provided escape sequence.
            layout.append(
                f"set label {110 + index} {gp('━━  ' + wrapped.replace(chr(92)+'n', chr(10)))} "
                f"at screen {0.06 + .455*(index%2)},{.916 - .034*(index//2)} "
                f"font ',11' tc rgb {gp(colors[group])}")
            denominators = []
            for range_index in range(len(ranges)):
                ns = [supports.get((range_index, metric, group), Support()).n for metric in METRICS]
                denominators.append(chr(65 + range_index) + ": " + " / ".join(f"{n:,}" for n in ns))
            layout.append(
                f"set label {120+index} {gp('n  ' + '     ·     '.join(denominators))} "
                f"at screen {0.06+.455*(index%2)},{.891-.034*(index//2)} "
                "font ',9' tc rgb '#64748b'")
        for index, bounds in enumerate(ranges):
            short = f"{chr(65+index)}   {iso(bounds[0])} ≤ creation < {iso(bounds[1])}"
            if args.cohort == "active":
                short = f"{chr(65+index)}   activity in [{iso(bounds[0])}, {iso(bounds[1])})"
            layout.append(f"set label {102+index} {gp(short)} at screen {0.105+.455*index},.822 font ',10' tc rgb '#0f172a'")
        footer = f"{args.cohort.title()}-in-range; lifetime evidence before each endpoint. Open measured tails included. Not unique content."
        selection = "Selection: " + ("roles=" + json.dumps(args.role, ensure_ascii=False) + "; " if args.role else "") + ("model categories=" + json.dumps(args.model, ensure_ascii=False) if args.model else "all model categories")
        clock_skips = sum(counts.get(reason + "_journals", 0)
                          for reason in ("missing_clock", "regressing_clock", "invalid_projection"))
        coverage = (
            f"Coverage: {counts.get('validated_journals', 0):,} / "
            f"{counts.get('scanned_journals', 0):,} journals validated (all scanned history); "
            f"{counts.get('failed_or_unsupported_journals', 0):,} failed/unsupported; "
            f"{counts.get('timed_out_journals', 0):,} timed out; "
            f"{clock_skips:,} clock/projection skips. Exclusions are not zeros.")
        layout.extend([
            f"set label 104 {gp(footer)} at screen .06,.064 font ',11' tc rgb '#475569'",
            f"set label 105 {gp(textwrap.shorten(selection, width=170, placeholder='… (exact selection in README)'))} at screen .06,.046 font ',10' tc rgb '#64748b'",
            f"set label 106 {gp(coverage)} at screen .06,.024 font ',10' tc rgb '#64748b'",
        ])
        program = "\n".join(data + lines + [
            "do for [format in 'svg png'] {",
            " if (format eq 'svg') { set terminal svg size 1760,1400 noenhanced font 'DejaVu Sans,12' }",
            " if (format eq 'png') { set terminal pngcairo size 1760,1400 noenhanced font 'DejaVu Sans,12' }",
            f" set output {gp(basename + '.')} . format",
        ] + layout + plots + ["unset multiplot", "unset label", "}", "set output", ""])
        (out / (basename + ".gnuplot")).write_text(program)
        subprocess.run(["gnuplot", basename + ".gnuplot"], cwd=out, check=True)
    (out / "pages.txt").write_text("\n".join(page_index) + "\n")
    return axes


def write_report(args, ranges, supports, counts, *, synthetic=False):
    args.out.mkdir(mode=0o700, parents=False, exist_ok=False)
    # Explicit chmod defeats inherited permissive umasks/default directory modes.
    args.out.chmod(0o700)
    with (args.out / "distribution.csv").open("w", newline="") as file:
        writer = csv.writer(file)
        writer.writerow(("range", "since_inclusive", "until_exclusive", "breakdown", "group",
                         "metric", "value", "multiplicity", "cumulative_count", "denominator"))
        for (index, metric, group), support in sorted(supports.items()):
            cumulative = 0
            for value, count in sorted(support.values.items()):
                cumulative += count
                writer.writerow((chr(65 + index), *map(iso, ranges[index]), args.breakdown,
                                 group, metric, value, count, cumulative, support.n))
    with (args.out / "summary.csv").open("w", newline="") as file:
        writer = csv.writer(file)
        writer.writerow(("range", "group", "metric", "eligible", "n", "missing", "zero",
                         "open_segments", "partial_units", "inference_starts", "measured_inputs",
                         "empty_segments", "compact_inputs_available", "p50", "p90", "p95", "max"))
        for (index, metric, group), support in sorted(supports.items()):
            writer.writerow((chr(65 + index), group, metric, support.eligible, support.n,
                             support.missing, support.values.get(0, 0), support.open,
                             support.partial, support.starts, support.measured, support.empty_tails,
                             support.compact_measured, support.quantile(.5), support.quantile(.9),
                             support.quantile(.95), max(support.values, default=None)))
    axes = render(args.out, supports, ranges, args, counts, synthetic=synthetic)
    selection = dict(cohort=args.cohort, breakdown=args.breakdown, role=args.role, model=args.model,
                     ranges=[dict(since_inclusive=iso(a), until_exclusive=iso(b)) for a, b in ranges])
    summary = json.dumps(dict(selection=selection, coverage=dict(counts)), indent=2, ensure_ascii=False)
    (args.out / "summary.json").write_text(summary + "\n")
    (args.out / "README.md").write_text(
        "# Context and compaction distributions\n\n"
        "## Exact selection and scan coverage\n\n```json\n" + summary + "\n```\n\n"
        "Ranges use inclusive starts and exclusive endpoints. Default: agents created in the "
        "trailing fourteen days, UTC now captured once, all lifetime evidence before the endpoint. "
        "Active cohorts instead require ordinary starts or accepted compactions in range and include "
        "their whole earlier lifetime; their ages differ. No provider/harness calls are made.\n\n"
        "## Interpretation\n\n"
        "Each individual sample is the maximum provider-reported ordinary inference input count "
        "in a successful-compaction-delimited segment, owned by prompt start. Accumulated samples "
        "sum those peaks, not overlapping prompt sizes. Cached inputs are included; outputs and "
        "compact-request inputs are excluded. Compact inputs are a different request cut, not "
        "an after-context size. Only accepted AgentCompacted boundaries count.\n\n"
        "These are observed request-input workload proxies, NOT unique accumulated text, final "
        "context lengths, billed tokens or window limits. System/tool material and carried summaries "
        "repeat across segments; additions never sent to inference are invisible. Different model "
        "tokenizers make cross-model totals non-invariant. ECDF steps show share at or below x; "
        "ties jump by multiplicity. Percentiles use nearest rank. Horizontal continuation is a "
        "CDF, not extrapolated observations.\n\n"
        "## Coverage and attribution\n\n"
        "Creation roles are immutable journal evidence. Model category is the set of ordinary "
        "models in the metric unit: one exact provider/model, Mixed models, or Unknown model. "
        "Repeatable --model selects these categories AFTER whole structural aggregation, separately "
        "at each metric's unit (a mixed agent may have single-model segments). Role filters apply "
        "to the agent uniformly. Provider breakdown uses the set of ordinary provider prefixes. "
        "No alias guessing or last-model attribution occurs.\n\n"
        "Unknown nonempty segments omit the strict agent sum, while measured individual segments "
        "and valid zero compaction counts remain. A segment with some missing terminals retains "
        "its observed peak but has partial coverage. Empty tails have no size sample and do not "
        "invalidate measured totals. Measured open tails are included; open means no subsequent "
        "boundary as of that endpoint, not currently running or permanently finished. Agents "
        "with no inference have missing size, not a fabricated zero. True measured zeros remain "
        "zeros. n + missing = eligible. Coverage fields refer to underlying segments/prompts "
        "and are not additive across metrics. Compaction compact-input availability is diagnostic only.\n\n"
        "Unsupported/corrupt/timed-out journals and terminals with missing start ownership are "
        "skipped, never zero agents. Conservative clock "
        "policy skips any journal with missing or regressing append timestamps, even on unrelated "
        "events, to avoid an ambiguous historical cut. The typed supported journal schema is the "
        "authority for token counts: legacy serde defaults cannot prove raw serialized field presence "
        "and arbitrary legacy zeros are NOT independently authenticated. A running checkpoint can "
        "lag writes; snapshots across agents are not globally atomic. All scan failure counters "
        "cover all discovered history, NOT proven eligible-in-range failures. The broad "
        "failed_or_unsupported_journals total contains the disjoint CLI diagnostic categories; "
        "do not add that total to its subcategories. Decode-incompatible-or-malformed combines "
        "unsupported legacy typed shapes and malformed encodings because CLI text cannot prove "
        "which caused a decode failure. Checkpoint categories distinguish unsupported checkpoint "
        "schemas from missing/invalid live checkpoints. Unknown diagnostics remain other failures, "
        "not guessed migrations or corruption. Raw diagnostics are never exported.\n\n"
        "## Artifacts and reproducibility\n\n"
        "distribution.csv is lossless aggregate support; summary.csv preserves zero/missing/open/"
        "partial coverage and percentiles; summary.json reports scan coverage. pages.txt lists exact "
        "groups. Every page shares each metric's X limits and stable report colors across ranges. "
        "Display labels may be shortened; exact names remain in CSV and pages.txt. No groups or tails "
        "are dropped. Tokens and exceptionally wide compaction ranges use log10(1+x), with real zero "
        "preserved and ticks labeled in original units. Otherwise axes are linear. Axis decisions: "
        + json.dumps(axes) + ".\n\n"
        "Re-render from this directory: `gnuplot context-page-01.gnuplot` (repeat for each page). "
        "Programs embed only aggregate step support. CSV cells are literal exact names; treat them "
        "as text when importing into spreadsheets. No IDs, paths, transactions, prompt text or raw "
        "errors are exported. Aggregate activity, roles and models remain sensitive: keep this "
        "owner-only directory private and inspect before sharing. Do not commit artifacts.\n")
    for file in args.out.iterdir():
        file.chmod(0o600)
    print(summary)
    print(f"Artifacts: {args.out}")


def parser():
    result = argparse.ArgumentParser(description=__doc__)
    state = Path(os.environ.get("XDG_STATE_HOME", str(Path.home() / ".local/state")))
    result.add_argument("--agents-dir", type=Path, default=state / "tau/agents")
    result.add_argument("--tau", type=Path, default=Path("tau"))
    result.add_argument("--since")
    result.add_argument("--until")
    result.add_argument("--compare-since")
    result.add_argument("--compare-until")
    result.add_argument("--cohort", choices=("created", "active"), default="created")
    result.add_argument("--breakdown", choices=("none", "role", "model", "provider", "role-model"), default="none")
    result.add_argument("--role", action="append", default=[], help="Exact immutable creation role; repeatable")
    result.add_argument("--model", action="append", default=[], help="Exact aggregate provider/model category, Mixed models, or Unknown model; repeatable")
    result.add_argument("--timeout", type=float, default=120, help="Finite per-journal trace timeout, seconds")
    result.add_argument("--out", type=Path, required=True, help="New owner-only output directory")
    return result


def main():
    args = parser().parse_args()
    if not math.isfinite(args.timeout) or args.timeout <= 0:
        raise ValueError("--timeout must be positive and finite")
    if bool(args.compare_since) != bool(args.compare_until):
        raise ValueError("comparison requires both --compare-since and --compare-until")
    now = instant(dt.datetime.now(dt.timezone.utc).isoformat())
    ranges = [select_range(args.since, args.until, now)]
    if args.compare_since:
        ranges.append(select_range(args.compare_since, args.compare_until, now))
    counts = collections.Counter()
    supports = scan(args, ranges, counts)
    write_report(args, ranges, supports, counts)


if __name__ == "__main__":
    main()
