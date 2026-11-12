//! Tips: one or two sentences the first time the player reaches something.
//! The player leads; a tip never interrupts, never blocks a key, uses no
//! random numbers and changes no rules.

use serde::{Deserialize, Serialize};

/// The desk screens the first week's Space line points to, in desk order,
/// by the name the desk row starts with.
pub const DESK_SCREENS: [&str; 6] = ["Tenders", "Trips", "Roster", "Hiring", "Money", "Company"];

/// Space adds the unopened-screen line through this career day.
pub const FIRST_WEEK_DAYS: i64 = 7;

macro_rules! tips {
    ($($name:ident, $id:literal, $title:literal, $text:literal;)*) => {
        #[derive(Clone, Copy, Debug, PartialEq, Eq)]
        pub enum Tip {
            $($name,)*
        }

        impl Tip {
            pub const ALL: &'static [Tip] = &[$(Tip::$name,)*];

            /// The id kept in the save.
            pub const fn id(self) -> &'static str {
                match self {
                    $(Tip::$name => $id,)*
                }
            }

            /// The row label on the Tips screen.
            pub const fn title(self) -> &'static str {
                match self {
                    $(Tip::$name => $title,)*
                }
            }

            /// What is said, starting with "Tip:".
            pub const fn text(self) -> &'static str {
                match self {
                    $(Tip::$name => $text,)*
                }
            }
        }
    };
}

