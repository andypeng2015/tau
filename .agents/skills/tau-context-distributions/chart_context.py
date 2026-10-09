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
import hashlib
import json
import math
import os
import re
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
# Exact standard roster in built-in.harness.yaml and docs/examples/harness.yaml.
# This report policy is deliberately not a prefix match or runtime role discovery.
STANDARD_ROLES = (
    "coordinator", "engineer-junior", "engineer", "engineer-senior",
    "researcher", "researcher-senior", "researcher-ultra", "reviewer",
)
PAGE_GROUPS = 10


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


def human_range(bounds):
    """Date-only chart shorthand; exact UTC instants remain in report files."""
    dates = [dt.datetime.fromisoformat(iso(value).replace("Z", "+00:00"))
             for value in bounds]
    start, end = dates
    def label(date, year=False):
        return f"{date:%b} {date.day}" + (f", {date.year}" if year else "")
    return label(start, start.year != end.year) + " – " + label(end, True)


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
    # Keep raw evidence on segments for the explicit provider breakdown.
    models = {model.split("/", 1)[1] for model in models}
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
    # Unambiguous joint identity even when custom names contain separators.
    return json.dumps([role, model], ensure_ascii=False)


def group_pair(group):
    """Read exact joint identities; legacy support labels are rerenderable."""
    try:
        pair = json.loads(group)
    except (ValueError, TypeError):
        pair = None
    if isinstance(pair, list) and len(pair) == 2 and all(isinstance(item, str) for item in pair):
        return tuple(pair)
    role, _, model = group.partition(" | ")
    return role, model


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


def selected_roles(args):
    return tuple(args.role) if args.role else None if args.all_roles else STANDARD_ROLES


def aggregate(agents, args, supports, counts):
    roles = selected_roles(args)
    for index, agent in enumerate(agents):
        if not agent.selected(args.cohort) or roles is not None and agent.role not in roles:
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
            category = model_category(unit_models)
            if (args.breakdown in ("model", "role-model") and category == MIXED
                    and not args.include_mixed and MIXED not in args.model):
                counts[f"{chr(65 + index)}_{metric}_excluded_mixed_units"] += 1
                continue
            if args.model and category not in args.model:
                counts[f"{chr(65 + index)}_{metric}_excluded_model_filter_units"] += 1
                continue
            counts[f"{chr(65 + index)}_{metric}_included_units"] += 1
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


def ticks(maximum, offset, pixels=1400):
    if offset:
        if offset == 1 and maximum <= 20:
            values = (list(range(maximum + 1)) if maximum <= 5 else
                      sorted({value for value in (0, 1, 2, 3, 5, 10, maximum) if value <= maximum}))
        else:
            first = 4 if offset == 10_000 else 0
            values = [0] + [factor * 10**power
                            for power in range(first, int(math.log10(maximum)) + 1)
                            for factor in (1, 2, 5) if factor * 10**power <= maximum]
        def label(value):
            for unit, suffix in ((10**12, "T"), (10**9, "B"), (10**6, "M"), (10**3, "k")):
                if value >= unit:
                    return f"{value / unit:g}{suffix}" if value < 10**15 else f"{value:.0e}"
            return str(value)
        upper = math.log10(1 + maximum / offset) * 1.04
        kept = []
        last_right = -math.inf
        for value in values:
            text = label(value)
            x = math.log10(1 + value / offset)
            center = x / upper * pixels
            half_width = len(text) * 4
            if center - half_width >= last_right + 12:
                kept.append(f'{gp(text)} {x:.12g}')
                last_right = center + half_width
        return ", ".join(kept)
    return None


def step_data(support, offset, upper):
    """Ties jump by multiplicity; zero/singleton and continuation remain faithful."""
    if not support.n:
        return []
    points = [(0.0, 0.0)]
    previous = 0
    for value, count in sorted(support.values.items()):
        x = math.log10(1 + value / offset) if offset else value
        points.extend(((x, 100 * previous / support.n),
                       (x, 100 * (previous + count) / support.n)))
        previous += count
    points.append((upper, 100.0))
    return points


def stable_fraction(name):
    """Unlike Python hash(), this survives processes, filters and report ranges."""
    return int.from_bytes(hashlib.sha256(name.encode()).digest()[:8], "big") / 2**64


