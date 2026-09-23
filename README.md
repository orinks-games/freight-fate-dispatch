# Freight Fate: Dispatch (beta)

Run the desk at a Freight Fate carrier. Assign loads, then handle what the
road throws back over the phone: hours running out, breakdowns, weather and
detention. Time only moves when you act.

## Keys

| Key | Effect |
|---|---|
| Up / Down | Move, with wraparound |
| Home / End | First / last item |
| Letter | Jump to the next item starting with it |
| Enter | Select, or answer a ringing phone |
| Escape | Back |
| F1 | Help for the current item |
| Control | Stop speech |

## Files

- Careers save at the start of each day in `%APPDATA%\FreightFateDispatch\careers`.
- Importing a Freight Fate driver only reads the Freight Fate save.
  - Dispatch looks in `%APPDATA%\FreightFate\profiles` and in a `FreightFate` folder unzipped beside this one.
  - To use another folder, set `FF_DISPATCH_FREIGHT_FATE_SAVES`.
- `FF_DISPATCH_NO_SPEECH=1` prints speech to the console instead of speaking.
- Sign-in to orinks.net is optional and uses your Freight Fate account.
