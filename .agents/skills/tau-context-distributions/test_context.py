"""Deterministic measurement and presentation oracles, without private journals."""

import collections
import io
import json
import os
import struct
from pathlib import Path
import tempfile
import unittest
from unittest.mock import patch

import chart_context as chart


def args(**values):
    result = chart.parser().parse_args(["--out", "unused"])
    # Measurement oracles use an explicit unbroken population, not CLI defaults.
    result.breakdown = "none"
    for name, value in values.items():
        setattr(result, name, value)
    return result


def evidence(facts, created=1, role="engineer", summary=None):
    rows = [dict(schema="tau.agent_context", schema_version=0,
                 record_type="header", content_included=False)]
    rows.append(dict(record_type="agent_created", seq=0,
                     recorded_at_unix_micros=created, role=role, agent_id="synthetic"))
    for seq, (kind, at, fields) in enumerate(facts, 1):
        rows.append(dict(record_type=kind, seq=seq, recorded_at_unix_micros=at,
                         agent_id="synthetic", **fields))
    rows.append(dict(record_type="journal_summary", agent_id="synthetic",
                     clock_regressions=0, missing_timestamps=0, **(summary or {})))
    return io.StringIO("\n".join(map(json.dumps, rows)))


def start(at, prompt, model="provider/model"):
    return "inference_started", at, dict(prompt_id=prompt, model=model)


def finish(at, prompt, value):
    return "inference_finished", at, dict(prompt_id=prompt, input_tokens=value)


def compact(at, transaction="tx", value=200):
    return "compacted", at, dict(transaction_id=transaction, model="provider/compact",
                                compact_input_tokens=value)


def collect(facts, ranges=((1, 100),), selection=None, created=1):
    states = chart.read_trace(evidence(facts, created), ranges)
    support, counts = {}, collections.Counter()
    chart.aggregate(states, selection or args(), support, counts)
    return states, support, counts


