"""Speech output: Prism screen-reader/TTS when available, console fallback.

Set FF_DISPATCH_NO_SPEECH=1 (tests, headless) to skip speech entirely; spoken
lines still reach the ``ff_dispatch.transcript`` logger either way, matching
Freight Fate's transcript-first playtesting approach.
"""

from __future__ import annotations

import logging
import os

log = logging.getLogger(__name__)
transcript = logging.getLogger("ff_dispatch.transcript")


class Speech:
    def __init__(self) -> None:
        self._backend = None
        if os.environ.get("FF_DISPATCH_NO_SPEECH"):
            return
        try:
            import prismatoid

            backend = prismatoid.acquire_best()
            # acquire_best can hand back a backend that is not actually usable
            # at runtime (learned the hard way in Freight Fate).
            checker = getattr(backend, "is_supported_at_runtime", None)
            if checker is None or checker():
                self._backend = backend
        except Exception:
            log.warning("Prism speech unavailable; falling back to console", exc_info=True)

    def say(self, text: str, interrupt: bool = True) -> None:
        transcript.info("%s", text)
        if self._backend is None:
            print(text, flush=True)
            return
        try:
            if interrupt:
                self._backend.stop()
            self._backend.speak(text)
        except Exception:
            log.warning("Speech backend failed mid-say", exc_info=True)

    def stop(self) -> None:
        if self._backend is not None:
            try:
                self._backend.stop()
            except Exception:
                log.warning("Speech backend failed to stop", exc_info=True)

    def shutdown(self) -> None:
        self.stop()
