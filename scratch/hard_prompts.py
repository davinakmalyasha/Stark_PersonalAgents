"""hard_prompts.py — SWE-bench-style USER prompts for the gen2-4 suites.

Every original gen2/3/4 task prompt is replaced here by a short, ambiguous,
non-developer request — no file names, no function names, no expected values,
no fix hints. The agent must explore the project, form hypotheses, and verify,
exactly like a human bug report. The hidden tests are unchanged and remain the
ground truth (FAIL_TO_PASS semantics).

Runner usage: `scratch/eval_runner.py --style user` (keeps original prompts
under `--style spec` as an A/B baseline).
"""

USER_PROMPTS = {
    # ───────────────────────── gen2-bugfix ─────────────────────────
    "g2-fp-precision":
        "The invoice page shows totals like 0.30000000000000004 and customers are complaining the "
        "decimals look broken. Please make the amounts come out normal.",
    "g2-async-race":
        "Our job runner reports 'failed' even when one of the background jobs actually succeeds — a "
        "fast job that errors out seems to kill everything. We want the first successful job's result.",
    "g2-date-parse":
        "The date shown for an order is off by one day for some orders, depending on the time of day. "
        "It should always show the order's own date.",
    "g2-sort-tiebreak":
        "Tasks with the same priority appear in random order on the board. They should always be "
        "sorted alphabetically within the same priority.",
    "g2-unicode-escape":
        "Some special characters in customer names (like accented letters and arrows) come through "
        "raw in our exported files and break other systems. Everything non-standard should be turned "
        "into its escaped form.",
    "g2-circular-json":
        "Our app crashes when saving certain objects with a JSON.stringify error. Saving should never "
        "crash — just write something out safely.",
    "g2-nan-zero":
        "Math results occasionally show up as '-0' in the UI, which looks wrong. Negative zero should "
        "display as plain zero.",
    "g2-deep-eq":
        "Two objects that look identical are being treated as different, and it's breaking our change "
        "detection. Compare the contents, not the references.",
    "g2-neg-zero-format":
        "The money formatter prints '-0.00' for tiny negative amounts — customers see a minus sign on "
        "nothing. It should print '0.00'.",
    "g2-proto-pollution":
        "We noticed a weird 'polluted' property appearing on objects after a merge. Merging should "
        "only copy real, expected fields.",
    "g2-dedupe-order":
        "After deduplicating a list the ordering is wrong — the kept items are not the first "
        "occurrences anymore.",
    "g2-regex-dotall":
        "Code blocks that span multiple lines are being missed by our extraction — only single-line "
        "blocks come back.",
    "g2-string-codes":
        "Emoji in text are being counted as two characters, which breaks our character counts. Each "
        "emoji should count once.",
    "g2-promise-settle":
        "When one request in a batch fails, the whole batch result is lost. We want every item's "
        "outcome — successes and failures.",
    "g2-timeout-order":
        "Background jobs are running out of order and results come back shuffled. They must run one "
        "after another, in order.",

    # ───────────────────────── gen2-refactor ─────────────────────────
    "g2r-extract-helper":
        "Two functions do the exact same check but the logic is duplicated. Merge the shared logic "
        "into one helper and have both use it.",
    "g2r-rename-export":
        "We renamed this function everywhere in our codebase except here. Finish the rename so the "
        "old name is gone.",
    "g2r-split-module":
        "This file mixes formatting and math utilities. Split them into separate files but keep "
        "everything working for existing callers.",
    "g2r-loop-map":
        "The code does the same thing two different ways and the loop version is harder to read. "
        "Clean it up without changing behavior.",
    "g2r-const-config":
        "There are magic numbers in the code. Give them names so future devs know what they mean.",
    "g2r-dedup-branch":
        "The same try/catch block is copy-pasted in two functions. Extract it into one place and "
        "reuse it.",
    "g2r-extract-pure":
        "The stats function computes things inline. Break the sum and the average out into their own "
        "reusable functions.",
    "g2r-merge-modules":
        "We have two tiny modules that should really be one. Merge them into a single module and "
        "remove the old files.",
    "g2r-param-defaults":
        "Callers keep passing the same page and size arguments everywhere. Make those the defaults so "
        "calls can be shorter.",
    "g2r-template-literal":
        "The greeting is built with string concatenation, which is hard to read. Use a template "
        "literal instead.",
    "g2r-constant-hoist":
        "The same URL is repeated in several functions. Define it once at the top and reuse it.",
    "g2r-early-return":
        "The permission check is nested ifs several levels deep. Flatten it with early returns for "
        "readability.",
    "g2r-rename-param":
        "A parameter has a meaningless one-letter name. Rename it to something descriptive.",
    "g2r-switch-map":
        "The switch statement can be replaced with a simple lookup object. Clean it up.",
    "g2r-object-shorthand":
        "The object construction repeats the key names. Use shorthand syntax.",

    # ───────────────────────── gen2-python ─────────────────────────
    "g2p-mutable-default":
        "Every time the function is called without a list it keeps the previous items. Each call "
        "should start fresh.",
    "g2p-closure-late":
        "The multiplier functions all multiply by the same number. Each one should use its own "
        "multiplier.",
    "g2p-list-copy":
        "Calling the sort function changes my original list. It should only return a sorted copy.",
    "g2p-dict-default":
        "Counting crashes when a key isn't there yet. Missing keys should just start at zero.",
    "g2p-generator-next":
        "Getting the next item crashes when the generator is done. Return the default instead.",
    "g2p-float-round":
        "Money rounding is off — half values round the wrong way and decimals drift. Round half up "
        "properly.",
    "g2p-parse-int":
        "Parsing a number crashes on non-numeric input. Bad input should return -1 instead of "
        "crashing.",
    "g2p-html-escape":
        "User input with quotes and ampersands passes through unescaped — that's an XSS hole. Escape "
        "everything.",
    "g2p-scope-global":
        "The id counter crashes with an UnboundLocalError on the very first call.",
    "g2p-exc-swallow":
        "Errors are being swallowed silently — we get None for real bugs. Only missing keys should "
        "return None; real errors must surface.",
    "g2p-flatten":
        "Nested lists aren't being flattened. The function should flatten to any depth.",
    "g2p-tail-mutates":
        "Getting the tail of a list modifies the original list. It should return a copy.",
    "g2p-sort-ci":
        "Sorting is case-sensitive: 'Banana' sorts before 'apple'. Sort without case.",
    "g2p-safe-get":
        "Getting an item by index returns the wrong element — it's off by one.",
    "g2p-set-order":
        "Removing duplicates scrambles the order of the items. Keep the original order.",

    # ───────────────────────── gen2-multifile ─────────────────────────
    "g2m-counter-shared":
        "The counter never goes up no matter how many times we hit the increment endpoint. It stays "
        "at 0.",
    "g2m-circular-import":
        "Two modules reference each other, and one of them sees the other as undefined at startup.",
    "g2m-stale-config":
        "After renaming the config through the API, the read endpoint still returns the old name.",
    "g2m-repo-service":
        "The items endpoint returns an object wrapper, but clients expect a plain list.",
    "g2m-async-init":
        "The items endpoint returns null — the data layer was never initialized.",
    "g2m-event-ready":
        "The status endpoint always reports 'started: false' even though everything boots fine.",
    "g2m-utils-export":
        "Everything crashes with 'add is not a function' when the module is required.",
    "g2m-singleton-data":
        "Items added through the API disappear — listing always comes back empty.",
    "g2m-export-rename":
        "The items endpoint crashes with a 500 — it calls a method that doesn't exist.",
    "g2m-shared-cache":
        "After adding an item, the list endpoint still shows the old data until we restart.",
    "g2m-routes-split":
        "POSTing to the items endpoint 500s but GET works. The body data never arrives.",
    "g2m-global-reset":
        "Page visit counts never go above 1.",
    "g2m-relative-path":
        "The app crashes at boot with MODULE_NOT_FOUND.",
    "g2m-config-default":
        "The app always shows the fallback values instead of the real configured ones.",
    "g2m-dual-file-mutate":
        "The admin view shows stale settings — toggling dark mode doesn't show up there.",
    "g2m-gate-order":
        "Anyone can see the admin data — the permission check seems to run after the data is already "
        "sent.",

    # ───────────────────────── gen3-trap ─────────────────────────
    "g3t-two-bugs":
        "Processing users crashes for some inputs, and others come back with wrong casing. Names with "
        "surrounding spaces are the main complaint. Empty users must not crash either.",
    "g3t-decoy-comment":
        "Keys built from users change when the user's name changes. They should be stable and based "
        "on the user's identity, not their display name.",
    "g3t-wrong-file-error":
        "The name endpoint returns lowercase names; it should return them in uppercase with a "
        "prefix. The error trace points at the server file, but I suspect the imported helper is "
        "the real problem.",
    "g3t-surgical-fix":
        "Valid 3-character names are being rejected as 'too short'. Shorter names should still be "
        "rejected — just not 3-character ones.",
    "g3t-symptom-vs-cause":
        "The items list is empty on every request even though data is seeded at startup. Something "
        "wipes it during boot.",
    "g3t-misleading-name":
        "The item counter always shows the whole list instead of a number.",
    "g3t-double-inversion":
        "The permission check returns the opposite of what it should — active users are blocked and "
        "inactive ones pass. One small flip won't fix it.",
    "g3t-shadow-variable":
        "Adding items crashes with a ReferenceError about a variable used before it's ready.",
    "g3t-copy-paste":
        "The odds filter returns evens. It looks like the evens body was copied in. Fix only the "
        "odds one, don't touch the evens filter.",
    "g3t-false-positive-test":
        "Joining a list with separators produces extra empty slots where null values are. Nulls "
        "should be skipped entirely.",
    "g3t-order-trap":
        "Parsing lines with a missing field crashes. Missing values should default to zero — not "
        "crash, not be null.",
    "g3t-escape-order":
        "Escaped HTML is double-escaping ampersands — the output contains '&amp;amp;'.",
    "g3t-type-trap":
        "Looking up by a string id wrongly finds numeric ids. The string '5' must not match the "
        "number 5.",
    "g3t-greedy-regex":
        "Stripping tags removes text between tags too — everything collapses to nothing.",
    "g3t-empty-array":
        "The first-element helper returns null for empty lists, but callers expect undefined.",

    # ───────────────────────── gen3-chain ─────────────────────────
    "g3c1-1":
        "Add order creation to the API. Orders with an empty name must be rejected with an error.",
    "g3c1-2":
        "Deleting an order always fails — every id gives 404. Deleting existing orders must work.",
    "g3c1-3":
        "The order list should support a status filter, and with no filter it must show everything.",
    "g3c2-1":
        "The reset button doesn't reset anything — the count stays after pressing it.",
    "g3c2-2":
        "The counter should stop at 10. Further increments must return an error instead of going "
        "past the limit.",
    "g3c2-3":
        "The stats endpoint shows the limit as the current count. It should show the real count.",
    "g3c3-1":
        "Deduplication should ignore case — 'A' and 'a' are the same item.",
    "g3c3-2":
        "Sorting should ignore case.",
    "g3c3-3":
        "Grouping by first letter separates 'Apple' and 'banana' into different groups. Group by the "
        "lowercased first letter.",
    "g3c4-1":
        "Cached values never expire — old data keeps being served. Entries should go stale after a "
        "time.",
    "g3c4-2":
        "Invalidating the cache does nothing — old entries still come back.",
    "g3c4-3":
        "After updating the cached value, the read endpoint still returns the old value.",
    "g3c5-1":
        "Requests without a token are passing through to the admin check. The token check reads the "
        "wrong header name.",
    "g3c5-2":
        "The users endpoint only checks the token, so anyone with a token can see it. It must require "
        "admin too.",
    "g3c5-3":
        "The rate limit is global — one client's budget blocks everyone. Each client should have "
        "their own budget.",

    # ───────────────────────── gen3-log ─────────────────────────
    "g3l-undefined-prop":
        "Cart totals crash when an item has no price. A debug log is in the project with the exact "
        "error.",
    "g3l-nan-result":
        "Totals come back as NaN for currency inputs. A debug log is in the project.",
    "g3l-off-by-one":
        "Page 1 skips the first item. A debug log is in the project.",
    "g3l-mutates-input":
        "The sort helper is mutating my original array. A debug log is in the project.",
    "g3l-async-await":
        "Loading a user returns undefined. A debug log is in the project.",
    "g3l-swallowed-error":
        "A failing task silently returns null instead of failing loudly. A debug log is in the "
        "project.",
    "g3l-parse-json":
        "Bad JSON crashes the app with an unhandled error. A debug log is in the project.",
    "g3l-wrong-export":
        "Module load fails with 'not a function'. A debug log is in the project.",
    "g3l-stale-state":
        "The id counter always returns 1. A debug log is in the project.",
    "g3l-array-index":
        "Getting the nth user crashes on empty input. A debug log is in the project.",

    # ───────────────────────── gen3-long ─────────────────────────
    "g3lng-rename-everywhere":
        "We renamed a function to calculate() across the project but it wasn't finished — some files "
        "still use the old name. Finish the rename everywhere; the old name must be gone from every "
        "file.",
    "g3lng-deep-dirs":
        "The pipeline result is wrong — the sum comes back as the count. The bug is somewhere deep "
        "in the project tree.",
    "g3lng-interface-impl":
        "Four services should format identically. One of them drops the opening bracket.",
    "g3lng-pipeline":
        "The pipeline output is missing a token. One stage drops short words.",
    "g3lng-big-utils":
        "One of the twelve utility functions misbehaves: values below the minimum return the "
        "maximum. Find it and fix only it.",
    "g3lng-config-spread":
        "The app ignores the local port override and always runs on 8080. A typo somewhere in the "
        "config breaks the override.",
    "g3lng-many-routes":
        "One of the endpoints returns the wrong data — the users list returns the items list.",
    "g3lng-multi-state":
        "Increments through one endpoint never show up in another endpoint's counter. Two separate "
        "counters exist somehow.",

    # ───────────────────────── gen4-trap ─────────────────────────
    "g4t-fix-creates-bug":
        "The user lookup returns nothing when the cache is empty. It should fetch. But if you just "
        "always fetch, the cache becomes useless — the cache must be used on hits and fetch on "
        "misses.",
    "g4t-decoy-fixed":
        "Phone numbers keep their '+' sign after formatting. A comment claims it's already fixed — "
        "trust the requirement, not the comment.",
    "g4t-shadowing-state":
        "The registry count is always 1 — every call starts fresh instead of accumulating.",
    "g4t-tz-trap":
        "The day-of-week value flips around midnight. It should be timezone-independent.",
    "g4t-proto-chain":
        "Config lookups return built-in things like 'toString' as if they were configured values.",
    "g4t-cumulative-fix":
        "The balance is wrong: deposits and withdrawals are swapped, AND the sign is hidden. Both "
        "bugs must be fixed together.",
    "g4t-stateful-crash":
        "The increment helper returns NaN — something breaks the module state on every call.",
    "g4t-regex-escape":
        "Occurrence counts treat dots and plus signs as regex magic — counting '.' counts "
        "everything.",
    "g4t-soft-fail":
        "The maximum of an empty list is reported as 0 instead of undefined — a silent wrong answer, "
        "not a crash.",
    "g4t-mutate-caller":
        "Adding an item modifies my original list. It must return a new list and leave mine alone.",
    "g4t-double-apply":
        "The discount is applied twice — 10% off gives 81 instead of 90.",
    "g4t-undefined-vs-null":
        "The first-nonempty search returns null when nothing is found; callers expect undefined.",
    "g4t-index-shift":
        "The rank reported is off by one — the first item ranks 0 but callers expect 1.",
    "g4t-var-hoist":
        "All the loggers use the last prefix. Each one should use its own.",
    "g4t-float-meta":
        "The average has floating-point noise (0.15000000000000002). Round it.",

    # ───────────────────────── gen4-chain ─────────────────────────
    "g4c1-1":
        "The cart accepts invalid items — empty names and zero or bad prices go in. Reject them "
        "without adding.",
    "g4c1-2":
        "Carts over 100 should get 10% off the total. They don't.",
    "g4c1-3":
        "The total should round down after the discount. It still shows decimals.",
    "g4c2-1":
        "Deposits with zero or negative amounts are being recorded. Reject them and record nothing.",
    "g4c2-2":
        "Withdrawals can take the balance negative. Overdrafts should be rejected and not recorded.",
    "g4c2-3":
        "There's no limit on transactions. Cap it at 5 per day.",
    "g4c3-1":
        "Sessions never expire — old tokens keep working. They should go stale after 30 seconds.",
    "g4c3-2":
        "There's no way to log out. Add one so the token stops working afterwards.",
    "g4c3-3":
        "Tokens should be single-use — the second call with the same token must fail.",
    "g4c4-1":
        "The key-value store ignores TTL — old entries never expire.",
    "g4c4-2":
        "The store grows forever. Cap it at 3 entries, evicting the oldest.",
    "g4c4-3":
        "The API returns entries past their expiry. The read route should respect the 5-second "
        "expiry.",
    "g4c5-1":
        "Empty jobs can be enqueued. Reject them.",
    "g4c5-2":
        "The queue grows forever. Cap pending jobs at 3.",
    "g4c5-3":
        "Done jobs can't be retried. Add a way to put them back into the queue.",

    # ───────────────────────── gen4-log ─────────────────────────
    "g4l-multiline-trace":
        "Order processing crashes when an order has no items. A debug log is in the project.",
    "g4l-misleading-message":
        "Parsing lines crashes with an error that points at the wrong line. Lines without a second "
        "field are the trigger. A debug log is in the project.",
    "g4l-wrong-frame":
        "Render returns undefined when given a raw string. It should handle both forms. A debug log "
        "is in the project.",
    "g4l-stateful-crash":
        "Read crashes after a successful open. A debug log is in the project.",
    "g4l-race-promise":
        "Loading both values returns one of them as undefined. A debug log is in the project.",
    "g4l-soft-fail-log":
        "Discounts produce negative prices. A debug log is in the project.",
    "g4l-shadow-global":
        "All the suffix functions use the last suffix. A debug log is in the project.",
    "g4l-type-coerce":
        "Version comparison is wrong — 1.9 compares greater than 1.10. A debug log is in the "
        "project.",
    "g4l-null-guard":
        "Merging with a null argument crashes. A debug log is in the project.",
    "g4l-array-limit":
        "Taking the first n items pads with undefined when n is too big. A debug log is in the "
        "project.",
    "g4l-prototype-key":
        "Picking keys leaks prototype members like toString. A debug log is in the project.",
    "g4l-mutated-input":
        "Getting sorted keys deletes a key from the original object. A debug log is in the project.",
    "g4l-deep-nesting":
        "Flattening returns a string instead of an array. A debug log is in the project.",
    "g4l-wrong-param":
        "Formatting currency crashes when given a string. A debug log is in the project.",
    "g4l-chained-state":
        "Two counters share the same count. A debug log is in the project.",
    "g4l-sort-copy":
        "Getting the top scores sorts my original array in place. A debug log is in the project.",
    "g4l-closure-capture":
        "Greeter functions return 'Hello undefined'. A debug log is in the project.",

    # ───────────────────────── gen4-long ─────────────────────────
    "g4lng-25-modules":
        "One of the 25 pipeline modules returns its input unchanged instead of doubling it. One "
        "output is wrong.",
    "g4lng-35-modules":
        "One of the 35 modules triples with a plus-one instead. One output is wrong.",
    "g4lng-40-two-bugs":
        "Two of the 40 modules are wrong — one returns zero always, another returns 5x instead of "
        "4x. Find both.",
    "g4lng-deep-tree":
        "The pipeline result is 5 short of expected. The bug sits at the bottom of a deep folder "
        "tree.",
    "g4lng-big-single":
        "One of 40 functions in a big file subtracts instead of adds. Find it and fix only it.",
    "g4lng-feature-dirs":
        "The total is off — one of the 5 feature helpers subtracts instead of adds.",
    "g4lng-chain-links":
        "The chain result is one less than expected — one link in the middle doesn't do its step.",
}