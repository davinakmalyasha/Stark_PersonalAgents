# Developer cleanup benchmark

Run date: 2026-08-19  
Suite: gen2 (61) + gen3 (48) + gen4 (54) + gen7 (20) = 183 tasks  
Runner mode attempted: user prompts, parallel workers, repairs disabled for the probe

## Current cleanup run

Status: **blocked before a valid model result**.

The first task reached the Developer loop and initialized its workspace/map, but the
provider requests failed with Windows `WinError 10013` (`socket access forbidden`) for
both configured DeepSeek models. No valid current pass, token, cache, or cost metrics
were produced, so this run must not be reported as a post-cleanup benchmark result.

The runner was also corrected so future runs define the task workspace root, report
only the selected task IDs, and surface child-process errors instead of silently
aggregating stale JSON reports.

## Existing 183-task reference reports

These reports were already present in `scratch/eval_runs/` and were written on
2026-08-09 with `spec` prompts. They are a reference baseline, not a measurement of
the current cleanup run.

| Metric | Reference value |
|---|---:|
| Pass@final | 183 / 183 |
| Pass@1 | 176 / 183 |
| Total cost | $0.227475 |
| Cost / task | $0.001243 |
| Total paid calls | 1,340 |
| Input tokens | 10,153,178 |
| Cache hit | 9,093,248 |
| Cache miss | 1,059,930 |
| Cache-hit rate | 89.56% |
| Output tokens | 191,517 |
| Reasoning tokens | 10,279 |

No before/after delta is claimed because the live post-cleanup run could not reach a
model response.
