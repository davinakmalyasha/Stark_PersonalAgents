# Personal OS

Personal OS is the local-first record layer behind the General agent's everyday
work: money, todos, food/calories, and training. It is one JSON document and one
tool. No bank connection, no cloud account, no background sync.

## Where the data lives

```
<harness>/.deep_agents/personal_os.json
```

- One document, written atomically (temp file + rename).
- Money is stored as integer cents; dates as `YYYY-MM-DD`; months as `YYYY-MM`.
- If the file is corrupt, the tool refuses to write and reports the path. It
  never silently resets the store.
- Back up by copying that file. Runtime state under `.deep_agents/` is ignored
  by git.

Agents never edit the file directly. They call `personal_os`; the tool keeps
ids, totals, and budgets consistent.

## Actions

| Action | Purpose |
|---|---|
| `overview` | Today's snapshot: month totals, budgets, open/overdue todos, today's calories, 7-day training minutes |
| `money_add` | Record an expense or income (`amount`, `kind`, `category`, `note`, `date`) |
| `money_list` | List transactions for a month, optionally filtered by category |
| `money_summary` | Expenses/income/net, per-category totals, budget remaining |
| `money_budget_set` | Set a monthly category budget |
| `todo_add` | Add a todo with optional due date and priority |
| `todo_list` | List open, done, or all todos |
| `todo_done` | Complete a todo by id (`todo-3` or `3`) |
| `todo_remove` | Remove a todo by id |
| `food_log` | Log calories for a meal |
| `food_summary` | Show one day's food and calorie total |
| `training_log` | Log an activity and duration |
| `training_summary` | Show sessions and total minutes over a lookback window |

## Money policy

`personal_os` is bookkeeping only:

- It records what the user reports. It does not connect to a bank.
- It never initiates a transfer or payment and never asks for card or bank
  credentials.
- A budget is a target, not an enforcement mechanism. The summary shows
  remaining, not a block.
- If the user asks to move money, the agent hands that step back to the user.

## Scope access

| Scope | Access |
|---|---|
| `general` | Full surface: reads and writes |
| `house` | Read-only household context (`overview`, `todo_list`, `food_summary`, `training_summary`); never money |
| Everything else | No `personal_os` |

The boundary is enforced twice: `scope_map` in the frozen contract controls
advertising, and `read_only_denial` in `stark-tools` blocks write actions and
House money reads at dispatch.

## Examples

```
personal_os(action="money_add", amount=12.50, category="food", note="lunch")
personal_os(action="money_summary", month="2026-11")
personal_os(action="todo_add", text="Renew passport", due="2026-12-01", priority="high")
personal_os(action="food_log", name="Oatmeal", calories=450)
personal_os(action="training_log", activity="Run", minutes=30)
personal_os(action="overview")
```
