# Functions roadmap

Where formulas go next. The end point: every kind of formula, window functions included, works on
blends as on models. On the way, the average person sees a short list of plain-worded calculations;
the harder functions sit under **Advanced**, one click away for analysts.

## Rules that hold at every step

- **No SQL from people.** Every new function is parsed (`formula.rs`, `results.rs`) and written per
  engine by `compiler::Dialect`, with a test per dialect (ScramDB, PostgreSQL, BigQuery, DuckDB).
- **Row filters come first.** A formula only ever sees the rows its caller may see; nothing here
  touches `access::row_filters_for` or `access::same_data`.
- **Everything as code.** Each function works in the app, in YAML (models, queries, shared
  formulas) and through MCP (`check_formula`, `save_formula`, `run_query`), with `?dry_run=1`.
- **Plain words.** Each function has a one-line help in everyday language, an example, and an
  error that says where and why ("`year` needs a date, and `amount` is a number").

## Where we are (0.1.10)

| Kind | Runs | Functions | On blends |
|---|---|---|---|
| On totals | in the query | `sum avg min max count count_distinct`, `if coalesce round abs least greatest`, arithmetic, comparisons | no |
| Per row | in the query | the same, plus `lower upper concat`; no dates | no |
| On a chart's results | on ScramDB, as window functions | `running_sum moving_avg previous rank percent_of_total` | **yes** |

Gaps: no date functions; little text handling; results formulas can't filter or set a Top N; the
editor shows every function as an equal chip, in the same list for everyone.

## Step 1: Easy first, Advanced one click away (the editor) — done

The formula editor gets two layers.

**Quick calculations** (no typing). Next to a measure in Explore, a "Calculate…" menu in plain words:

- *Share of total* (`percent_of_total`), *Change from previous* (`x - previous(x)`),
  *% change from previous*, *Running total* (`running_sum`), *Rank* (`rank`),
  *Average of last 3* (`moving_avg`), *Divided by…* (another of the chart's measures).
  (Rounding is left to the measure's format.) Comparing rows needs a dimension: with none, only
  dividing is offered. Tested end to end on ScramDB (`api::formulas::the_quick_calculations_run`).

Each adds a formula with a readable name ("Revenue, share of total") that people can open and
change, so they learn the language by seeing it.

**The editor**, its functions grouped by what they do, each group short:

- *Numbers* (`round abs least greatest`), *Logic* (`if coalesce`), *Text*, *Dates* (step 2),
  *Compared with other rows* (the results functions).
- **Advanced ▸** (closed by default, remembered per person): the harder functions and options
  from later steps (window options, conditional totals, patterns, percentiles). Today:
  `coalesce`, `least`, `greatest`.

Who sees Advanced: everyone who may explore, hidden by default.

## Step 2: Dates (per row) — done

Easy: `year(d) quarter(d) month(d) week(d) day(d) weekday(d) hour(d)`, `today()`,
`days_between(a, b)`.
Advanced: `date_trunc(d, 'month')`, `date_add(d, 3, 'day')`, `date_diff(a, b, 'month')` (calendar
days, months or years).

How it went:
- Weeks are ISO (from Monday), weekdays 1 (Monday) to 7, on every engine (BigQuery counts from
  Sunday: translated). `date_add` takes a whole number; a BigQuery timestamp adds months through
  its date-time.
- `today()` is written into the query as today's UTC date: DuckDB's sandbox has no ICU (so no
  `CURRENT_DATE`), and a cached result is never yesterday's today.
- One table of answers (`formula::tests::DATES`) runs on ScramDB, PostgreSQL and in DuckDB's
  sandbox; BigQuery is checked as SQL. `api::formulas::dates_in_fields_of_each_row` uses them as
  fields end to end.
- Left for later: date literals (`'2026-01-01'` compared with a date), `format_date` (patterns
  differ per engine; `year`/`month` with `concat` cover most of it), dates in conditions on totals
  (with step 4's `sum_if`).

## Step 3: Text and choices — done

Easy: `contains(t, 'x')`, `starts_with`, `ends_with` (capitals aside, as the contains filter; no
wildcards), `length`, `trim`, `left(t, n)`, `right(t, n)`, `replace(t, 'a', 'b')`,
`in(x, 'a', 'b', 'c')`.
Advanced: `case(x, 'a', 1, 'b', 2, else)`.

Tested as dates are: one table of answers (`formula::tests::TEXTS`) on ScramDB, PostgreSQL and in
DuckDB's sandbox, BigQuery as SQL, and `api::formulas::text_and_choices_in_fields_of_each_row`
end to end.

Not yet: **patterns** (`matches(t, 'pattern')`, `extract(t, 'pattern')`). A pattern people type
would run on each engine's own regular expressions; PostgreSQL's (so ScramDB's) can take
exponential time on some patterns, held back only by the statement timeout. To do safely: a
pattern language of our own (parsed like formulas, so only safe constructs reach the engine), or
patterns only on engines with linear-time matching (DuckDB and BigQuery use RE2).

## Step 4: Smarter totals

Easy: `sum_if(amount, region = 'EMEA')`, `count_if(returned)`, `avg_if`.
Advanced: `median(x)`, `percentile(x, 0.9)` (exact where the engine has it, else refused rather
than approximated silently), `share_of(revenue, region = 'EMEA')`.

## Step 5: Results formulas grow

- Filter and Top N on a results formula (a second pass on ScramDB, after the window functions).
- Easy: `next`, `difference(x)`, `percent_change(x)`, `running_avg`, `dense_rank`.
- Advanced: say along what and within what, instead of the first date / the others:
  `running_sum(revenue, along: month, within: region)`, `moving_avg(x, 7, along: day)`,
  `first(x)`, `last(x)`, `ntile(x, 4)`, `rank(x, within: category)`.
  In the editor, Advanced shows these as two pickers (*along*, *within*), not syntax to learn.

## Step 6: Formulas on blends (the goal)

Blends aggregate per engine, then join on ScramDB. Each kind of formula goes where that design
puts it:

1. **Per row, on one side**: pushed down into that side's own query, on its own engine, before it
   aggregates (`amount * qty` on the warehouse side). Same functions as on a model.
2. **On totals, across sides**: run on ScramDB on the joined, aggregated result:
   `revenue / rainy_days`, `sum_if` on a side's measure, ratios between engines.
3. **Window functions, with every Advanced option**, on the blend's result: the results functions
   already run there; this adds *along* / *within* over any of the blend's dimensions, filtering
   and Top N on them, and saving them as a blend's own shared formulas.

Never: copying raw rows across engines to make a formula work (the blends rule).

## Later, to decide

- Fixed-level totals ("revenue per region, whatever the chart shows"): powerful, hard to explain;
  Advanced only, if at all.
- Formulas on date columns' grains in model YAML (a model's own "fiscal year").

## Order

1 → 2 → 3 → 4 → 5 → 6. Steps 1 and 2 give the average person the most; step 5 is the groundwork
for step 6.