tips! {
    // The desk.
    Desk, "desk", "The desk",
        "Tip: F1 explains whatever you are on, and Space says what needs you now. Tips play once each. Tips, on the desk, repeats them or switches them off.";
    Assign, "assign", "After assigning",
        "Tip: Assigning took 10 minutes. Wait for what's next, on the desk, runs the clock to the next call or arrival.";
    Phone, "phone", "The phone",
        "Tip: Enter answers the phone from anywhere at the desk, and the caller waits until you do. F1 on an answer says what it costs.";
    // Freight.
    Board, "board", "The load board",
        "Tip: Customer loads lead the board and carry an appointment. Enter on a load lists your drivers and whether each can legally make it.";
    Spot, "spot", "Spot freight",
        "Tip: Spot freight comes from a broker. Its driver list ends with a counter offer, which takes 5 minutes and can lose the load.";
    Tenders, "tenders", "Tenders",
        "Tip: A tender is a customer's load waiting for yes or no. Accept what you can cover. An accepted load nobody takes by the close is a service failure, and a tender left open counts as declined.";
    TenderCall, "tender_call", "Tomorrow's tenders",
        "Tip: These tenders are the next working day's loads. Once one is accepted, Enter on it in the Tenders screen plans its driver tonight. That truck rolls when the desk opens, with no desk time spent.";
    PrePlan, "preplan", "Planning ahead",
        "Tip: A plan holds only if the driver is in, rested and legal when the desk opens. If it falls through, the morning briefing says why.";
    HotLoad, "hot_load", "Hot loads",
        "Tip: A hot load pays more on a tighter clock, and taking it adds it to the end of the board. Drop a broker's load after taking it, and that broker pays 8 percent less for a month.";
    Late, "late", "A late load",
        "Tip: A late load counts against the customer's scorecard or the broker's trust. Space names a load at risk of running late.";
    Detention, "detention", "Detention",
        "Tip: Billing detention brings in $150, and that shipper holds your trucks more often. Waiving it costs the driver's goodwill, and the shipper holds your trucks half as often.";
    Dedicated, "dedicated", "Dedicated accounts",
        "Tip: A dedicated account is 10 loads a week at 5 percent over the contract rate. Under 90 percent acceptance or on time for two weeks, and the customer pulls it.";
    // Drivers.
    Who, "who", "Picking a driver",
        "Tip: Each idle driver is read with the legal hours they have left. F1 says whether the run is legal: 11 hours of driving inside a 14-hour day, then 10 hours off.";
    ShortHours, "short_hours", "Short on hours",
        "Tip: Short on hours means this driver can't make the appointment legally. A second Enter sends them anyway, and a scale that checks the log puts them out of service for 10 hours.";
    Preference, "preference", "What a driver asked for",
        "Tip: This load goes against what the driver asked for. Sending them anyway costs a little goodwill.";
    Roster, "roster", "The roster",
        "Tip: Enter on a driver finds them a load or sends them home. Their card ends with their record and the traits you have learned.";
    Home, "home", "Home time",
        "Tip: A driver due home expects to be at the terminal tonight. On the Roster, Enter on them offers to send them home, and F1 there says how far it is.";
    Trips, "trips", "Trucks on the road",
        "Tip: Checking on a truck takes 2 minutes and says where it is and when it lands.";
    Sick, "sick", "Sick calls and days off",
        "Tip: A driver who calls in sick won't drive today whatever you answer, and asking costs goodwill. A driver you ask to give up a personal day works it, and holds it against you.";
    Empty, "empty", "Empty on the road",
        "Tip: A driver empty on the road gets three local offers, and one of them heads back toward the terminal.";
    Notice, "notice", "A driver about to quit",
        "Tip: A driver kept out past their home time calls before they quit. Sending them home wins back goodwill, and asking them to stay out loses more of it.";
    Hiring, "hiring", "Hiring",
        "Tip: An interview takes 15 minutes and shows two of a driver's five traits. Hiring needs a spare truck, and the Trucks screen under Money buys one.";
    // The law.
    OutOfHours, "out_of_hours", "Out of hours on the road",
        "Tip: A driver out of legal hours parks for 10 hours. Calling the receiver may move the appointment, and sometimes another driver can finish the run on their own hours.";
    Inspection, "inspection", "Inspections",
        "Tip: A clean inspection takes a point off the carrier's safety score. A violation adds points and a fine, and over 30 points the carrier is audited.";
    Speeding, "speeding", "Citations",
        "Tip: A citation stays on the driver's record for 3 years and on the carrier's safety score for 2.";
    Breakdown, "breakdown", "Breakdowns",
        "Tip: A truck past 15,000 miles since its last service breaks down three times as often. The Trucks screen, under Money, sends one to the shop for the rest of the day.";
    Weekend, "weekend", "The weekend",
        "Tip: Weekends are light. A driver near 70 hours in 8 days gets them all back with 34 hours off.";
    // Money and growth.
    Money, "money", "Factoring",
        "Tip: Factoring is on, so every load pays the next day for 3 percent. With it off you keep the 3 percent, brokers pay in 30 days, and customers pay in 30 or 45.";
    Trucks, "trucks", "The shop",
        "Tip: Enter on a truck, twice, sends it to the shop for the rest of the day at $600. The list ends with buying a used tractor on a note.";
    Settlement, "settlement", "Friday's settlement",
        "Tip: Driver pay goes out in tonight's settlement. The Money screen says how much.";
    Scorecard, "scorecard", "The week's scorecard",
        "Tip: Tonight's close reads the week's scorecard. Each customer wants 90 percent of tenders accepted and 90 percent on time. Each driver wants a week between 1,000 and 3,500 miles.";
    Bills, "bills", "The first of the month",
        "Tip: The first of the month brings the truck notes, insurance and terminal rent. Cash can run $40,000 below zero on the line of credit. A month past that line ends the career.";
    Company, "company", "Reputation",
        "Tip: Reputation sets the carrier's tier. A known carrier, at 60, gets one more spot load a day and room for another account.";
}

impl Tip {
    pub fn from_id(id: &str) -> Option<Tip> {
        Tip::ALL.iter().copied().find(|t| t.id() == id)
    }
}

/// What this career has heard and opened. Saved with the career; a save
/// from before tips loads with everything heard, so it stays quiet.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct TipLog {
    /// Tip ids, in the order heard.
    #[serde(default)]
    pub heard: Vec<String>,
    /// Desk screens opened, by `DESK_SCREENS` name.
    #[serde(default)]
    pub opened: Vec<String>,
}

