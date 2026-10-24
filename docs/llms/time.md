# Grenat — time, dates and durations (`Time`, `Duration`, `freeze_time`)

Read this when a program computes with dates (a week's Monday, an overdue invoice, a schedule) or a test needs a fixed clock. It assumes the core reference, `llms.txt`.

An instant is a `Float` (epoch seconds), never an object; reading the clock needs `uses time`.

```ruby
## Whether an invoice is overdue: due 30 days after it was issued.
def overdue?(issued: String, now: Float) -> Bool
  Time.parse(issued) + 30.days < now
end

## The Monday of the week of `t`, as a date.
def monday(t: Float) -> String
  Time.date(t - (Time.weekday(t) - 1) * 1.day)
end

def today_report -> String uses time
  "#{Time.today} at #{Time.iso(Time.now)}"
end
```

- `Time.now` (`uses time`), `Time.parse("2026-09-28T08:00:00+02:00")` (ISO 8601; a date alone is midnight UTC; invalid → `ArgumentError`), `Time.at(2026, 9, 28, 8, 0, 0)`.
- Written in UTC: `Time.iso(t)` → `"2026-09-28T06:00:00Z"`, `Time.date(t)` → `"2026-09-28"`, `Time.weekday(t)` → 1 (Monday)…7 (cut to the millisecond, never rounded). `Time.today` is a `"YYYY-MM-DD"` string.
- `7.days`, `1.hour`, `2.min` are `Duration`s: `Time.now - 7.days` is a `Float`, `2.h - 30.min`, `3 * 1.h`, `1.h < 2.h`, `1.h == 3600` (a duration compares with its seconds), `d.to_i` (seconds).
- No time zones besides offsets, no `t.year` (use `Time.date`), no `strftime`.
- Cron schedules (`every cron:`) are in UTC.

## Tests

`freeze_time("2026-09-28T08:00:00Z") do … end` makes `Time.now`/`Time.today` answer that instant inside (only in a `test` block).

```ruby
test "dates" do
  assert overdue?("2026-08-01", Time.parse("2026-09-28"))
  assert_equal "2026-09-28", monday(Time.parse("2026-10-01T12:00:00Z"))
  assert_equal 7, Time.weekday(Time.parse("2026-10-04"))
  assert_equal "2026-09-28T06:00:00Z", Time.iso(Time.parse("2026-09-28T08:00:00+02:00"))
  assert_equal 5400, (2.h - 30.min).to_i
  freeze_time("2026-09-28T08:00:00Z") do
    assert_equal "2026-09-28 at 2026-09-28T08:00:00Z", today_report
  end
end
```
