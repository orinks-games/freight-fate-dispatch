"""The hiring office: a daily applicant pool, signing costs, and roster growth.

Applicants regenerate deterministically each career day, so sleeping on a
good one is a real decision — tomorrow's pool is different people. Signing
costs come out of the terminal's operating funds, scaling with the
experience being bought.
"""

from __future__ import annotations

import random
from dataclasses import dataclass

from .career_state import ROSTER_CAP, DispatchCareer
from .drivers import RosterDriver, generate_roster

APPLICANTS_PER_DAY = 3
SIGNING_BASE = 250.0
SIGNING_PER_LEVEL = 120.0
SIGNING_REPUTATION_FACTOR = 6.0  # dollars per reputation point above 50


def signing_cost(driver: RosterDriver) -> float:
    """What it costs to put this applicant on the board."""
    premium = max(0.0, driver.career.reputation - 50.0) * SIGNING_REPUTATION_FACTOR
    return round(SIGNING_BASE + driver.level * SIGNING_PER_LEVEL + premium, 2)


@dataclass
class Applicant:
    driver: RosterDriver
    cost: float

    def spoken(self) -> str:
        return f"{self.driver.summary()} Signing cost {self.cost:,.0f} dollars."


def applicants_for_day(career: DispatchCareer) -> list[Applicant]:
    """Today's walk-ins, deterministic per career day, minus anyone hired."""
    rng = random.Random(f"hiring:{career.seed}:{career.day}")
    pool = generate_roster(rng.randrange(2**31), count=APPLICANTS_PER_DAY)
    on_roster = {d.name for d in career.roster}
    applicants = []
    for driver in pool:
        if driver.name in on_roster:
            continue
        driver.hired_day = career.day
        applicants.append(Applicant(driver=driver, cost=signing_cost(driver)))
    return applicants


def hire(career: DispatchCareer, applicant: Applicant) -> str:
    """Sign the applicant if the funds and the roster allow; spoken outcome."""
    if len(career.roster) >= ROSTER_CAP:
        return f"The roster is full at {ROSTER_CAP} drivers. You cannot sign anyone else."
    if any(d.name == applicant.driver.name for d in career.roster):
        return f"{applicant.driver.name} is already on your roster."
    if career.funds < applicant.cost:
        return (
            f"Signing {applicant.driver.name} costs {applicant.cost:,.0f} dollars; "
            f"the account holds {career.funds:,.0f}. You cannot cover it."
        )
    career.funds = round(career.funds - applicant.cost, 2)
    career.roster.append(applicant.driver)
    return (
        f"{applicant.driver.name} signs on for {applicant.cost:,.0f} dollars. "
        f"{len(career.roster)} drivers on the roster, "
        f"{career.funds:,.0f} dollars left in the account."
    )