impl TipLog {
    /// A career saved before tips: every tip heard, every screen opened.
    pub fn old_save() -> TipLog {
        TipLog {
            heard: Tip::ALL.iter().map(|t| t.id().to_string()).collect(),
            opened: DESK_SCREENS.iter().map(|s| s.to_string()).collect(),
        }
    }

    pub fn has_heard(&self, tip: Tip) -> bool {
        self.heard.iter().any(|id| id == tip.id())
    }

    /// Mark a tip heard; false if it already was.
    pub fn hear(&mut self, tip: Tip) -> bool {
        if self.has_heard(tip) {
            return false;
        }
        self.heard.push(tip.id().to_string());
        true
    }

    /// The tips heard, in the order heard. An id this build doesn't know
    /// is kept in the save but not listed.
    pub fn heard_tips(&self) -> Vec<Tip> {
        self.heard
            .iter()
            .filter_map(|id| Tip::from_id(id))
            .collect()
    }

    pub fn forget(&mut self) {
        self.heard.clear();
    }

    pub fn open(&mut self, screen: &str) {
        if DESK_SCREENS.contains(&screen) && !self.opened.iter().any(|s| s == screen) {
            self.opened.push(screen.to_string());
        }
    }

    /// The first desk screen, in desk order, not opened yet.
    pub fn first_unopened(&self) -> Option<&'static str> {
        DESK_SCREENS
            .iter()
            .copied()
            .find(|s| !self.opened.iter().any(|o| o == s))
    }
}

/// The Space line for the first week: the first screen not opened yet.
pub fn unopened_line(log: &TipLog, day: i64) -> Option<String> {
    if day > FIRST_WEEK_DAYS {
        return None;
    }
    log.first_unopened()
        .map(|s| format!("You haven't opened {s} yet."))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_tip_has_a_unique_id_and_starts_with_tip() {
        let mut ids: Vec<&str> = Tip::ALL.iter().map(|t| t.id()).collect();
        ids.sort();
        ids.dedup();
        assert_eq!(ids.len(), Tip::ALL.len());
        for t in Tip::ALL {
            assert!(t.text().starts_with("Tip: "), "{t:?}");
            assert_eq!(Tip::from_id(t.id()), Some(*t));
            for bad in ["--", "(", ")", "%", "..", "  "] {
                assert!(!t.text().contains(bad), "{t:?} has {bad:?}");
            }
            // A tip names what the player is on, never "row".
            assert!(
                !t.text()
                    .to_lowercase()
                    .split(|c: char| !c.is_alphabetic())
                    .any(|w| w == "row" || w == "rows"),
                "{t:?} says row"
            );
        }
    }

    #[test]
    fn a_tip_is_heard_once_and_listed_in_order() {
        let mut log = TipLog::default();
        assert!(log.hear(Tip::Board));
        assert!(log.hear(Tip::Desk));
        assert!(!log.hear(Tip::Board));
        assert_eq!(log.heard_tips(), vec![Tip::Board, Tip::Desk]);
        log.forget();
        assert!(log.heard_tips().is_empty());
    }

    #[test]
    fn an_old_save_has_heard_everything() {
        let log = TipLog::old_save();
        assert!(Tip::ALL.iter().all(|t| log.has_heard(*t)));
        assert_eq!(log.first_unopened(), None);
    }

    #[test]
    fn the_space_line_names_the_first_unopened_screen_in_the_first_week() {
        let mut log = TipLog::default();
        assert_eq!(
            unopened_line(&log, 1).as_deref(),
            Some("You haven't opened Tenders yet.")
        );
        log.open("Tenders");
        log.open("Roster");
        assert_eq!(
            unopened_line(&log, 7).as_deref(),
            Some("You haven't opened Trips yet.")
        );
        assert_eq!(unopened_line(&log, 8), None);
    }
}
