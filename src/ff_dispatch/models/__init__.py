"""Dispatch game models: rosters, shifts, careers, calls, and hiring."""

from .career_state import ROSTER_CAP, DispatchCareer, ServiceRecord
from .drivers import (
    ActiveRun,
    RosterDriver,
    driver_from_payload,
    driver_from_record,
    driver_payload,
    generate_roster,
)
from .hiring import Applicant, applicants_for_day, hire, signing_cost
from .phone import CallSchedule, DispatcherCall, apply_choice, calls_for_day
from .shift import AssignmentOutcome, Departure, DispatchShift, ShiftSummary

__all__ = [
    "ROSTER_CAP",
    "ActiveRun",
    "Applicant",
    "AssignmentOutcome",
    "CallSchedule",
    "Departure",
    "DispatchCareer",
    "DispatchShift",
    "DispatcherCall",
    "RosterDriver",
    "ServiceRecord",
    "ShiftSummary",
    "applicants_for_day",
    "apply_choice",
    "calls_for_day",
    "driver_from_payload",
    "driver_from_record",
    "driver_payload",
    "generate_roster",
    "hire",
    "signing_cost",
]
