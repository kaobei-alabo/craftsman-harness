#!/usr/bin/env python3
"""Check the accepted exec contract and its conformance vectors; no CLI execution.

    python3 -I docs/schemas/check_exec.py --self-test
    python3 -I docs/schemas/check_exec.py --result RESULT.json
    python3 -I docs/schemas/check_exec.py --events EVENTS.jsonl [--allow-prefix]

Uses the standard library and the existing trace checker's structural engine,
without changing trace-v1 code, schema or vectors. Accepted specification, not runtime enforcement.
"""
import argparse
import copy
import importlib.util
import json
import math
from pathlib import Path
import re
import sys

ROOT = Path(__file__).resolve().parent
# -I deliberately excludes the working directory and script directory from
# imports. Load only the trusted sibling by absolute path, never an ambient module.
sys.dont_write_bytecode = True
_trace_spec = importlib.util.spec_from_file_location("_exec_trace_schema", ROOT / "check_trace.py")
_trace = importlib.util.module_from_spec(_trace_spec)
_trace_spec.loader.exec_module(_trace)
Validator = _trace.Validator
calendar_problems = _trace.calendar_problems
MAX_BYTES = 16 * 1024 * 1024
U64 = 18446744073709551615
ROLES = ("main", "subagent", "compaction")
TOOLS = {"read_file", "write_file", "edit_file", "ls", "glob", "grep", "execute",
         "web_fetch", "web_search", "write_todos", "task", "send_message",
         "gadget_list", "gadget_call", "ask_user"}
WRAPPERS = {"sh", "bash", "zsh", "fish", "dash", "cmd", "cmd.exe", "powershell",
            "powershell.exe", "pwsh", "env", "sudo", "doas", "xargs", "exec"}
GIT_READ = {"status", "diff", "log", "show", "rev-parse", "ls-files"}
META_CHARS = set(";&|$`><()\n\r")


class ExecValidator(Validator):
    """Only add keywords needed by this contract; the trace validator stays intact."""
    EXTRA = {"maximum", "maxLength", "oneOf"}

    def errors(self, value, schema, path="$"):
        if isinstance(schema, bool):
            return super().errors(value, schema, path)
        # JSON Schema compares numeric values mathematically (1 and 1.0),
        # while the trace-v1 checker intentionally has stricter literal rules.
        # Normalize only the structural view; exec semantic checks below still
        # require integer literals for counters. Trace-v1 code stays unchanged.
        def numeric_view(v):
            if type(v) is float and math.isfinite(v) and v.is_integer(): return int(v)
            if isinstance(v, list): return [numeric_view(i) for i in v]
            if isinstance(v, dict): return {k: numeric_view(i) for k, i in v.items()}
            return v
        out = super().errors(numeric_view(value), {k: v for k, v in schema.items() if k not in self.EXTRA}, path)
        if isinstance(value, str) and "maxLength" in schema and len(value) > schema["maxLength"]:
            out.append(path + ": string too long")
        if type(value) in (int, float) and "maximum" in schema and value > schema["maximum"]:
            out.append(path + ": number above maximum")
        if "oneOf" in schema and sum(not self.errors(value, s, path) for s in schema["oneOf"]) != 1:
            out.append(path + ": must match exactly one alternative")
        return out


def unique_object(pairs):
    result = {}
    for key, value in pairs:
        if key in result:
            raise ValueError("duplicate JSON member")
        result[key] = value
    return result


def parse_json(raw):
    if not isinstance(raw, bytes) or len(raw) > MAX_BYTES or any(b >= 128 for b in raw):
        raise ValueError("machine JSON must be bounded ASCII")
    value = json.loads(raw.decode("ascii"), object_pairs_hook=unique_object,
                       parse_constant=lambda _: (_ for _ in ()).throw(ValueError("nonfinite JSON number")))
    def safe_tree(v):
        if type(v) is float and not math.isfinite(v): raise ValueError("nonfinite JSON number")
        if isinstance(v, str) and any(0xD800 <= ord(c) <= 0xDFFF for c in v): raise ValueError("unpaired Unicode surrogate")
        if isinstance(v, list):
            for child in v: safe_tree(child)
        if isinstance(v, dict):
            for key, child in v.items(): safe_tree(key); safe_tree(child)
    safe_tree(value)
    return value


def parse_wire(raw, target):
    if target == "result":
        return parse_json(raw)
    if not raw or not raw.endswith(b"\n") or raw.startswith(b"\xef\xbb\xbf"):
        raise ValueError("event feed must be nonempty and newline terminated")
    chunks = raw.split(b"\n")[:-1]
    if any(not c for c in chunks):
        raise ValueError("blank event line")
    if len(raw) > MAX_BYTES:
        raise ValueError("event feed exceeds validation bound")
    return [parse_json(chunk) for chunk in chunks]