class MeasurementTests(unittest.TestCase):
    def test_exact_default_roster_and_explicit_overrides(self):
        roles = list(chart.STANDARD_ROLES) + ["engineer-custom", "reviewer-senior",
                                            "rostra-bot", "fedi-project-manager"]
        agents = [chart.read_trace(evidence([start(2, "a"), finish(3, "a", 10)],
                                           role=role), [(1, 100)])[0] for role in roles]
        for selection, expected in (
                (args(breakdown="role"), set(chart.STANDARD_ROLES)),
                (args(breakdown="role", all_roles=True), set(roles)),
                (args(breakdown="role", role=["rostra-bot"]), {"rostra-bot"}),
                (args(breakdown="role", role=["engineer"], all_roles=True), {"engineer"})):
            support, counts = {}, collections.Counter()
            for agent in agents:
                chart.aggregate([agent], selection, support, counts)
            self.assertEqual({key[2] for key in support}, expected)
            self.assertEqual(counts["A_selected_agents"], len(expected))
        for breakdown in ("none", "model", "provider", "role-model"):
            for cohort in ("created", "active"):
                support, counts = {}, collections.Counter()
                selection = args(breakdown=breakdown, cohort=cohort)
                for agent in agents:
                    chart.aggregate([agent], selection, support, counts)
                self.assertEqual(counts["A_selected_agents"], len(chart.STANDARD_ROLES))

    def test_segment_peaks_not_prompt_sums(self):
        """Repeated overlapping inputs contribute a single high-water per segment."""
        facts = [start(2, "a"), finish(3, "a", 10), start(4, "b"), finish(5, "b", 20),
                 start(6, "c"), finish(7, "c", 18), compact(8),
                 start(9, "d"), finish(10, "d", 4), start(11, "e"), finish(12, "e", 8)]
        _, support, _ = collect(facts)
        self.assertEqual(support[0, "segments", "All selected"].values, {20: 1, 8: 1})
        self.assertEqual(support[0, "accumulated", "All selected"].values, {28: 1})
        self.assertEqual(support[0, "compactions", "All selected"].values, {1: 1})
        self.assertEqual(support[0, "segments", "All selected"].open, 1)

    def test_prompt_spanning_boundary_and_empty_tail(self):
        """Terminal ownership follows start, and an empty fresh tail is not zero."""
        _, support, _ = collect([start(2, "a"), compact(3), finish(4, "a", 20)])
        self.assertEqual(support[0, "segments", "All selected"].values, {20: 1})
        self.assertEqual(support[0, "segments", "All selected"].open, 0)
        self.assertEqual(support[0, "accumulated", "All selected"].values, {20: 1})
        self.assertEqual(support[0, "accumulated", "All selected"].empty_tails, 1)

    def test_strict_unknown_sum_keeps_measured_segments_and_count(self):
        """An unknown nonempty segment invalidates only the whole-agent size sum."""
        _, support, _ = collect([start(2, "a"), finish(3, "a", None),
                                 compact(4), start(5, "b"), finish(6, "b", 0)])
        self.assertEqual(support[0, "accumulated", "All selected"].missing, 1)
        self.assertEqual(support[0, "segments", "All selected"].values, {0: 1})
        self.assertEqual(support[0, "segments", "All selected"].missing, 1)
        self.assertEqual(support[0, "compactions", "All selected"].values, {1: 1})

    def test_partial_segment_keeps_observed_peak(self):
        """Missing terminals within a measured segment mark coverage, not missing size."""
        _, support, _ = collect([start(2, "a"), finish(3, "a", 0), start(4, "b")])
        self.assertEqual(support[0, "accumulated", "All selected"].values, {0: 1})
        self.assertEqual(support[0, "accumulated", "All selected"].partial, 1)
        self.assertEqual(support[0, "accumulated", "All selected"].starts, 2)
        self.assertEqual(support[0, "accumulated", "All selected"].measured, 1)

    def test_no_inference_has_missing_size_real_zero_compactions(self):
        """Valid no-work agents are not fabricated zero-size contexts."""
        _, support, _ = collect([])
        self.assertEqual(support[0, "compactions", "All selected"].values, {0: 1})
        self.assertEqual(support[0, "accumulated", "All selected"].missing, 1)
        self.assertNotIn((0, "segments", "All selected"), support)

    def test_historical_endpoint_does_not_backfill(self):
        """One scan reconstructs independent endpoints with no future model/boundary leakage."""
        facts = [start(2, "a"), finish(10, "a", 20), compact(11),
                 start(12, "b", "other/different"), finish(13, "b", 8)]
        _, support, _ = collect(facts, ((1, 10), (1, 14)), args(breakdown="model", include_mixed=True))
        self.assertEqual(support[0, "accumulated", "model"].missing, 1)
        self.assertEqual(support[0, "compactions", "model"].values, {0: 1})
        self.assertEqual(support[1, "accumulated", chart.MIXED].values, {28: 1})
        self.assertEqual(support[1, "compactions", chart.MIXED].values, {1: 1})

    def test_creation_and_event_endpoints(self):
        """Since is inclusive, until exclusive, and an at-until boundary stays future."""
        _, support, counts = collect([start(3, "a"), finish(4, "a", 10), compact(10)],
                                     ((2, 10),), created=2)
        self.assertEqual(counts["A_selected_agents"], 1)
        self.assertEqual(support[0, "compactions", "All selected"].values, {0: 1})
        _, support, counts = collect([], ((1, 2),), created=2)
        self.assertEqual(support, {})
        self.assertEqual(counts["A_selected_agents"], 0)

    def test_model_filters_do_not_remove_boundaries(self):
        """Exact model selection applies after per-metric aggregation, not to raw rows."""
        facts = [start(2, "a"), finish(3, "a", 20), compact(4),
                  start(5, "b", "other/different"), finish(6, "b", 8)]
        _, support, _ = collect(facts, selection=args(model=["model"], breakdown="model"))
        self.assertEqual(support[0, "segments", "model"].values, {20: 1})
        self.assertEqual(len(support), 1)
        _, support, _ = collect(facts, selection=args(model=[chart.MIXED], breakdown="model"))
        self.assertEqual(support[0, "accumulated", chart.MIXED].values, {28: 1})
        self.assertEqual(support[0, "compactions", chart.MIXED].values, {1: 1})
        self.assertEqual(chart.group_for("role", {"provider/a", "provider/b"}, "provider"), "provider")

    def test_active_cohort_includes_prior_lifetime(self):
        """Active selection retains earlier segments rather than clipping workload."""
        facts = [start(2, "a"), finish(3, "a", 20), compact(4),
                 start(11, "b"), finish(12, "b", 8)]
        _, created, _ = collect(facts, ((10, 20),))
        self.assertEqual(created, {})
        _, active, _ = collect(facts, ((10, 20),), args(cohort="active"))
        self.assertEqual(active[0, "accumulated", "All selected"].values, {28: 1})

    def test_role_filters_use_creation_role(self):
        """Exact role filters are independent from models and agent-name prefixes."""
        _, support, _ = collect([start(2, "a")], selection=args(role=["reviewer"]))
        self.assertEqual(support, {})
        _, support, _ = collect([start(2, "a")], selection=args(breakdown="role"))
        self.assertIn((0, "accumulated", "engineer"), support)

    def test_unknown_and_mixed_segment_classes(self):
        """No model means unknown; multiple starts in one segment are genuinely mixed."""
        _, support, _ = collect([], selection=args(breakdown="model"))
        self.assertIn((0, "compactions", chart.UNKNOWN), support)
        _, support, _ = collect([start(2, "a"), finish(3, "a", 10),
                                 start(4, "b", "other/different"), finish(5, "b", 20)],
                                selection=args(breakdown="model", include_mixed=True))
        self.assertEqual(support[0, "segments", chart.MIXED].values, {20: 1})

    def test_provider_independent_identity_and_exact_filters(self):
        facts = [start(2, "a", "chatgpt/a/b"), finish(3, "a", 10),
                 start(4, "b", "chatgpt-fedi/a/b"), finish(5, "b", 20),
                 compact(6), start(7, "c", "other/a/b"), finish(8, "c", 8)]
        for breakdown in ("model", "role-model"):
            _, support, counts = collect(facts, selection=args(breakdown=breakdown))
            group = "a/b" if breakdown == "model" else json.dumps(["engineer", "a/b"])
            self.assertEqual(support[0, "accumulated", group].values, {28: 1})
            self.assertEqual(support[0, "segments", group].values, {20: 1, 8: 1})
            self.assertEqual(support[0, "compactions", group].values, {1: 1})
            self.assertEqual(counts["A_accumulated_excluded_mixed_units"], 0)
        _, support, _ = collect(facts, selection=args(breakdown="provider"))
        self.assertEqual(support[0, "accumulated", "Mixed providers"].values, {28: 1})
        self.assertEqual(support[0, "segments", "other"].values, {8: 1})
        for name, expected in (("a/b", 3), ("chatgpt/a/b", 0), ("A/b", 0)):
            _, support, _ = collect(facts, selection=args(breakdown="model", model=[name]))
            self.assertEqual(len(support), expected)

    def test_mixed_defaults_are_metric_local_and_opt_in(self):
        facts = [start(2, "a", "provider/a"), finish(3, "a", 10), compact(4),
                 start(5, "b", "other/b"), finish(6, "b", 20)]
        for breakdown in ("model", "role-model"):
            _, support, counts = collect(facts, selection=args(breakdown=breakdown))
            self.assertEqual({key[1] for key in support}, {"segments"})
            self.assertEqual(sum(unit.n for unit in support.values()), 2)
            self.assertEqual(counts["A_selected_agents"], 1)
            self.assertEqual(counts["A_accumulated_excluded_mixed_units"], 1)
            self.assertEqual(counts["A_compactions_excluded_mixed_units"], 1)
            self.assertEqual(counts["A_segments_excluded_mixed_units"], 0)
            self.assertEqual(counts["A_segments_included_units"], 2)
            _, included, _ = collect(facts, selection=args(breakdown=breakdown, include_mixed=True))
            self.assertEqual(sum(unit.n for unit in included.values()), 4)
            self.assertEqual(sum(unit.values.total() for (i, metric, g), unit in included.items()
                                 if metric == "accumulated"), 1)
        for breakdown in ("none", "role", "provider"):
            _, support, _ = collect(facts, selection=args(breakdown=breakdown))
            self.assertEqual({key[1] for key in support}, set(chart.METRICS))
        # A genuinely mixed segment is excluded too, not allocated to either model.
        _, support, counts = collect(
            [start(2, "a", "p/a"), finish(3, "a", 10),
             start(4, "b", "q/b"), finish(5, "b", 20)],
            selection=args(breakdown="model"))
        self.assertEqual(support, {})
        self.assertEqual(counts["A_segments_excluded_mixed_units"], 1)

    def test_duplicates_and_clock_ambiguity_rejected(self):
        """Ambiguous structural evidence never becomes repeated samples or zero counts."""
        for facts in ([start(2, "a"), start(3, "a")],
                      [start(2, "a"), finish(3, "a", 10), finish(4, "a", 10)],
                      [compact(2), compact(3)],
                      [start(3, "a"), finish(2, "a", 1)],
                      [start(0, "a")]):
            with self.subTest(facts=facts), self.assertRaises(ValueError):
                collect(facts)
        for field, reason in (("clock_regressions", "regressing_clock"), ("missing_timestamps", "missing_clock")):
            stream = evidence([])
            rows = list(stream)
            summary = json.loads(rows[-1])
            summary[field] = 1
            rows[-1] = json.dumps(summary)
            with self.assertRaisesRegex(ValueError, reason):
                chart.read_trace(iter(rows), [(1, 100)])

    def test_overflow_is_not_clamped(self):
        """Strict sums and individual scalar counts reject unsupported integer overflow."""
        with self.assertRaises(ValueError):
            collect([start(2, "a"), finish(3, "a", chart.U64), compact(4),
                     start(5, "b"), finish(6, "b", 1)])
        for bad in (-1, 1.5, True, chart.U64 + 1):
            with self.subTest(bad=bad), self.assertRaises(ValueError):
                chart.checked(bad)

    def test_ranges_require_zones_and_keep_precise_microseconds(self):
        """Captured current-moment defaults and offset timestamps do not round to days."""
        value = chart.instant("2026-10-08T12:34:56.123456Z")
        self.assertEqual(chart.iso(value), "2026-10-08T12:34:56.123456Z")
        self.assertEqual(chart.select_range(None, None, value), (value - 14 * chart.DAY, value))
        self.assertEqual(chart.instant("2026-10-08T14:34:56.123456+02:00"), value)
        for since, until in (("2026-10-08", None), (None, "1969-12-31T23:00:00Z"),
                             ("2026-10-08T12:34:56Z", "2026-10-08T12:34:56Z")):
            with self.assertRaises(ValueError):
                chart.select_range(since, until, value)

    def test_ties_zero_singleton_and_percentiles(self):
        """ECDF support preserves exact multiplicities and a genuine zero vertical step."""
        support = chart.Support()
        for value in (0, 10, 10, 20):
            support.add(value, [])
        self.assertEqual(support.quantile(.5), 10)
        self.assertEqual(support.quantile(.95), 20)
        self.assertEqual(chart.step_data(support, False, 21),
                         [(0., 0.), (0, 0.), (0, 25.), (10, 25.), (10, 75.),
                          (20, 75.), (20, 100.), (21, 100.)])
        singleton = chart.Support()
        singleton.add(0, [])
        self.assertEqual(chart.step_data(singleton, True, 1), [(0, 0), (0, 0), (0, 100), (1, 100)])
        self.assertEqual(chart.step_data(chart.Support(), False, 1), [])

    def test_literal_gnuplot_labels(self):
        """Hostile labels cannot escape strings, become commands, or use enhanced markup."""
        self.assertEqual(chart.gp('"; system "bad"\n\\x'), '"\\"; system \\"bad\\"\\n\\\\x"')

    def test_gnuplot_backticks_are_literal_not_shell_substitution(self):
        """Actual gnuplot evaluation must not execute role/model/filter label bytes."""
        with tempfile.TemporaryDirectory() as directory:
            marker = Path(directory) / "executed"
            text = f"""role' "quoted" \\slash \\`touch {marker}`\n`touch {marker}`"""
            result = chart.subprocess.run(["gnuplot", "-e", "print " + chart.gp(text)],
                                          capture_output=True, text=True, check=True)
            self.assertFalse(marker.exists())
            self.assertEqual(result.stderr.rstrip("\n"), text)

    def test_single_projection_feeds_two_ranges_without_error_leak(self):
        """Each discovered journal is projected once, failures export only aggregate counts."""
        with tempfile.TemporaryDirectory() as root:
            for name in ("agent-a", "agent-b"):
                directory = Path(root) / name
                directory.mkdir()
                (directory / "events.cbor").touch()
            calls = []

            def project(command, stdout, **kwargs):
                calls.append(command)
                if command[3] == "agent-b":
                    return subprocess_result(1)
                stdout.write(evidence([start(2, "a"), finish(3, "a", 10)]).getvalue())
                return subprocess_result(0)

            counts = collections.Counter()
            with patch.object(chart.subprocess, "run", project):
                support = chart.scan(args(agents_dir=Path(root)), [(1, 4), (1, 5)], counts)
            self.assertEqual(len(calls), 2)
            self.assertEqual(counts["failed_or_unsupported_journals"], 1)
            self.assertEqual(support[0, "accumulated", "All selected"].values, {10: 1})
            self.assertEqual(support[1, "accumulated", "All selected"].values, {10: 1})

    def test_failure_categories_preserve_decode_uncertainty_and_hide_errors(self):
        """Fixed coarse categories distinguish reader failures from new projector rejection."""
        cases = {
            b"failed to decode agent store record from /private/events.cbor: Semantic": "decode_incompatible_or_malformed_journals",
            b"failed to read agent store /private/meta.json: No such file": "checkpoint_unavailable_or_invalid_journals",
            b"unsupported agent checkpoint schema": "unsupported_checkpoint_journals",
            b"invalid agent event sequence in private-path": "framing_or_sequence_journals",
            b"agent journal not found at private-path": "missing_journal_artifact_journals",
            b"failed to open agent store private-path": "io_or_snapshot_journals",
            b"ambiguous context evidence: terminal without start": "unowned_terminal_journals",
            b"ambiguous context evidence: prompt terminal": "context_projection_rejected_journals",
            b"unknown private error body \xff": "other_trace_failure_journals",
        }
        for text, category in cases.items():
            with self.subTest(category=category):
                self.assertEqual(chart.trace_failure_category(text), category)