def oklch_hex(lightness, chroma, hue):
    """OKLCH -> sRGB, reducing chroma at fixed L/h rather than clipping hue.

    Inverse OKLab matrix: Björn Ottosson's public-domain reference implementation
    https://bottosson.github.io/posts/oklab/ (2021-01-25 matrices).
    """
    def linear_rgb(c):
        a = c * math.cos(math.radians(hue))
        b = c * math.sin(math.radians(hue))
        l = (lightness + .3963377774 * a + .2158037573 * b)**3
        m = (lightness - .1055613458 * a - .0638541728 * b)**3
        s = (lightness - .0894841775 * a - 1.2914855480 * b)**3
        return (4.0767416621 * l - 3.3077115913 * m + .2309699292 * s,
                -1.2684380046 * l + 2.6097574011 * m - .3413193965 * s,
                -.0041960863 * l - .7034186147 * m + 1.7076147010 * s)
    rgb = linear_rgb(chroma)
    if not all(0 <= channel <= 1 for channel in rgb):
        low, high = 0, chroma
        for _ in range(24):
            middle = (low + high) / 2
            if all(0 <= channel <= 1 for channel in linear_rgb(middle)):
                low = middle
            else:
                high = middle
        rgb = linear_rgb(low)
    def encoded(channel):
        channel = min(1, max(0, channel))  # Final floating-point/rounding guard.
        return 12.92 * channel if channel <= .0031308 else 1.055 * channel**(1/2.4) - .055
    return "#" + "".join(f"{round(encoded(channel) * 255):02x}" for channel in rgb)


def role_style(role):
    family, level = role, 1
    for suffix, rank in (("-junior", 0), ("-senior", 2), ("-ultra", 3)):
        if role.endswith(suffix):
            family, level = role[:-len(suffix)], rank
            break
    base = {"coordinator": 55, "engineer": 260, "researcher": 150, "reviewer": 25}.get(
        family, 360 * stable_fraction("role:" + family))
    hue = (base + (-12, 0, 12, 24)[level]) % 360
    return (family, level, role), hue, (.64, .58, .52, .46)[level]


def model_version(model):
    """Presentation family/version only; never alias aggregation identities."""
    match = re.search(r"(?<![A-Za-z0-9])(\d+(?:\.\d+)*)(?![A-Za-z0-9.])", model)
    if match:
        version = tuple(int(part) for part in match[1].split("."))
        family = model[:match.start()] + "{version}" + model[match.end():]
    else:
        family, version = model, ()
    return family, version


def model_lightness(models):
    families = collections.defaultdict(set)
    for model in models:
        family, version = model_version(model)
        if version:
            families[family].add(version)
    result = {}
    for model in models:
        family, version = model_version(model)
        versions = sorted(families[family])
        # Up to .18 total separation, preserving visible role hues and contrast.
        # A singleton/unversioned family uses the normal role lightness.
        result[model] = (.09 - .18 * versions.index(version) / (len(versions) - 1)
                         if version and len(versions) > 1 else 0)
    return result


def model_style(model):
    family, version = model_version(model)
    lightness = .45
    hue = stable_fraction("model:" + family)
    return (family, version, model), hue, lightness


def group_style(group, breakdown, lightness_offsets=None):
    if breakdown == "role":
        key, hue, lightness = role_style(group)
    elif breakdown == "model":
        key, hue, lightness = model_style(group)
        lightness = .58 + (lightness_offsets or {}).get(group, 0)
    elif breakdown == "role-model":
        role, model = group_pair(group)
        role_key, hue, lightness = role_style(role)
        model_key, _, _ = model_style(model)
        key = role_key, model_key, group
        lightness += (lightness_offsets or {}).get(model, 0)
    else:
        key, hue, lightness = (group,), stable_fraction(breakdown + ":" + group), .42
    if breakdown in ("role", "role-model", "model"):
        color = oklch_hex(lightness, .15, hue * 360 if breakdown == "model" else hue)
    else:
        rgb = colorsys.hls_to_rgb(hue, lightness, .72)
        color = "#" + "".join(f"{round(channel * 255):02x}" for channel in rgb)
    return key, color


def model_pattern(model):
    """Preferred line-only pattern for an exact normalized model name."""
    digest = hashlib.sha256(("pattern:" + model).encode()).digest()
    return digest[0] % len(LINE_PATTERNS)