def relative_file(value):
    if not value or len(value) > 4096 or value.startswith(("/", "\\")):
        return False
    if any(c in value for c in "\\:*?[]{}") or any(ord(c) < 32 or 127 <= ord(c) <= 159 for c in value):
        return False
    return all(part not in ("", ".", "..") for part in value.split("/"))


def rule_valid(rule, allow):
    if not isinstance(rule, str) or len(rule) > 4096:
        return False
    if rule.startswith("tool:"):
        name = rule[5:]
        return name in TOOLS and not (allow and name == "ask_user")
    if rule.startswith("path:"):
        value = rule[5:]
        if not relative_file(value):
            return False
        parts = [p.lower() for p in value.split("/")]
        return not (allow and any(p in {".git", ".craftsman"} or p.startswith(".env") for p in parts))
    if not rule.startswith("exec:"):
        return False
    try:
        tokens = parse_json(rule[5:].encode("ascii"))
    except (ValueError, UnicodeError):
        return False
    if not isinstance(tokens, list) or not 1 <= len(tokens) <= 16:
        return False
    if any(not isinstance(t, str) or not t or len(t) > 256 or any(c in META_CHARS or ord(c) < 32 or 127 <= ord(c) <= 159 for c in t) for t in tokens):
        return False
    if not re.fullmatch(r"[A-Za-z][A-Za-z0-9_.-]*", tokens[0]) or tokens[0].lower() in WRAPPERS:
        return False
    if tokens[0].lower() == "git":
        if len(tokens) < 2 or tokens[1] not in GIT_READ:
            return False
        forbidden = ("--out", "--git-dir", "--work-tree", "--config-env", "--exec-path", "--paginate")
        if any(t.startswith(forbidden) or t == "-C" or t.startswith("-C") or t == "-c" or t.startswith("-c") for t in tokens[2:]):
            return False
    return True


def usage_problems(usage):
    out = []
    roles = usage["roles"]
    for role in ROLES:
        row = roles[role]
        if row["requests"] != row["reported_calls"] + row["estimated_calls"]:
            out.append("usage role request classification differs")
        if row["requests"] == 0 and any(row[k] != 0 for k in ("input_tokens", "output_tokens", "unknown_attempts")):
            out.append("usage without an admitted request")
    for key in ("input_tokens", "output_tokens"):
        if usage[key] != sum(roles[r][key] for r in ROLES):
            out.append("aggregate usage differs from role ledger")
    reported = sum(roles[r]["reported_calls"] for r in ROLES)
    estimated = sum(roles[r]["estimated_calls"] for r in ROLES)
    expected = "mixed" if reported and estimated else "reported" if reported else "estimated" if estimated else "none"
    if usage["source"] != expected:
        out.append("usage source summary differs")
    if usage["billing_complete"] != (sum(roles[r]["unknown_attempts"] for r in ROLES) == 0):
        out.append("unknown attempts misreported as complete billing")
    return out


def limit_reason_valid(limit):
    """Proposed #97 clarification only; no meter, requests or usage are invented.

    Existing exhausted/overrun observations retain their original rule. The
    optional context is a distinct pre-exhaustion token-reservation refusal.
    Arbitrary-precision integer arithmetic does not wrap at the u64 boundary.
    Shape/range/integer-literal checks precede this relation in result_problems.
    """
    if "admission" not in limit:
        return limit["observed"] >= limit["maximum"]
    admission = limit["admission"]
    return (limit["kind"] == "tokens" and limit["observed"] < limit["maximum"]
            and admission["remaining_tokens"] == limit["maximum"] - limit["observed"]
            and admission["estimated_input_tokens"] + admission["requested_output_tokens"] > admission["remaining_tokens"])