def subprocess_result(code):
    return type("Result", (), {"returncode": code})()


def synthetic_support():
    """Long/hostile labels, many groups, no data, missing, zeros and extreme tails."""
    result = {}
    for index in range(13):
        group = f'provider-{index:02}/very-long_model-label-with-versions-and_"quotes"_and_underscores-2026'
        for range_index in (0, 1):
            for metric in chart.METRICS:
                support = chart.Support()
                if not (range_index == 1 and index >= 8):
                    for value in ([0, 1 + index, 1 + index, 4 + index, 0, 12 + index]
                                  if metric == "compactions"
                                  else [0, 1 + index, 500 * (index + 1), 500 * (index + 1),
                                        100_000 * (index + 1), 10**12]):
                        support.add(value, [])
                    support.add(None, [])
                    support.open = 3
                result[range_index, metric, group] = support
    return result


class RenderingTests(unittest.TestCase):
    def test_default_combined_view_preserves_pairing_and_zoom_resolution(self):
        default = chart.parser().parse_args(["--out", "unused"])
        self.assertEqual(default.breakdown, "role-model")
        self.assertIsNone(default.compare_since)
        _, supports, _ = collect(
            [start(2, "a", "provider/a"), finish(3, "a", 10), compact(4),
             start(5, "b", "other/b"), finish(6, "b", 20)], selection=default)
        self.assertEqual({key[2] for key in supports},
                         {json.dumps(["engineer", model]) for model in ("a", "b")})
        self.assertTrue(all(key[1] == "segments" for key in supports))
        a = chart.group_for("custom | provider/a", {"provider/b"}, "role-model")
        b = chart.group_for("custom", {"provider/a | provider/b"}, "role-model")
        self.assertNotEqual(a, b)
        self.assertEqual(chart.group_pair(a), ("custom | provider/a", "b"))
        for model in ("provider/a", "other/b", chart.MIXED, chart.UNKNOWN):
            self.assertEqual(chart.group_style("engineer | " + model, "role-model")[1],
                             chart.group_style("engineer", "role")[1])
        models = [f"model{index}" for index in range(14)] + [chart.MIXED, chart.UNKNOWN]
        self.assertEqual(len(set(chart.model_patterns(models).values())), len(models))
        self.assertEqual(chart.model_pattern(models[0]), chart.model_pattern(models[0]))
        support = {}
        for role in ("engineer", "researcher"):
            for model in models:
                for metric in chart.METRICS:
                    unit = chart.Support()
                    unit.add(0, [])
                    unit.add(100_000 if metric != "compactions" else 2, [])
                    support[0, metric, role + " | " + model] = unit
        with tempfile.TemporaryDirectory() as temporary:
            selection = args(out=Path(temporary) / "combined", breakdown="role-model")
            chart.write_report(selection, [(1, 100)], support, collections.Counter(), synthetic=True)
            programs = list(selection.out.glob("*.gnuplot"))
            self.assertEqual(len(programs), 1)
            program = programs[0].read_text()
            self.assertIn("Roles · hues", program)
            self.assertIn("Models · family patterns, newer darker", program)
            self.assertIn("with lines lw", program)
            self.assertNotIn("linespoints", program)
            self.assertNotIn(" point pt ", program)
            styles = json.loads((selection.out / "model-styles.json").read_text())
            self.assertTrue(all(set(style) == {"family", "dash", "lightness_offset"} for style in styles.values()))
            self.assertEqual(len({style["dash"] for style in styles.values()}), len(models))
            self.assertEqual(struct.unpack(">II", programs[0].with_suffix(".png").read_bytes()[16:24]),
                             (3520, 3600))
            groups = json.loads((selection.out / "pages.txt").read_text().split(": ", 1)[1])
            self.assertEqual(len(groups), 32)

    def test_human_dates_keep_exact_report_bounds(self):
        bounds = (chart.instant("2026-09-25T00:16:39.269691Z"),
                  chart.instant("2026-10-09T00:16:39.269691Z"))
        self.assertEqual(chart.human_range(bounds), "Sep 25 – Oct 9, 2026")
        self.assertEqual(chart.human_range((chart.instant("2025-12-31T12:00:00Z"),
                                           chart.instant("2026-01-01T12:00:00Z"))),
                         "Dec 31, 2025 – Jan 1, 2026")
        self.assertEqual(chart.iso(bounds[0]), "2026-09-25T00:16:39.269691Z")

    def test_intermediate_ticks_prune_collisions_not_data(self):
        for width in (640, 1434):
            ticks = chart.ticks(1_200_000, 10_000, width)
            for text in ("0", "10k", "50k", "100k", "500k", "1M"):
                self.assertIn(chart.gp(text) + " ", ticks)
        ticks = chart.ticks(chart.U64, 10_000, 640)
        self.assertIn('"0" 0', ticks)
        self.assertLess(len(ticks.split(", ")), 30)

    def test_shifted_axes_keep_zero_and_tails(self):
        maximum, offset, upper = chart.metric_axis("accumulated", [0, 1, 10_000, 1_000_000])
        self.assertEqual((maximum, offset), (1_000_000, 10_000))
        support = chart.Support()
        for value in (0, 1, 10_000, 1_000_000):
            support.add(value, [])
        points = chart.step_data(support, offset, upper)
        self.assertEqual(points[1:3], [(0, 0), (0, 25)])
        self.assertLess(chart.math.log10(2) / upper, .16)
        self.assertEqual(points[-2][1], 100)
        self.assertGreater(upper, points[-2][0])
        self.assertIn('"10k"', chart.ticks(maximum, offset))
        self.assertIn('"1M"', chart.ticks(maximum, offset))
        self.assertNotIn('"1k"', chart.ticks(maximum, offset))
        self.assertEqual(chart.metric_axis("segments", [0, 1, 9999])[1], 0)
        self.assertEqual(chart.metric_axis("segments", [100_000, 200_000])[1], 0)
        self.assertGreater(chart.metric_axis("segments", [0, 1, chart.U64])[2],
                           chart.math.log10(1 + chart.U64 / 10_000))
        for values in ([0], [0, 1]):
            self.assertEqual(chart.metric_axis("compactions", values)[1], 0)
        self.assertEqual(chart.metric_axis("compactions", [0, 1, 17])[1], 1)
        ticks = chart.ticks(17, 1)
        self.assertIn('"0" 0', ticks)
        self.assertIn('"1"', ticks)
        self.assertIn('"2"', ticks)
        self.assertIn('"17"', ticks)

    def test_normalized_styles_match_across_providers(self):
        names = [chart.model_category({name}) for name in
                 ("chatgpt/gpt-6-astra", "chatgpt-fedi/gpt-6-astra")]
        self.assertEqual(names[0], names[1])
        self.assertEqual(chart.model_patterns(names), {"gpt-6-astra": chart.model_patterns(names[:1])["gpt-6-astra"]})

    def test_semantic_family_order_and_stable_shades(self):
        self.assertEqual(chart.role_style("coordinator")[1], 55)
        self.assertNotEqual(chart.role_style("coordinator")[1], chart.role_style("engineer")[1])
        roles = ["engineer-senior", "reviewer", "engineer", "engineer-junior",
                 "researcher-senior", "researcher", "custom-ultra", "custom-junior"]
        ordered = sorted(roles, key=lambda name: chart.group_style(name, "role")[0])
        self.assertEqual([name for name in ordered if name.startswith("engineer")],
                         ["engineer-junior", "engineer", "engineer-senior"])
        self.assertEqual([name for name in ordered if name.startswith("researcher")],
                         ["researcher", "researcher-senior"])
        styles = [chart.role_style(name) for name in
                  ("engineer-junior", "engineer", "engineer-senior")]
        self.assertEqual([style[1] for style in styles], [248, 260, 272])
        self.assertGreater(styles[0][2], styles[1][2])
        self.assertGreater(styles[1][2], styles[2][2])
        models = [f"gpt-{version}-sol" for version in ("6.10", "6.1", "5.6", "6")]
        ordered = sorted(models, key=lambda name: chart.group_style(name, "model")[0])
        self.assertEqual(ordered, [models[index] for index in (2, 3, 1, 0)])
        styles = [chart.model_style(name) for name in ordered]
        self.assertEqual(len({style[1] for style in styles}), 1)
        lightness = chart.model_lightness(ordered)
        self.assertTrue(all(lightness[a] > lightness[b] for a, b in zip(ordered, ordered[1:])))
        self.assertEqual(chart.model_style("gpt-6-sol")[1], styles[1][1])
        for breakdown in ("role", "model", "provider", "role-model"):
            names = ["custom", "provider/gpt-6-sol", "engineer | provider/gpt-6-sol",
                     "Unknown model", "Mixed models"]
            original = {name: chart.group_style(name, breakdown) for name in names}
            self.assertEqual(original, {name: chart.group_style(name, breakdown)
                                        for name in reversed(names)})

    def test_oklch_conversion_and_model_pattern_collisions(self):
        self.assertEqual(chart.oklch_hex(0, 0, 0), "#000000")
        self.assertEqual(chart.oklch_hex(1, 0, 0), "#ffffff")
        self.assertEqual(chart.oklch_hex(.5, 0, 0), "#636363")
        for hue in range(0, 360, 15):
            color = chart.oklch_hex(.58, 1, hue)
            self.assertRegex(color, r"^#[0-9a-f]{6}$")
            self.assertEqual(color, chart.oklch_hex(.58, 1, hue + 360))
        inventory = [f"model{i}" for i in range(100)]
        models = next([a, b] for i, a in enumerate(inventory) for b in inventory[i+1:]
                      if chart.model_pattern(a) == chart.model_pattern(b))
        self.assertEqual(chart.model_pattern(models[0]), chart.model_pattern(models[1]))
        patterns = chart.model_patterns(models)
        self.assertNotEqual(patterns[models[0]], patterns[models[1]])
        self.assertEqual(patterns, chart.model_patterns(list(reversed(models))))
        # Stable preferences unless another observed model conflicts.
        self.assertEqual(patterns[models[0]], chart.model_patterns(models[:1])[models[0]])
        self.assertEqual(chart.role_style("researcher-ultra")[1], 174)

    def test_model_family_patterns_and_numeric_version_brightness(self):
        models = ["gpt-6.1-sol", "gpt-5.6-sol", "gpt-6-sol",
                  "gpt-6-astra", "gpt-6.1-astra", "grok-4.7", "deepseek-flash"]
        patterns = chart.model_patterns(models)
        self.assertEqual({patterns[m] for m in models if m.endswith("-sol")}, {"(10,4)"})
        self.assertEqual({patterns[m] for m in models if m.endswith("-astra")}, {"1"})
        self.assertEqual(len(set(patterns.values())), 4)
        offsets = chart.model_lightness(models)
        self.assertEqual([offsets[m] for m in ("gpt-5.6-sol", "gpt-6-sol", "gpt-6.1-sol")],
                         [.09, 0, -.09])
        self.assertEqual(offsets["deepseek-flash"], 0)
        self.assertEqual(offsets["grok-4.7"], 0)
        self.assertEqual(offsets, chart.model_lightness(list(reversed(models))))
        # Numeric 6.10 is later/darker than 6.1, not lexical order.
        numeric = chart.model_lightness(["gpt-6.10-sol", "gpt-6.1-sol"])
        self.assertLess(numeric["gpt-6.10-sol"], numeric["gpt-6.1-sol"])
        for role in chart.STANDARD_ROLES:
            base = chart.role_style(role)[2]
            colors = [chart.group_style(json.dumps([role, m]), "role-model", offsets)[1]
                      for m in ("gpt-5.6-sol", "gpt-6-sol", "gpt-6.1-sol")]
            self.assertEqual(len(set(colors)), 3)
            self.assertEqual(colors, [chart.oklch_hex(base + offsets[m], .15, chart.role_style(role)[1])
                                     for m in ("gpt-5.6-sol", "gpt-6-sol", "gpt-6.1-sol")])

    def test_render_edges_and_empty_selection(self):
        """Real gnuplot output handles empty panels, long literals and shared paginated axes."""
        root = os.environ.get("CONTEXT_PREVIEW_DIR")
        with tempfile.TemporaryDirectory() as temporary:
            base = Path(root or temporary)
            base.mkdir(exist_ok=True, parents=True)
            selection = args(out=base / "synthetic", breakdown="model")
            ranges = [(chart.instant("2026-09-10T12:00:00Z"), chart.instant("2026-09-24T12:00:00Z")),
                      (chart.instant("2026-09-24T12:00:00Z"), chart.instant("2026-10-08T12:00:00Z"))]
            chart.write_report(selection, ranges, synthetic_support(), collections.Counter(scanned_journals=17), synthetic=True)
            programs = list(selection.out.glob("*.gnuplot"))
            self.assertEqual(len(programs), 2)
            for program in programs:
                self.assertTrue(program.with_suffix(".png").stat().st_size > 10_000)
                self.assertTrue(program.with_suffix(".svg").stat().st_size > 10_000)
                self.assertIn("noenhanced", program.read_text())
                self.assertIn("set arrow", program.read_text())
                self.assertIn("unset arrow", program.read_text())
                for noise in ("Share at or below", "missing=", "open segments=", "Selection:",
                              "Coverage:", "Created-in-range", "≤ creation", "legend n:",
                              "set label 120", "T12:00:00Z"):
                    self.assertNotIn(noise, program.read_text())
                self.assertIn("Sep 10 – Sep 24, 2026", program.read_text())
            axes = [line for line in programs[0].read_text().splitlines() if line.startswith("set xrange")]
            self.assertEqual(axes, [line for line in programs[-1].read_text().splitlines() if line.startswith("set xrange")])
            pages = (selection.out / "pages.txt").read_text().splitlines()
            self.assertTrue(all(len(json.loads(line.split(": ", 1)[1])) <= 10 for line in pages))
            self.assertIn("No measured observations", (selection.out / "context-page-02.gnuplot").read_text())
            summary = json.loads((selection.out / "summary.json").read_text())
            self.assertEqual(summary["selection"]["role_scope"], "standard")
            self.assertEqual(summary["selection"]["effective_roles"], list(chart.STANDARD_ROLES))
            self.assertEqual(selection.out.stat().st_mode & 0o777, 0o700)
            chart.write_report(args(out=base / "empty"), ranges, {}, collections.Counter(), synthetic=True)
            self.assertIn("No measured observations", (base / "empty/context-page-01.gnuplot").read_text())
            self.assertTrue((base / "empty/context-page-01.png").stat().st_size > 10_000)


if __name__ == "__main__":
    unittest.main()
