#!/usr/bin/env python3
"""A quote's search over runs of consecutive lines, as the pipeline's citation locator does it: each
run's text is joined once, and a quote is found with one search per run."""
from bisect import bisect_left, bisect_right


class Run:
    """Consecutive lines with their text joined by single spaces, and where each line starts in it."""

    def __init__(self, lines):
        self.lines, self.starts, parts, at = lines, [], [], 0
        for line in lines:
            self.starts.append(at)
            parts.append(line["text"])
            at += len(line["text"]) + 1
        self.text = " ".join(parts)
        self.page = lines[0]["page"]


def shortest_window(runs, needle, max_window):
    """The fewest consecutive lines of one run holding `needle`, the first run's on a tie. Matches
    may overlap: a later one can sit on fewer lines."""
    best = None
    for run in runs:
        at = run.text.find(needle)
        while at != -1:
            first = bisect_right(run.starts, at) - 1
            last = bisect_left(run.starts, at + len(needle)) - 1
            if last - first < max_window and (best is None or last - first + 1 < len(best)):
                best = run.lines[first:last + 1]
            if best is not None and len(best) == 1:
                return best
            at = run.text.find(needle, at + 1)
    return best