def result_problems(value, validator):
    structural = validator.errors(value, {"$ref": "#/$defs/result"})
    if structural:
        return ["result structural schema violation"]
    counters = [value["exit_code"], value["rounds"], value["wall_ms"], value["messages_omitted"], value["permissions"]["denied_calls"],
                value["limits"]["max_requests"], value["usage"]["input_tokens"], value["usage"]["output_tokens"]]
    counters += [n for n in (value["limits"]["token_budget"], value["limits"]["timeout_ms"]) if n is not None]
    counters += [value["usage"]["roles"][role][key] for role in ROLES for key in
                 ("requests", "input_tokens", "output_tokens", "reported_calls", "estimated_calls", "unknown_attempts")]
    if value["_meta"]["craftsman"]["limit"] is not None:
        counters += [value["_meta"]["craftsman"]["limit"]["maximum"], value["_meta"]["craftsman"]["limit"]["observed"]]
        if "admission" in value["_meta"]["craftsman"]["limit"]:
            admission = value["_meta"]["craftsman"]["limit"]["admission"]
            counters += [admission[k] for k in ("estimated_input_tokens", "requested_output_tokens", "remaining_tokens")]
    if any(type(n) is not int for n in counters): return ["counter must be a JSON integer literal"]
    out = usage_problems(value["usage"])
    usage = value["usage"]
    requests = sum(usage["roles"][r]["requests"] for r in ROLES)
    if value["rounds"] != usage["roles"]["main"]["requests"]:
        out.append("main-round count differs")
    if (value["thread_id"] is None) != (value["session"]["mode"] == "ephemeral"):
        out.append("ephemeral/thread identity mismatch")
    if value["features"]["project_instructions"] and not value["workspace"]["trust_workspace"]:
        out.append("project instructions enabled without trust")
    if value["sandbox"]["filesystem"] != value["workspace"]["confinement"]:
        out.append("confinement metadata differs")
    policy = value["permissions"]
    if any(not rule_valid(r, True) for r in policy["allow"]) or any(not rule_valid(r, False) for r in policy["deny"]):
        out.append("invalid invocation permission rule")
    if len(policy["allow"]) != len(set(policy["allow"])) or len(policy["deny"]) != len(set(policy["deny"])):
        out.append("duplicate permission rule")
    if len(policy["env_pass"]) != len(set(policy["env_pass"])) or any(name.upper().endswith(("_API_KEY", "_TOKEN", "_SECRET", "_PASSWORD")) for name in policy["env_pass"]):
        out.append("duplicate or protected credential env pass")
    if value["trace"]["status"] == "written" and (value["trace"]["path"] is None or value["trace"]["reason"] is not None):
        out.append("written trace lacks safe identity/status")
    if value["trace"]["status"] == "disabled" and value["trace"]["reason"] is None:
        out.append("disabled trace lacks reason")
    meta = value["_meta"]["craftsman"]
    if value["error"] is not None and value["error"]["retryable"] != meta["retryable"]:
        out.append("error/meta retryability differs")
    limits = value["limits"]
    if requests > limits["max_requests"]:
        out.append("request admission exceeded shared cap")
    if value["status"] == "completed" and limits["token_budget"] is not None and usage["input_tokens"] + usage["output_tokens"] > limits["token_budget"]:
        out.append("completed result concealed token overrun")
    if value["status"] == "limit":
        limit = meta["limit"]
        expected = {"model_requests": (limits["max_requests"], requests),
                    "tokens": (limits["token_budget"], usage["input_tokens"] + usage["output_tokens"]),
                    "wall_time": (limits["timeout_ms"], value["wall_ms"])}[limit["kind"]]
        if expected[0] is None or (limit["maximum"], limit["observed"]) != expected or not limit_reason_valid(limit):
            out.append("limit kind/count/unit differs from observed ledger")
    return out


def event_problems(values, validator, allow_prefix=False):
    if not isinstance(values, list) or not values:
        return ["empty event feed"]
    if any(validator.errors(v, {"$ref": "#/$defs/event"}) or calendar_problems(v) for v in values):
        return ["event structural/calendar violation"]
    if any(type(v["seq"]) is not int for v in values): return ["sequence must be an integer literal"]
    if any(type(v["payload"][k]) is not int for v in values if v["kind"] == "model_usage" for k in ("input_tokens", "output_tokens", "unknown_attempts")):
        return ["usage delta must be an integer literal"]
    out = []
    first = values[0]
    if first["kind"] != "run_started" or first["agent_path"] != ["main"]:
        out.append("first event must start main run")
    if sum(v["kind"] == "run_started" for v in values) != 1:
        out.append("run start not unique")
    if [v["seq"] for v in values] != list(range(len(values))):
        out.append("sequence is not gap free")
    if any(v[k] != first[k] for k in ("run_id", "thread_id") for v in values):
        out.append("run/thread identity changed")
    finals = [i for i, v in enumerate(values) if v["kind"] == "result"]
    if len(finals) > 1 or (finals and finals[0] != len(values) - 1) or (not finals and not allow_prefix):
        out.append("final result must be unique and last")
    tools = {}
    denied_calls = 0
    ledger = {r: {k: 0 for k in ("requests", "input_tokens", "output_tokens", "reported_calls", "estimated_calls", "unknown_attempts")} for r in ROLES}
    for v in values:
        body = v["payload"]
        if v["kind"] in ("tool_call", "tool_call_update"):
            key = (tuple(v["agent_path"]), body["tool_call_id"])
            if v["kind"] == "tool_call":
                if key in tools: out.append("duplicate tool id in agent path")
                tools[key] = body["status"]
            else:
                before = tools.get(key)
                if before is None or before in ("completed", "failed"):
                    out.append("tool update has no live started call")
                tools[key] = body["status"]
                if body["_meta"]["craftsman"]["tool_status"] == "denied": denied_calls += 1
        elif v["kind"] == "model_usage":
            path = v["agent_path"]
            role = body["role"]
            if (role == "main" and path != ["main"]) or (role == "compaction" and path != ["main", "compaction"]) or (role == "subagent" and (len(path) < 2 or path[-1] == "compaction")):
                out.append("model role and agent path differ")
            row = ledger[role]
            row["requests"] += 1
            row[body["source"] + "_calls"] += 1
            for key in ("input_tokens", "output_tokens", "unknown_attempts"):
                row[key] += body[key]
        elif v["kind"] == "result":
            out += result_problems(body, validator)
            if any(tools[k] not in ("completed", "failed") for k in tools):
                out.append("result sealed with unfinished tool")
            if body["run_id"] != first["run_id"] or body["thread_id"] != first["thread_id"]:
                out.append("result payload identity differs")
            if any(body["usage"]["roles"][role][key] != ledger[role][key] for role in ROLES for key in ledger[role]):
                out.append("result usage differs from emitted role ledger")
            if body["permissions"]["denied_calls"] != denied_calls:
                out.append("result denial count differs from tool updates")
            started = first["payload"]
            stable = {"workspace": ("cwd_sha256", "trust_workspace", "confinement"),
                      "features": ("gadgets", "project_instructions", "skills", "subagents", "mcp", "lsp", "durable_memory", "background_tasks"),
                      "permissions": ("mode", "on_unapproved", "allow", "deny", "env_pass", "ignored_ambient_settings"),
                      "limits": ("max_requests", "token_budget", "timeout_ms"), "session": ("mode",)}
            for key, names in stable.items():
                if any(body[key][name] != started[key][name] for name in names): out.append("effective invocation settings changed")
    return out


