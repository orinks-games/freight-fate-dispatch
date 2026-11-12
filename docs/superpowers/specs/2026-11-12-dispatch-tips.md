# Tips: a context-sensitive tutorial for Dispatch

The lines below are as built. Where a draft line did not match the game, it was corrected; the changes are listed at the end.

Approved by Orinks on 2026-11-12. The player leads; the game never walks them through a script. A tip is one or two spoken sentences that play the first time the player reaches something.

## Rules

- A tip plays once per career, the first time its trigger happens.
- A tip is queued after everything else that key press says (`Out::Queue`, last), with a short sound in front. It never interrupts and never blocks a key.
- At most one tip per key press. If two triggers fire together, the second stays unheard and plays the next time its trigger happens.
- Every tip starts with the word "Tip:" so it is clear with sounds off.
- Tips use no random numbers and change no rules. A seed plays the same with tips on or off.
- Tips are on by default. The on or off switch is global (in `dispatch.json`, serde default on). Which tips have been heard is saved with the career.
- A save from before this feature loads with every tip marked heard, so an existing career stays quiet but can still replay them.
- Sound: use `Cue::Notify` if nothing at the desk already uses it in a way that would confuse; otherwise add a `Cue::Tip` made by the sound scripts.

## The Tips row and screen

- New desk row after Briefing: `Tips, 5 heard`. Help: `The tips you have heard, to hear again or switch off.`
- Screen title: `Tips. 5 heard, 27 still to find.` No titles are shown for tips not yet found.
- First row: `Tips: on` or `Tips: off`. Enter switches it. Help: `Tips play the first time you reach something.`
- Then one row per heard tip, in the order heard, labelled with its title. Enter says the tip. F1 says the tip too.
- Last row: `Hear all tips again`. Enter asks once: `Forget the 5 tips you have heard? Enter forgets them, Escape keeps them.` A second Enter clears the list for this career.

## Space, first week only

On days 1 to 7 with tips on, when Space finds nothing that needs the player, it adds one line naming the first desk screen they have not opened, in desk order: Tenders, Trips, Roster, Hiring, Money, Company. For example: `You haven't opened Hiring yet.` Opening a screen is tracked with the same saved state as the tips. After day 7 Space says only what it says today.

## The tips

Each line below is `id`, title, trigger, then the spoken text. Before building, check every fact against the code and help.md. If a line is wrong, fix the line to match the game and list the change in the PR. Lines marked (verify) were written from help.md alone.

### The desk

- `desk`, "The desk". First morning of a new career, after the briefing and the first row.
  Tip: F1 explains whatever you are on, and Space says what needs you now. Tips play once each. Tips, on the desk, repeats them or switches them off.
- `assign`, "After assigning". After the first load is assigned.
  Tip: Assigning took 10 minutes. Wait for what's next, on the desk, runs the clock to the next call or arrival.
- `phone`, "The phone". The first time the phone rings.
  Tip: Enter answers the phone from anywhere at the desk, and the caller waits until you do. F1 on an answer says what it costs.

### Freight

- `board`, "The load board". First time the Load board opens.
  Tip: Customer loads lead the board and carry an appointment. Enter on a load lists your drivers and whether each can legally make it.
- `spot`, "Spot freight". First time the cursor lands on a spot load.
  Tip: Spot freight comes from a broker. Its driver list ends with a counter offer, which takes 5 minutes and can lose the load.
- `tenders`, "Tenders". First time the Tenders screen opens with a tender waiting.
  Tip: A tender is a customer's load waiting for yes or no. Accept what you can cover. An accepted load nobody takes by the close is a service failure, and a tender left open counts as declined.
- `tender_call`, "Tomorrow's tenders". The first 5 PM tender call.
  Tip: These tenders are the next working day's loads. Once one is accepted, Enter on it in the Tenders screen plans its driver tonight. That truck rolls when the desk opens, with no desk time spent.
- `preplan`, "Planning ahead". First time the pre-plan driver list opens.
  Tip: A plan holds only if the driver is in, rested and legal when the desk opens. If it falls through, the morning briefing says why.
- `hot_load`, "Hot loads". The first hot load call.
  Tip: A hot load pays more on a tighter clock, and taking it adds it to the end of the board. Drop a broker's load after taking it, and that broker pays 8 percent less for a month.
- `late`, "A late load". The first load delivered late.
  Tip: A late load counts against the customer's scorecard or the broker's trust. Space names a load at risk of running late.
- `detention`, "Detention". The first detention call.
  Tip: Billing detention brings in $150, and that shipper holds your trucks more often. Waiving it costs the driver's goodwill, and the shipper holds your trucks half as often.
- `dedicated`, "Dedicated accounts". The first dedicated account call.
  Tip: A dedicated account is 10 loads a week at 5 percent over the contract rate. Under 90 percent acceptance or on time for two weeks, and the customer pulls it.

### Drivers

- `who`, "Picking a driver". First time the "Who takes it?" list opens.
  Tip: Each idle driver is read with the legal hours they have left. F1 says whether the run is legal: 11 hours of driving inside a 14-hour day, then 10 hours off.
- `short_hours`, "Short on hours". First time the cursor lands on a row that says "Short on hours".
  Tip: Short on hours means this driver can't make the appointment legally. A second Enter sends them anyway, and a scale that checks the log puts them out of service for 10 hours.
- `preference`, "What a driver asked for". First time the cursor lands on a row where the load goes against the driver's request.
  Tip: This load goes against what the driver asked for. Sending them anyway costs a little goodwill.
