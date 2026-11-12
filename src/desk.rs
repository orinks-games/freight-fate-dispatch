//! The dispatcher's desk clock. It only moves when the player acts.

pub const SHIFT_START: u32 = 6 * 60;
pub const SHIFT_END: u32 = 18 * 60;
pub const MIN_PER_DAY: i64 = 24 * 60;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Action {
    Assign,
    TakeCall,
    Interview,
    CheckTruck,
    SalesCall,
    /// Pairing a driver tonight with tomorrow's load.
    PrePlan,
}

impl Action {
    pub const fn cost_min(self) -> u32 {
        match self {
            Action::Assign => 10,
            Action::TakeCall => 5,
            Action::Interview => 15,
            Action::CheckTruck => 2,
            Action::SalesCall => 30,
            Action::PrePlan => 5,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct DeskClock {
    /// Minutes since midnight.
    pub minute: u32,
}

impl DeskClock {
    pub fn morning() -> Self {
        DeskClock {
            minute: SHIFT_START,
        }
    }

    pub fn spend(&mut self, action: Action) {
        self.minute = (self.minute + action.cost_min()).min(SHIFT_END);
    }

    pub fn closed(&self) -> bool {
        self.minute >= SHIFT_END
    }

    pub fn spoken(&self) -> String {
        clock_text(self.minute)
    }
}

/// "6 AM", "9:35 AM", "12 PM", "6:05 PM".
pub fn clock_text(minute_of_day: u32) -> String {
    let h = (minute_of_day / 60) % 24;
    let m = minute_of_day % 60;
    let (h12, half) = match h {
        0 => (12, "AM"),
        1..=11 => (h, "AM"),
        12 => (12, "PM"),
        _ => (h - 12, "PM"),
    };
    if m == 0 {
        format!("{h12} {half}")
    } else {
        format!("{h12}:{m:02} {half}")
    }
}

/// Absolute game minute for a desk minute on a career day.
pub fn abs_min(day: i64, minute_of_day: u32) -> i64 {
    day * MIN_PER_DAY + minute_of_day as i64
}

/// "3:10 PM today", "3:10 PM tomorrow", "3:10 PM in 2 days".
pub fn abs_text(abs: i64, today: i64) -> String {
    let t = clock_text(abs.rem_euclid(MIN_PER_DAY) as u32);
    match abs.div_euclid(MIN_PER_DAY) - today {
        n if n <= 0 => format!("{t} today"),
        1 => format!("{t} tomorrow"),
        n => format!("{t} in {n} days"),
    }
}

/// A line written with `abs_text` at the close, retold the next morning:
/// "9 PM today" was last night, "5 AM tomorrow" is today, "in 2 days" is
/// tomorrow.
pub fn told_next_morning(line: &str) -> String {
    line.replace("PM today", "PM last night")
        .replace("AM today", "AM yesterday")
        .replace("AM tomorrow", "AM today")
        .replace("PM tomorrow", "PM today")
        .replace("AM in 2 days", "AM tomorrow")
        .replace("PM in 2 days", "PM tomorrow")
}

/// "2 hours", "1 hour 15 minutes", "40 minutes", to the nearest 5 minutes.
pub fn spoken_duration(minutes: f64) -> String {
    let total = ((minutes / 5.0).round() * 5.0).max(5.0) as i64;
    let (h, m) = (total / 60, total % 60);
    let hours = match h {
        0 => String::new(),
        1 => "1 hour".into(),
        n => format!("{n} hours"),
    };
    match (h, m) {
        (_, 0) => hours,
        (0, m) => format!("{m} minutes"),
        (_, m) => format!("{hours} {m} minutes"),
    }
}

/// Things due at an absolute game minute, handed out in time order.
#[derive(Clone, Debug)]
pub struct Schedule<T> {
    items: Vec<(i64, T)>,
}

impl<T> Default for Schedule<T> {
    fn default() -> Self {
        Schedule { items: Vec::new() }
    }
}

impl<T> Schedule<T> {
    pub fn push(&mut self, at: i64, item: T) {
        self.items.push((at, item));
        self.items.sort_by_key(|(t, _)| *t); // stable: equal times keep push order
    }

    pub fn next_at(&self) -> Option<i64> {
        self.items.first().map(|(t, _)| *t)
    }

    /// Everything due by `now`, with the time each was scheduled for.
    pub fn due(&mut self, now: i64) -> Vec<(i64, T)> {
        let split = self.items.partition_point(|(t, _)| *t <= now);
        self.items.drain(..split).collect()
    }

    /// Push back everything after `after` that matches, e.g. a trip's later
    /// rings when the trip itself is delayed.
    pub fn delay_where(&mut self, after: i64, minutes: i64, pred: impl Fn(&T) -> bool) {
        for (t, item) in &mut self.items {
            if *t > after && pred(item) {
                *t += minutes;
            }
        }
        self.items.sort_by_key(|(t, _)| *t);
    }

    pub fn len(&self) -> usize {
        self.items.len()
    }

    pub fn is_empty(&self) -> bool {
        self.items.is_empty()
    }

    pub fn iter(&self) -> impl Iterator<Item = &(i64, T)> {
        self.items.iter()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn action_costs_match_the_spec() {
        assert_eq!(Action::Assign.cost_min(), 10);
        assert_eq!(Action::TakeCall.cost_min(), 5);
        assert_eq!(Action::Interview.cost_min(), 15);
        assert_eq!(Action::CheckTruck.cost_min(), 2);
        assert_eq!(Action::SalesCall.cost_min(), 30);
    }

    #[test]
    fn clock_clamps_at_six_pm_and_closes() {
        let mut c = DeskClock {
            minute: SHIFT_END - 4,
        };
        assert!(!c.closed());
        c.spend(Action::Assign);
        assert_eq!(c.minute, SHIFT_END);
        assert!(c.closed());
    }

    #[test]
    fn times_read_aloud() {
        assert_eq!(clock_text(360), "6 AM");
        assert_eq!(clock_text(575), "9:35 AM");
        assert_eq!(clock_text(720), "12 PM");
        assert_eq!(clock_text(1085), "6:05 PM");
        assert_eq!(abs_text(abs_min(3, 910), 2), "3:10 PM tomorrow");
        assert_eq!(abs_text(abs_min(5, 360), 2), "6 AM in 3 days");
    }

    #[test]
    fn schedule_hands_out_due_items_in_order_and_keeps_the_rest() {
        let mut s = Schedule::default();
        s.push(50, "late");
        s.push(10, "first");
        s.push(10, "second");
        assert_eq!(s.due(20), vec![(10, "first"), (10, "second")]);
        assert_eq!(s.len(), 1);
        assert!(s.due(49).is_empty());
        assert_eq!(s.due(50), vec![(50, "late")]);
    }

    #[test]
    fn news_told_next_morning_shifts_its_days() {
        assert_eq!(
            told_next_morning("available at 5:13 AM tomorrow after a 10-hour break"),
            "available at 5:13 AM today after a 10-hour break"
        );
        assert_eq!(
            told_next_morning("delivered at 9 PM today"),
            "delivered at 9 PM last night"
        );
    }
}
