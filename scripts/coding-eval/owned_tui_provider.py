"""Marked synthetic Responses provider; controls are outside the agent mount."""
import sys

if __name__ == "__main__" and sys.argv[1:] != ["--owned-tui-provider-v1"]:
    raise SystemExit(0)

import hashlib
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer
import json
import threading


def encoded(value):
    return json.dumps(value, ensure_ascii=False, separators=(",", ":")).encode()


def events(items, ordinal):
    rid = "owned-fixture-" + str(ordinal)
    rows = [{"type": "response.created", "response": {"id": rid, "status": "in_progress", "model": "gpt-6.1-sol"}}]
    completed = []
    for index, item in enumerate(items):
        added = {**item, "status": "in_progress"}
        if item["type"] == "function_call":
            added["arguments"] = ""
        else:
            added["content"] = []
        rows.append({"type": "response.output_item.added", "output_index": index, "item": added})
        if item["type"] == "function_call":
            rows.extend([
                {"type": "response.function_call_arguments.delta", "item_id": item["id"], "output_index": index, "delta": item["arguments"]},
                {"type": "response.function_call_arguments.done", "item_id": item["id"], "output_index": index, "arguments": item["arguments"]}])
        else:
            text = item["content"][0]["text"]
            rows.extend([
                {"type": "response.content_part.added", "item_id": item["id"], "output_index": index, "content_index": 0, "part": {"type": "output_text", "text": ""}},
                {"type": "response.output_text.delta", "item_id": item["id"], "output_index": index, "content_index": 0, "delta": text},
                {"type": "response.content_part.done", "item_id": item["id"], "output_index": index, "content_index": 0, "part": item["content"][0]}])
        item = {**item, "status": "completed"}
        completed.append(item)
        rows.append({"type": "response.output_item.done", "output_index": index, "item": item})
    rows.append({"type": "response.completed", "response": {"id": rid, "status": "completed",
        "incomplete_details": None, "output": completed,
        "usage": {"input_tokens": 100, "output_tokens": 50, "total_tokens": 150}}})
    return b"".join(b"data: " + encoded(row) + b"\n\n" for row in rows)


class Engine:
    def __init__(self, bootstrap):
        self.bootstrap = bootstrap
        self.phase = "original"
        self.phase_requests = 0
        self.requests = 0
        self.release = threading.Event()
        self.lock = threading.Lock()
        self.output_lock = threading.Lock()

    def emit(self, kind, **facts):
        value = {"kind": kind, "version": 1, **facts}
        data = encoded(value)
        if len(data) > 4096:
            raise ValueError("fixture_observation_bound")
        with self.output_lock:
            sys.stdout.buffer.write(data + b"\n")
            sys.stdout.buffer.flush()

    def control(self, value):
        if value.get("op") == "phase" and value.get("phase") in {"original", "setup", "update", "compact", "pending", "eof", "final", "duplicate", "reread"}:
            with self.lock:
                self.phase = value["phase"]
                self.phase_requests = 0
                self.expected_input_sha256 = value.get("input_sha256")
                self.release.clear()
            self.emit("fixture_phase_selected", phase=self.phase)
        elif value.get("op") == "release":
            self.release.set()
            self.emit("fixture_pending_release")
        else:
            raise ValueError("fixture_control_refused")

    def response(self, body, raw):
        if body.get("model") != "gpt-6.1-sol" or body.get("max_output_tokens") != 8192:
            raise ValueError("fixture_settings_mismatch")
        if body.get("reasoning", {}).get("effort") != "xhigh":
            raise ValueError("fixture_effort_mismatch")
        with self.lock:
            phase, index, ordinal = self.phase, self.phase_requests, self.requests
            self.phase_requests += 1
            self.requests += 1
        if ordinal >= 24:
            raise ValueError("fixture_request_bound")
        self.emit("fixture_request_observed", ordinal=ordinal, phase=phase, phase_index=index,
                  request_bytes=len(raw), request_sha256=hashlib.sha256(raw).hexdigest(),
                  observed_model=body["model"], observed_effort=body["reasoning"]["effort"],
                  observed_output_ceiling=body["max_output_tokens"])
        scenario = self.bootstrap["scenario"]
        if phase in {"pending", "eof"}:
            return phase, ordinal, b'data: {"type":"response.created","response":{"id":"owned-pending","status":"in_progress","model":"gpt-6.1-sol"}}\n\n'
        text_view = json.dumps(body, ensure_ascii=False)
        if phase == "compact":
            if "C1" not in text_view or "replace_none" not in text_view:
                raise ValueError("fixture_compaction_input_missing")
            self.emit("fixture_history_verified", ordinal=ordinal, phase=phase)
            items = [{"type": "message", "id": "compact-summary", "role": "assistant", "content": [{"type": "output_text", "text": "Completed implementation and checks. Early contract: checkpoint C1, mode careful, merge_policy replace_none. Preserve handoff and completed effects; do not repeat tools."}]}]
        elif phase == "update":
            items = [{"type": "message", "id": "update-" + str(ordinal), "role": "assistant", "phase": "final", "content": [{"type": "output_text", "text": "Update accepted."}]}]
        else:
            actions = self.bootstrap["actions"][phase]
            if index == 0:
                if phase == "final":
                    required = "created I1" if scenario == "interruption-reopen-v1" else "[exit code 7]" if scenario == "retained-results-v1" else "C1"
                    if required not in text_view:
                        raise ValueError("fixture_retained_history_missing")
                    self.emit("fixture_history_verified", ordinal=ordinal, phase=phase)
                items = [{"type": "function_call", "id": "item-" + call["id"], "call_id": call["id"],
                          "name": call["name"], "arguments": json.dumps(call["args"], ensure_ascii=False)} for call in actions]
            elif index == 1:
                if not all(call["id"] in text_view for call in actions):
                    raise ValueError("fixture_actual_results_missing")
                if phase in {"duplicate", "reread"} or phase == "setup" and scenario == "retained-results-v1":
                    call = actions[0]
                    matches = []
                    def walk(value):
                        if isinstance(value, dict):
                            if value.get("type") == "function_call_output" and value.get("call_id") == call["id"]:
                                matches.append(value.get("output"))
                            for child in value.values():walk(child)
                        elif isinstance(value, list):
                            for child in value:walk(child)
                    walk(body)
                    if len(matches) != 1 or not isinstance(matches[0], str):
                        raise ValueError("fixture_specific_negative_result_missing")
                    output = matches[0]
                    if phase == "setup":
                        expected = self.bootstrap["native_expected_result_sha256"]
                        if hashlib.sha256(output.encode()).hexdigest() != expected:
                            raise ValueError("fixture_native_relay_result_mismatch")
                        self.emit("fixture_native_relay_result_verified", ordinal=ordinal, phase=phase, phase_index=index,
                            call_sha256=hashlib.sha256(json.dumps(call, sort_keys=True, ensure_ascii=False,
                                separators=(",", ":"), allow_nan=False).encode()).hexdigest(),
                            output_sha256=hashlib.sha256(output.encode()).hexdigest())
                        condition = None
                    elif phase == "duplicate":
                        if "FileExistsError" not in output or "[exit code 1]" not in output:
                            raise ValueError("fixture_duplicate_refusal_missing")
                        condition = "recorded_file_exists_refusal"
                    else:
                        if "changed after the original observation" not in output:
                            raise ValueError("fixture_current_read_result_missing")
                        condition = "recorded_current_probe_read"
                    if condition is not None:
                        self.emit("fixture_negative_result_verified", ordinal=ordinal, phase=phase, phase_index=index,
                                  call_sha256=hashlib.sha256(json.dumps(call, sort_keys=True, ensure_ascii=False,
                                      separators=(",", ":"), allow_nan=False).encode()).hexdigest(),
                                  output_sha256=hashlib.sha256(output.encode()).hexdigest(), condition=condition)
                self.emit("fixture_results_observed", ordinal=ordinal, phase=phase,
                          call_ids_sha256=hashlib.sha256(encoded([call["id"] for call in actions])).hexdigest())
                items = [{"type": "message", "id": "returned-" + str(ordinal), "role": "assistant", "phase": "final", "content": [{"type": "output_text", "text": "Fixture turn returned after the requested tools."}]}]
            else:
                raise ValueError("fixture_unexpected_extra_sampling")
        return "complete", ordinal, events(items, ordinal)


