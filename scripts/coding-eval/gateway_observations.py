"""Bounded private gateway facts, never request/response bodies or authority."""
import hashlib
import json
import threading


class ObservationFailure(ValueError):
    pass


class GatewayObservations:
    PHASES = {"attempted", "admitted", "refused", "provider_headers", "provider_eof",
              "unknown_usage", "settled", "transport_error", "accounting_error"}
    REASONS = {"request_limit", "spending_cap", "request_invalid", "accounting_closed", "other_refusal"}

    def __init__(self, stream, maximum=4096):
        if type(maximum) is not int or not 1 <= maximum <= 4096:
            raise ObservationFailure("invalid_observation_bound")
        self.stream = stream
        self.maximum = maximum
        self.sequence = 0
        self.closed = False
        self.lock = threading.Lock()

    def record(self, phase, run, ordinal=None, status=None, reason=None, terminal=None):
        if phase not in self.PHASES or not isinstance(run, str) or len(run.encode()) > 100:
            raise ObservationFailure("invalid_observation")
        if ordinal is not None and (type(ordinal) is not int or not 0 <= ordinal < 10000):
            raise ObservationFailure("invalid_observation")
        if status is not None and (type(status) is not int or not 100 <= status <= 599):
            raise ObservationFailure("invalid_observation")
        if reason is not None and reason not in self.REASONS:
            raise ObservationFailure("invalid_observation")
        if terminal is not None and terminal not in {"response.completed", "response.incomplete", "response.failed", "error"}:
            raise ObservationFailure("invalid_observation")
        with self.lock:
            if self.closed or self.sequence >= self.maximum:
                self.closed = True
                raise ObservationFailure("observation_closed")
            value = {"kind": "private_gateway_observation", "version": 1, "sequence": self.sequence,
                     "run_sha256": hashlib.sha256(run.encode()).hexdigest(), "phase": phase,
                     "ordinal": ordinal, "http_status": status, "reason": reason, "terminal_event": terminal}
            encoded = json.dumps(value, separators=(",", ":")) + "\n"
            if len(encoded.encode()) > 512:
                self.closed = True
                raise ObservationFailure("observation_too_large")
            try:
                self.stream.write(encoded)
                self.stream.flush()
            except (OSError, ValueError):
                self.closed = True
                raise ObservationFailure("observation_io_failed") from None
            self.sequence += 1


def refusal_reason(error):
    return {"Evaluation run reached its request limit.": "request_limit",
            "Evaluation reached its spending cap.": "spending_cap",
            "Evaluation accounting is closed.": "accounting_closed"}.get(str(error), "other_refusal")