LINE_PATTERNS = ("1", "(1,3)", "(4,3)", "(20,4)", "(8,3,1,3)",
                 "(8,3,1,3,1,3)", "(3,7)", "(14,7)",
                 "(12,3,4,3)", "(1,7)", "(4,3,4,7)", "(16,3,1,3,1,3)")


def model_patterns(models):
    """Resolve exact observed-inventory collisions, consistently across panels.

    Role hues stay filter-independent. A pattern can change if a conflicting
    family is filtered away; export the exact table rather than promising otherwise.
    """
    result, used = {}, {"1", "(20,4)"}
    families = sorted({model_version(model)[0] for model in models})
    for family in families:
        if family == "gpt-{version}-astra":
            result[family] = "1"
            continue
        if family == "gpt-{version}-sol":
            result[family] = "(20,4)"
            continue
        preference = model_pattern(family)
        candidates = [LINE_PATTERNS[(preference + i) % len(LINE_PATTERNS)]
                      for i in range(len(LINE_PATTERNS))]
        dash = next((pattern for pattern in candidates if pattern not in used), None)
        if dash is None:
            # Unbounded inventory fallback; no geometric marker fallback.
            length = 20
            while f"({length},4,1,4)" in used:
                length += 4
            dash = f"({length},4,1,4)"
        result[family] = dash
        used.add(dash)
    return {model: result[model_version(model)[0]] for model in models}


def metric_axis(metric, values):
    maximum = max(values, default=0)
    positive = min((value for value in values if value), default=1)
    if metric == "compactions":
        offset = 1 if maximum > 1 else 0
    else:
        offset = 10_000 if maximum > max(10_000, positive * 30) else 0
    upper = (math.log10(1 + maximum / offset) if offset else maximum) * 1.04 or 1
    return maximum, offset, upper