- `roster`, "The roster". First time the Roster opens.
  Tip: Enter on a driver finds them a load or sends them home. Their card ends with their record and the traits you have learned.
- `home`, "Home time". The first briefing that names a driver due home, but not the very first morning.
  Tip: A driver due home expects to be at the terminal tonight. On the Roster, Enter on them offers to send them home, and F1 there says how far it is.
- `trips`, "Trucks on the road". First time Trips opens with a truck rolling.
  Tip: Checking on a truck takes 2 minutes and says where it is and when it lands.
- `sick`, "Sick calls and days off". The first sick call or personal day call.
  Tip: A driver who calls in sick won't drive today whatever you answer, and asking costs goodwill. A driver you ask to give up a personal day works it, and holds it against you.
- `empty`, "Empty on the road". The first call from a driver who went empty away from the terminal.
  Tip: A driver empty on the road gets three local offers, and one of them heads back toward the terminal.
- `notice`, "A driver about to quit". The first quit warning call.
  Tip: A driver kept out past their home time calls before they quit. Sending them home wins back goodwill, and asking them to stay out loses more of it.
- `hiring`, "Hiring". First time Hiring opens.
  Tip: An interview takes 15 minutes and shows two of a driver's five traits. Hiring needs a spare truck, and the Trucks screen under Money buys one.

### The law

- `out_of_hours`, "Out of hours on the road". The first call from a driver who ran short of hours on a trip.
  Tip: A driver out of legal hours parks for 10 hours. Calling the receiver may move the appointment, and sometimes another driver can finish the run on their own hours.
- `inspection`, "Inspections". The first roadside inspection.
  Tip: A clean inspection takes a point off the carrier's safety score. A violation adds points and a fine, and over 30 points the carrier is audited.
- `speeding`, "Citations". The first speeding stop.
  Tip: A citation stays on the driver's record for 3 years and on the carrier's safety score for 2.
- `breakdown`, "Breakdowns". The first breakdown call.
  Tip: A truck past 15,000 miles since its last service breaks down three times as often. The Trucks screen, under Money, sends one to the shop for the rest of the day.
- `weekend`, "The weekend". The first Saturday briefing.
  Tip: Weekends are light. A driver near 70 hours in 8 days gets them all back with 34 hours off.

### Money and growth

- `money`, "Factoring". First time Money opens while factoring is on.
  Tip: Factoring is on, so every load pays the next day for 3 percent. With it off you keep the 3 percent, brokers pay in 30 days, and customers pay in 30 or 45.
- `trucks`, "The shop". First time Trucks opens.
  Tip: Enter on a truck, twice, sends it to the shop for the rest of the day at $600. The list ends with buying a used tractor on a note.
- `settlement`, "Friday's settlement". The first Friday briefing.
  Tip: Driver pay goes out in tonight's settlement. The Money screen says how much.
- `scorecard`, "The week's scorecard". The first Sunday briefing.
  Tip: Tonight's close reads the week's scorecard. Each customer wants 90 percent of tenders accepted and 90 percent on time. Each driver wants a week between 1,000 and 3,500 miles.
- `bills`, "The first of the month". The first morning the monthly bills are paid.
  Tip: The first of the month brings the truck notes, insurance and terminal rent. Cash can run $40,000 below zero on the line of credit. A month past that line ends the career.
- `company`, "Reputation". First time Company opens.
  Tip: Reputation sets the carrier's tier. A known carrier, at 60, gets one more spot load a day and room for another account.

## Help and tests

- Add a `## Tips` section to help.md that stands alone: what a tip is, the Tips row, how to switch them off, and how to hear them again.
- Update tests/smoke.rs and the first_shift fixture for the new desk row and the tips the script now hears.
- Tests: each trigger fires once; one tip per key press; tips off says nothing; an old save loads with all tips heard; a seeded day produces the same board and events with tips on and off; the Space line stops after day 7.
- Commit prefix: `tips:`.

## Corrections made while building

Checked against the code and help.md on 2026-11-12.

- `tender_call`: the 5 PM call brings the next working day's tenders (Friday's are for Monday), and a planned truck rolls when the desk opens, which is 10 AM on an audit day, not always 6 AM.
- `preplan`: the plan checks that the driver is in, rested and legal when the desk opens, matching the pre-plan screen's own help.
- `hot_load`: a hot load is an extra offer from a broker that pays more on a tighter clock and goes to the end of the board if taken. It is not a load that reaches the board early. The broker's trust line was dropped for length; the 8 percent for a month stands.
- `late`: Space names a trip at risk, after empty drivers and uncovered contract loads, so "while there is still time to act" was dropped.
- `who`: only an idle driver's row says hours left; a rolling driver's row says where they are.
- `home`: only a driver who goes home every night hears how far from home each load ends. For everyone, the driver card's "Send home" row says how far, in F1.
- `sick`: a sick driver doesn't drive whatever the answer, and asking costs goodwill. For a personal day, asking them to work means they do work, and hold it against you.
- `notice`: the quit-warning call comes from a driver kept out past promised home time. Two weeks' notice is a separate overnight news line for a driver whose morale is done. Sending them home is plus 5 goodwill and asking them to stay is minus 5.
- `out_of_hours`: a relief driver is offered only when one is close enough, hence "sometimes".
- `breakdown`: the shop keeps the truck for the rest of the day.
- Tip texts never say "row" (Orinks, 2026-11-12); the desk, spot, who, roster and trucks lines were reworded for that.
- The Tips screen's last row has the help line "Forgets the tips you have heard, so each plays again the next time you reach it."