def main():
    raw = sys.stdin.buffer.readline(1024 * 1024 + 1)
    if len(raw) > 1024 * 1024:
        raise ValueError("fixture_bootstrap_bound")
    engine = Engine(json.loads(raw))

    class Handler(BaseHTTPRequestHandler):
        protocol_version = "HTTP/1.1"
        def log_message(self, *_):
            pass
        def do_POST(self):
            try:
                if self.path != "/v1/responses" or self.headers.get("Authorization") != "Bearer synthetic-provider-key":
                    raise ValueError("fixture_route_refused")
                length = int(self.headers.get("Content-Length", "0"))
                if not 0 < length <= 262144:
                    raise ValueError("fixture_request_bound")
                self.connection.settimeout(120)
                raw = self.rfile.read(length)
                if len(raw) != length:
                    raise ValueError("fixture_short_request")
                mode, ordinal, payload = engine.response(json.loads(raw), raw)
                self.send_response(200)
                self.send_header("Content-Type", "text/event-stream")
                self.send_header("Connection", "close")
                self.end_headers()
                self.close_connection = True
                self.wfile.write(payload)
                self.wfile.flush()
                if mode == "pending":
                    engine.emit("fixture_pending_observed", ordinal=ordinal)
                    engine.release.wait(120)
                if mode in {"pending", "eof"}:
                    engine.emit("fixture_stream_returning_without_terminal", ordinal=ordinal)
            except (ValueError, OSError, KeyError, TypeError):
                engine.emit("fixture_request_failed")
                self.close_connection = True

    server = ThreadingHTTPServer(("0.0.0.0", 9876), Handler)
    server.daemon_threads = True
    thread = threading.Thread(target=server.serve_forever, daemon=True)
    thread.start()
    engine.emit("fixture_ready")
    for raw in sys.stdin.buffer:
        if len(raw) > 4096:
            raise ValueError("fixture_control_bound")
        engine.control(json.loads(raw))
    engine.release.set()
    server.shutdown()
    server.server_close()


if __name__ == "__main__":
    try:
        main()
    except (ValueError, OSError, KeyError, TypeError):
        raise SystemExit("Synthetic provider refused fixed control.")