def render(out, supports, ranges, args, counts, *, synthetic=False):
    """Semantic family order, shared axes, role hues and version brightness."""
    groups = sorted({key[2] for key in supports},
                    key=lambda group: group_style(group, args.breakdown)[0]) or ["All selected"]
    combined = args.breakdown == "role-model"
    roles = sorted({group_pair(group)[0] for group in groups},
                   key=lambda role: role_style(role)[0]) if combined else []
    models = sorted({group_pair(group)[1] for group in groups},
                     key=lambda model: model_style(model)[0]) if combined else []
    lightness_offsets = model_lightness(models if combined else groups if args.breakdown == "model" else [])
    colors = {group: group_style(group, args.breakdown, lightness_offsets)[1] for group in groups}
    patterns = model_patterns(models if combined else groups if args.breakdown == "model" else [])
    if combined or args.breakdown == "model":
        (out / "model-styles.json").write_text(json.dumps(
            {model: dict(family=model_version(model)[0], dash=dash,
                         lightness_offset=lightness_offsets[model])
             for model, dash in patterns.items()},
            ensure_ascii=False, indent=2) + "\n")
    axes = {}
    for metric in METRICS:
        values = [value for (_, kind, _), support in supports.items() if kind == metric for value in support.values]
        axes[metric] = metric_axis(metric, values)
    page_index = []
    page_groups = len(groups) if combined else PAGE_GROUPS
    for page, offset in enumerate(range(0, len(groups), page_groups), 1):
        selected = groups[offset:offset + page_groups]
        legend_rows = math.ceil(len(selected) / 2)
        date_y = (.908 - .009 * max(len(roles), len(models)) - .018 if combined else
                  .916 - .028 * (legend_rows - 1) - .045)
        panel_top = date_y - .04
        panel_step = (panel_top - .03) / 3
        width = 3520 if combined else 1760
        # Twofold increase in each dimension; fonts/strokes stay normal-sized
        # for zooming rather than consuming the extra pixel space.
        height = 3600 if combined else 1800 if legend_rows > 2 else 1400
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
            maximum, offset, upper = axes[metric]
            for range_index, bounds in enumerate(ranges):
                panel = f"p{metric_index}{range_index}"
                left = 0.105 + (0.455 * range_index if len(ranges) == 2 else 0)
                right = left + (0.365 if len(ranges) == 2 else 0.815)
                top = panel_top - panel_step * metric_index
                bottom = top - panel_step + .064
                plot = [
                    f"set lmargin at screen {left}", f"set rmargin at screen {right}",
                    f"set tmargin at screen {top}", f"set bmargin at screen {bottom}",
                    f"set title {gp(TITLES[metric_index])} font ',14' offset 0,.4",
                    f"set xlabel {gp(('Input tokens' if metric != 'compactions' else 'Compactions') + (' · shifted log (10k offset)' if offset == 10_000 else ' · log1p' if offset else ''))} font ',10'",
                    "unset ylabel",
                    f"set xrange [0:{upper:.12g}]",
                    (f"set xtics ({ticks(maximum, offset, pixels=int((right-left)*width))})" if offset else
                     "set xtics 1" if metric == "compactions" and maximum <= 10 else "set xtics autofreq"),
                ]
                terms = []
                for group_index, group in enumerate(selected):
                    support = supports.get((range_index, metric, group), Support())
                    if not support.n:
                        continue
                    block = f"{panel}g{group_index}"
                    data.append(f"${block} << EOD")
                    points = step_data(support, offset, upper)
                    data.extend(f"{x:.15g} {y:.15g}" for x, y in points)
                    data.append("EOD")
                    if combined:
                        dash = patterns[group_pair(group)[1]]
                    else:
                        dash = patterns[group] if args.breakdown == "model" else "1"
                    style = f"lines lw 2.6 dt {dash}"
                    terms.append(f"${block} using 1:2 with {style} lc rgb {gp(colors[group])} notitle")
                if not terms:
                    plot.append("set label 201 'No measured observations' at graph .5,.5 center font ',14' tc rgb '#64748b'")
                    plot.append("plot NaN notitle")
                    plot.append("unset label 201")
                else:
                    plot.append("plot " + ", \\\n".join(terms))
                plots.extend(plot)
        layout = [
            "set multiplot",
            f"set label 100 {gp('Context & compaction distributions' + (' | SYNTHETIC TEST DATA' if synthetic else ''))} at screen .06,.975 font ',23' tc rgb '#0f172a'",
            f"set label 101 {gp('By role and model' if combined else f'By {args.breakdown}' + (f' · page {page}/{math.ceil(len(groups)/PAGE_GROUPS)}' if len(groups) > PAGE_GROUPS else ''))} at screen .06,.946 font ',12' tc rgb '#64748b'",
        ]
        if combined:
            layout.extend([
                "set label 104 'Roles · hues' at screen .06,.927 font ',12' tc rgb '#475569'",
                "set label 105 'Models · family patterns, newer darker' at screen .515,.927 font ',12' tc rgb '#475569'",
            ])
            for index, role in enumerate(roles):
                y = .908 - .009 * index
                color = group_style(role, "role")[1]
                layout.extend([
                    f"set arrow {10+index} from screen .06,{y} to screen .076,{y} nohead lw 2.6 lc rgb {gp(color)}",
                    f"set label {110+index} {gp(textwrap.shorten(role, width=96, placeholder='…'))} at screen .083,{y} font ',11' tc rgb {gp(color)}",
                ])
            for index, model in enumerate(models):
                y = .908 - .009 * index
                dash = patterns[model]
                shade = oklch_hex(.58 + lightness_offsets[model], 0, 0)
                layout.extend([
                    f"set arrow {1000+index} from screen .515,{y} to screen .541,{y} nohead lw 2.6 dt {dash} lc rgb {gp(shade)}",
                    f"set label {3000+index} {gp(textwrap.shorten(model, width=132, placeholder='…'))} at screen .548,{y} font ',11' tc rgb '#475569'",
                ])
        for index, group in enumerate([] if combined else selected):
            display = textwrap.shorten(group.replace("\n", " "), width=46, placeholder="…")
            dash = patterns[group] if args.breakdown == "model" else "1"
            layout.append(
                f"set arrow {10+index} from screen {0.06+.455*(index%2)},{.916-.028*(index//2)} "
                f"to screen {0.076+.455*(index%2)},{.916-.028*(index//2)} "
                f"nohead lw 2.6 dt {dash} lc rgb {gp(colors[group])}")
            layout.append(
                f"set label {110 + index} {gp(display)} "
                f"at screen {0.083 + .455*(index%2)},{.916 - .028*(index//2)} "
                f"font ',11' tc rgb {gp(colors[group])}")
        for index, bounds in enumerate(ranges):
            short = (f"{chr(65+index)}   " if len(ranges) > 1 else "") + human_range(bounds)
            layout.append(f"set label {102+index} {gp(short)} at screen {0.105+.455*index},{date_y} font ',10' tc rgb '#0f172a'")
        program = "\n".join(data + lines + [
            "do for [format in 'svg png'] {",
            f" if (format eq 'svg') {{ set terminal svg size {width},{height} noenhanced font 'DejaVu Sans,12' }}",
            f" if (format eq 'png') {{ set terminal pngcairo size {width},{height} noenhanced font 'DejaVu Sans,12' }}",
            f" set output {gp(basename + '.')} . format",
        ] + layout + plots + ["unset multiplot", "unset label", "unset arrow", "}", "set output", ""])
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
                     model_identity="exact suffix after first /",
                     mixed_models_included=(args.breakdown not in ("model", "role-model")
                                            or args.include_mixed or MIXED in args.model),
                     role_scope="explicit" if args.role else "all" if args.all_roles else "standard",
                     effective_roles=selected_roles(args),
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
         "model names in the metric unit: exact suffix after the first /, Mixed models, or Unknown model. "
         "Different providers using the same exact suffix are one model, not Mixed models. "
         "Model and role-model views exclude Mixed models by default after normalization; "
         "--include-mixed or --model 'Mixed models' opts in. Exclusion/inclusion counts are "
         "per metric unit above; selected_agents is the cohort before model exclusions. "
        "Repeatable --model selects these categories AFTER whole structural aggregation, separately "
        "at each metric's unit (a mixed agent may have single-model segments). Role filters apply "
        "to the agent uniformly. By default only the exact standard roster is selected: "
        + ", ".join(STANDARD_ROLES) + ". Explicit --role replaces this roster, including custom "
        "roles; --all-roles selects all roles unless --role is given. Effective scope is exported "
        "above. Charts use date-only UTC shorthand; this report retains exact endpoints, cohort "
        "definitions, selections and scan coverage without cluttering the chart surface. "
        "Provider breakdown uses the set of ordinary provider prefixes. "
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
        "groups. Every page shares each metric's X limits and stable colors across reports/ranges. "
        "Related role families use light-to-dark junior/normal/senior/ultra shades; model families "
         "use numeric version order and related shades without aliasing different model names. "
        "The default combined role/model view puts all observed pairs on one page, with separate "
        "role-color and exact-model pattern legends, and a 3520×3600 zoomable image. It never "
        "fabricates unobserved role/model pairs. Joint CSV/page-index group identities are JSON "
        "[role, model] arrays so custom names cannot collide through a display separator. "
        "Exact model patterns are exported in model-styles.json. Inventory collisions are "
        "resolved deterministically; filtering away a conflicting model can change its pattern, "
         "while role hues remain filter-independent. Versions within each family share one "
         "line pattern: Astra is solid, Sol uses extended long dashes (20,4). Numeric version order maps "
         "oldest to +0.09 and newest to -0.09 OKLCH lightness relative to each role, with "
         "evenly spaced intermediate versions; singleton/unversioned families use zero offset. "
         "This report-inventory mapping may change when filtering versions. Numeric version "
         "order is not inferred release chronology. Grayscale model swatches show the same "
         "brightness offsets, while role swatches show base hues/lightness. "
        "Other breakdowns use at most ten series per page. "
        "Date comparisons are explicit opt-in; one window is the default. Display labels may be shortened; "
        "exact names remain in CSV and pages.txt. No selected groups or tails "
        "are dropped. Wide token ranges use log10(1+x/10000), emphasizing 10k–1M without hiding "
        "low/zero or extreme tails; small/narrow token ranges stay linear. Compaction counts use "
        "log10(1+x), except all-zero or binary ranges which stay linear. Real zeros are preserved "
        "and ticks use original units, including intermediate 2x/5x decade values where labels fit. "
        "Tick labels are collision-pruned by panel width, not data samples. "
         "Model line-only patterns match legend swatches; no geometric markers are used. "
        "Axis decisions [maximum, shift offset (0 means linear), upper plotted coordinate]: "
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
    result.add_argument("--breakdown", choices=("none", "role", "model", "provider", "role-model"), default="role-model")
    result.add_argument("--role", action="append", default=[], help="Exact immutable creation role; repeatable")
    result.add_argument("--all-roles", action="store_true", help="Include nonstandard roles; explicit --role still selects exact names")
    result.add_argument("--model", action="append", default=[], help="Exact model name after first /, Mixed models, or Unknown model; repeatable")
    result.add_argument("--include-mixed", action="store_true", help="Include Mixed models in model/role-model views (default excluded)")
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
