# Dispatch redesign, phase 2: drivers and the law

Date: 2026-11-11. The second of four phases from the redesign proposal.
Drivers become people with records, and the law works the way it does in
trucking, measured in game days.

## What changes for the player

- **Traits.** Five hidden traits per driver, 1 to 5: reliability (call-offs),
  pace (speed against the plan, and speeding stops), care (inspection
  results), communication (6 AM check calls) and patience (how much goodwill
  it takes before morale moves). A delivery reveals one with 20 percent odds,
  40 on the 90-day probation; a clean inspection reveals care, a speeding
  stop reveals pace, an interview reveals two.
- **Morale.** Goodwill is spoken as solid, fine, grumbling or done. Done
  gives two weeks' notice; back to fine cancels it. On Sundays a driver at
  80 goodwill or more may refer a friend, who leads the applicant pool with
  two known traits and a clean year.
- **Hiring.** Applications carry the MVR and PSP record (citations in 3
  years, inspections and how many were clean), seeded from the applicant's
  care and pace. Hired drivers start a 90-day probation.
- **Check calls.** The briefing's "still out" lines become 6 AM check calls
  with the truck's position and timing; a driver with low communication
  doesn't call.
- **Hours.** Not having the hours for an appointment is a warning, not a
  refusal. Sending the driver anyway runs the load straight through, marks
  the trip over hours, and makes a scale far more likely to stop it.
- **Inspections.** Each trip rolls an inspection from a base of 8 percent,
  moved by care, the safety score and running over hours. Level 1, 2 or 3
  (60, 30, 20 minutes). Clean credits the score; a violation (hours,
  logbook or equipment) cites the driver, adds points, and fines the driver
  for moving and log violations or the carrier for equipment. Hours at a
  scale is 10 hours out of service; equipment sometimes holds the truck 3
  hours and $450.
- **Speeding.** Pace over 3 draws traffic stops; one in three is serious.
- **Records and the score.** Citations last 3 years on the driver and 2 on
  the carrier (half weight in the second year) across seven categories. Two
  serious in 3 years suspends the CDL 60 days, three 120; the driver parks
  when the load delivers. Medical cards last 2 years (a day to renew). One
  random drug test a quarter (a day). Over 30 points: an audit (the desk
  opens at 10 AM, $2,000 if hours citations are on the record) and customers
  tender a quarter less.

## Model

- `law.rs`: `Violation`, `Category`, `Citation`, `DriverRecord`, `SafetyScore`, constants, `seeded_record`.
- `roster.rs`: `Traits`, `known`, `record`, `hired_day`, `leaving_day`, `morale()`, `reveal*`, `pace_factor()`.
- `incidents.rs`: `Kind::Inspection { level }`, `Kind::Speeding`, `Choice::Acknowledge`, `enforce`.
- `board.rs`: `hours_warning`, `refusal_or_warning`; `Trip.over_hours`, `push_past_hours`.
- `Game` gains `safety`, `referrals`, `audit_pending`; `law_overnight` and `morale_overnight` run at the close.

## Out of scope, later phases

The sleeper split and short-haul exception as planning options, insurance
priced by the safety score (phase 3), crashes and the hazmat category
having sources, a driver's pay while suspended.