def materialize(vector, fixtures):
    value = copy.deepcopy(fixtures[vector["seed"]])
    for patch in vector.get("patches", []):
        path = patch["path"]
        parent = value
        for part in path[:-1]: parent = parent[part]
        if patch["op"] == "set": parent[path[-1]] = copy.deepcopy(patch["value"])
        elif patch["op"] == "remove": del parent[path[-1]]
        elif patch["op"] == "append":
            target = value
            for part in path: target = target[part]
            target.append(copy.deepcopy(patch["value"]))
        else: raise ValueError("unknown conformance mutation")
    return value


def self_test(validator, vectors):
    errors = []
    for case in vectors["cases"]:
        value = materialize(case, vectors["fixtures"])
        records = value if case["target"] == "events" else [value]
        structural = all(not validator.errors(v, {"$ref": "#/$defs/" + ("event" if case["target"] == "events" else "result")}) for v in records)
        valid = not (event_problems(value, validator, case.get("allow_prefix", False)) if case["target"] == "events" else result_problems(value, validator))
        if structural != case["schema_valid"] or valid != case["valid"]: errors.append(case["name"])
    for case in vectors["rules"]:
        if rule_valid(case["rule"], case["allow"]) != case["valid"]: errors.append(case["name"])
    for case in vectors["wire"]:
        try:
            parsed = parse_wire(bytes.fromhex(case["hex"]), case["target"])
            valid = not (event_problems(parsed, validator) if case["target"] == "events" else result_problems(parsed, validator))
        except (ValueError, UnicodeError): valid = False
        if valid != case["valid"]: errors.append(case["name"])
    if errors: raise ValueError("conformance cases disagree: " + ", ".join(errors))
    print("exec contract conformance:", len(vectors["cases"]), "records/streams,", len(vectors["rules"]), "rules,", len(vectors["wire"]), "wire cases passed")


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    group = parser.add_mutually_exclusive_group(required=True)
    group.add_argument("--self-test", action="store_true")
    group.add_argument("--result", type=Path)
    group.add_argument("--events", type=Path)
    parser.add_argument("--allow-prefix", action="store_true")
    args = parser.parse_args()
    try:
        validator = ExecValidator(json.loads((ROOT / "exec-contract-v1.schema.json").read_text()))
        if args.self_test:
            self_test(validator, json.loads((ROOT / "exec-v1.vectors.json").read_text()))
        else:
            path = args.result or args.events
            with path.open("rb") as stream: raw = stream.read(MAX_BYTES + 1)
            value = parse_wire(raw, "result" if args.result else "events")
            errors = result_problems(value, validator) if args.result else event_problems(value, validator, args.allow_prefix)
            if errors: raise ValueError("contract violation: " + "; ".join(errors))
            print("valid exec contract")
        return 0
    except (OSError, ValueError, KeyError, TypeError) as error:
        # Never echo model text, malformed payloads or raw terminal controls.
        sys.stderr.write("exec contract check failed: " + json.dumps(str(error), ensure_ascii=True) + "\n")
        return 1


if __name__ == "__main__":
    raise SystemExit(main())
