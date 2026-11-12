# Dispatch redesign: design gaps

The four phase specs were built as written, but they left out parts of the
approved design. This pass adds the ones the game can carry today and
lists the ones it can't yet.

## Added

- **Dock terms.** Every load has seeded terms per facility: a strict
  appointment or first come, first served; drop and hook or a live load; a
  lumper fee or none. A late truck on a strict appointment is worked in
  one to three hours later and the driver sits through it. Drop and hook
  never waits at the shipper. A lumper is paid at the dock: out of a spot
  load, onto the invoice of a contract load.
- **Detention has two sides.** Billing it brings in $150 and the shipper
  holds trucks more often, up to three bills deep. Waiving it leaves the
  driver unpaid, which costs goodwill, and halves that shipper's holds
  until the next bill.
- **Pre-plan.** Tomorrow's accepted tenders are listed on the Tenders
  screen. Planning a driver takes 5 minutes tonight; at 6 AM the truck
  rolls with no desk time, or the plan falls through with a reason.
- **Counters against a hidden ceiling.** Each spot load has a ceiling 4 to
  16 percent over the posted rate. A counter comes up part of the way,
  further with a trusted broker. A counter that doesn't land leaves the
  rate alone, and sometimes loses the load.
- **Broker memory.** A dropped load costs 8 percent on that broker's
  rates for 30 days. Trust runs from minus 8 to 8 and halves each Sunday.
  A broker at 5 or more calls first with a
  hot load.
- **Driver preferences.** About four in ten drivers ask for no northeast,
  no reefer, or long runs only. Sending them against it costs goodwill.
- **Weekly miles.** Sunday night a thin week or a ragged one costs
  goodwill and a fair one earns some.
- **Broker quick pay.** With factoring off, spot loads can pay in 2 days
  for 3 percent.
- **New lanes.** A customer that grows to 4 loads a week, and again at 6,
  opens another lane.

## Not built

- Sleeper berth split and the 16-hour short-haul exception. Both need the
  trip planner in ff-core to understand them.
- Trailers as their own equipment, and a trailer pool for drop and hook.
- Regional fuel prices and fuel cards.
- Pickup windows, a wait for arriving early, and random detention at the
  receiver.
- Morale lost to a wrong ETA given to a driver.
- Check calls that turn up a problem one time in five.
- An audit that costs the whole day. It costs the morning.
- Factoring as a one-time choice. It is a switch on the Money screen.

## Balance

Sixty days on six seeds with the planner bot: deliveries about 220 against
252 before, on time 83 percent against 81, quits 8 against 7, reputation
61 against 65. The work-in wait, the lumper share and the goodwill costs
were softened to get there.
