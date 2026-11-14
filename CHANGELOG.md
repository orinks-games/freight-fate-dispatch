# Changelog

What changed in each release, written for players. New entries go under Unreleased; a release moves them under its version. The release workflow publishes a version's block as its release notes.

## Unreleased

- A load a driver is short on hours for now gives you choices in place of "send them anyway": call the receiver to move the appointment, run it legal and late, or push the driver past the clock. A careful or unhappy driver refuses a push, and one who goes can report it, which puts 4 points on the carrier's safety score.

## 1.0.1 - 2026-11-14

Version 1.0.1 fixes the bugs found in a long playtest of 1.0.0. Careers from 1.0.0 and 0.1.4 open as they are.

### Drivers and home time

- A driver due home who is idle at the terminal at the close now goes home then. Before, the briefing said they were "already in town", the close said they missed promised home time, and they lost goodwill every night until they quit.
- The briefing, Space, the roster card, the day's goal and the close now agree about who goes home and when they are back.
- A driver on home time or headed home is no longer called "home time overdue".
- A driver parked far from home on a CDL suspension is no longer said to have got home as promised.
- A driver quitting is announced once, not twice.
- A clean inspection no longer teaches you that a driver is rough on the equipment.

### Freight

- When a broker answers your counter by giving the load to another carrier, you go back to the board. Before, the driver list stayed open on the next load down, and Enter sent a driver on a load you never picked.
- A tender for today closes as declined when its appointment time comes. Before, it could still be accepted after the appointment had passed.
- The three local offers for a driver who goes empty on the road are now loads that driver can haul. The same goes for the morning freight in a city where a driver is sitting.
- The dock call about a shipper who isn't ready now comes after the truck reaches the shipper, not 30 minutes after you dispatch it. The wait it quotes matches the wait you get.
- A hot load you take and leave uncovered at the close now costs you with that broker, as the tip says. The close warning names it, and taking a hot load during a close stops the close so you can cover it.
- The close says how many loads ran late and names the worst first.

### Hours of service and the law

- A run that will be late but is inside the driver's legal hours now reads "Delivers late" and says about how late. Sending it is not a violation. Before, it was treated as running past the clock and could bring an hours citation and 10 hours out of service.
- "Short on hours" now means only that the appointment can't be made without driving past the legal clock.
- A 34-hour restart takes 34 hours. Before, idle time during the day didn't count, so a restart took about 65 hours.
- A truck you send past the clock now has to take 10 hours off when it delivers. Before, it was legal again right away.
- Hours needed no longer count the 10-hour break twice when the empty miles to the shipper outrun the driver's hours.
- Hours left now match the clock. Before, each trip took about 30 minutes more off a driver's day than had passed.
- The overnight break is a full 10 hours.
- The roster card says a driver's hours left on the 70 when they are running low, and when a restart will finish. A load the 70 rules out says "Needs a 34-hour restart" on its row.
- A driver on a break says until when, not "not driving today".
- A delay from an out-of-service order is blamed on the order, not on the inspection.
- Only a new serious violation can suspend a CDL.
- The drug test and medical card news name the right day.
- Help and the tip now say the audit comes at 30 points, which is what the game does.

### Money

- The line of credit works the same for everything: hiring, buying a truck, servicing a truck and opening a terminal can all spend down to $40,000 below zero, and no further. Before, hiring and buying needed cash above zero while service had no limit.
- When cash goes past the line, the briefing says so that morning and counts down the days left every morning after. The same for a safety score of 60. Before, the only warning came 23 days in.
- The close and the Money screen now say the $40 a day each truck costs in overhead. It was always charged and never spoken.
- Sending a driver home says what the empty miles cost.

### Saves

- If the day in progress can't be read from a save, the day starts again from the morning without the loads already on trucks, and the briefing tells you. Before, those loads came back on the board and could be delivered and billed twice.
- A failed save says why in plain words.
- A finished career reads its last night's news and its ending once, with the same day count in both places.
- A career from an older version copied into a folder where this version has already saved one is listed by name and says how to bring it over. The Files section of help explains moving careers between versions.
- Online services and the Freight Fate saves folder set in 0.1.4 are kept.
- A new career never replaces an existing one's file.

### Wording

- The Sunday scorecard reads your reputation after the week's changes, and each change says whether it went up or down.
- Empty Continue, Trips and Hiring lists say what is empty.
- The Quit row asks once before closing the game.
- Smaller fixes to times, units, plurals and help lines that only repeated the row.

If you install the game in a folder Windows won't let it write to, such as Program Files, it can't save. Keep it somewhere like Documents or the desktop.
