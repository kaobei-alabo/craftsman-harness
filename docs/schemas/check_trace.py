#!/usr/bin/env python3
"""Validate Craftsman trace files (schema craftsman-harness/trace@1) with the Python standard library.

    python3 -I docs/schemas/check_trace.py [--schema FILE] [--lines] TRACE.jsonl ...

Each file is checked line by line against trace-v1.schema.json (this script
implements exactly the JSON Schema keywords that file uses and stops with an
error on any other keyword), then for a calendar date that exists (the
schema's pattern lets 2026-02-30 through), and then against the file-level
rules of docs/trace-schema-v1.md, section 5.6. With --lines only the per-line check
runs, for files that hold single example lines.

Exit status 0 when every file is valid, 1 otherwise. Problems go to stdout.
"""
import datetime
import json
import os
import re
import sys

ANNOTATIONS = {"$schema", "$id", "title", "description", "$defs", "examples", "$comment"}
KEYWORDS = {
    "type", "enum", "const", "pattern", "minLength", "minimum", "properties", "required",
    "items", "prefixItems", "minItems", "maxItems", "uniqueItems", "allOf", "anyOf", "not",
    "if", "then", "else", "$ref",
}


def same(a, b):
    """JSON equality: true is not 1, 1 is not 1.0."""
    if isinstance(a, bool) or isinstance(b, bool):
        return isinstance(a, bool) and isinstance(b, bool) and a == b
    if type(a) is not type(b):
        return False
    if isinstance(a, list):
        return len(a) == len(b) and all(same(x, y) for x, y in zip(a, b))
    if isinstance(a, dict):
        return a.keys() == b.keys() and all(same(a[k], b[k]) for k in a)
    return a == b


def type_ok(value, name):
    if name == "object":
        return isinstance(value, dict)
    if name == "array":
        return isinstance(value, list)
    if name == "string":
        return isinstance(value, str)
    if name == "integer":
        return isinstance(value, int) and not isinstance(value, bool)
    if name == "number":
        return isinstance(value, (int, float)) and not isinstance(value, bool)
    if name == "boolean":
        return isinstance(value, bool)
    if name == "null":
        return value is None
    raise SystemExit("unsupported type name in schema: %r" % name)


class Validator:
    def __init__(self, schema):
        self.root = schema

    def resolve(self, ref):
        if not ref.startswith("#/"):
            raise SystemExit("unsupported $ref: %r" % ref)
        node = self.root
        for part in ref[2:].split("/"):
            node = node[part]
        return node

    def errors(self, value, schema, path="$"):
        """The list of problems of `value` against `schema` (empty when valid)."""
        if schema is True:
            return []
        if schema is False:
            return ["%s: not allowed here" % path]
        out = []
        for key in schema:
            if key not in KEYWORDS and key not in ANNOTATIONS:
                raise SystemExit("schema keyword not implemented by this checker: %r" % key)
        if "$ref" in schema:
            out += self.errors(value, self.resolve(schema["$ref"]), path)
        if "type" in schema:
            names = schema["type"] if isinstance(schema["type"], list) else [schema["type"]]
            if not any(type_ok(value, n) for n in names):
                out.append("%s: expected %s, got %r" % (path, "/".join(names), value))
                return out
        if "enum" in schema and not any(same(value, e) for e in schema["enum"]):
            out.append("%s: %r is not one of %r" % (path, value, schema["enum"]))
        if "const" in schema and not same(value, schema["const"]):
            out.append("%s: expected %r, got %r" % (path, schema["const"], value))
        if isinstance(value, str):
            if "minLength" in schema and len(value) < schema["minLength"]:
                out.append("%s: shorter than %d" % (path, schema["minLength"]))
            if "pattern" in schema and not re.search(schema["pattern"], value):
                out.append("%s: %r does not match %s" % (path, value, schema["pattern"]))
        if isinstance(value, (int, float)) and not isinstance(value, bool):
            if "minimum" in schema and value < schema["minimum"]:
                out.append("%s: %r is below %r" % (path, value, schema["minimum"]))
        if isinstance(value, dict):
            for name in schema.get("required", []):
                if name not in value:
                    out.append("%s: missing %r" % (path, name))
            for name, sub in schema.get("properties", {}).items():
                if name in value:
                    out += self.errors(value[name], sub, "%s.%s" % (path, name))
        if isinstance(value, list):
            if "minItems" in schema and len(value) < schema["minItems"]:
                out.append("%s: fewer than %d items" % (path, schema["minItems"]))
            if "maxItems" in schema and len(value) > schema["maxItems"]:
                out.append("%s: more than %d items" % (path, schema["maxItems"]))
            prefix = schema.get("prefixItems", [])
            for i, sub in enumerate(prefix):
                if i < len(value):
                    out += self.errors(value[i], sub, "%s[%d]" % (path, i))
            if "items" in schema:
                for i in range(len(prefix), len(value)):
                    out += self.errors(value[i], schema["items"], "%s[%d]" % (path, i))
            if schema.get("uniqueItems"):
                for i in range(len(value)):
                    for j in range(i):
                        if same(value[i], value[j]):
                            out.append("%s: items %d and %d are equal" % (path, j, i))
        for sub in schema.get("allOf", []):
            out += self.errors(value, sub, path)
        if "anyOf" in schema and not any(not self.errors(value, s, path) for s in schema["anyOf"]):
            out.append("%s: matches none of the alternatives" % path)
        if "not" in schema and not self.errors(value, schema["not"], path):
            out.append("%s: matches a schema it must not match" % path)
        if "if" in schema:
            if not self.errors(value, schema["if"], path):
                if "then" in schema:
                    out += self.errors(value, schema["then"], path)
            elif "else" in schema:
                out += self.errors(value, schema["else"], path)
        return out


def calendar_problems(line):
    """A `ts` that has the schema's shape must also name a real date (no 30 February)."""
    ts = line.get("ts") if isinstance(line, dict) else None
    match = isinstance(ts, str) and re.match(r"^([0-9]{4})-([0-9]{2})-([0-9]{2})T", ts)
    if not match:
        return []
    try:
        datetime.date(*(int(g) for g in match.groups()))
    except ValueError:
        return ["$.ts: %r is not a calendar date" % ts]
    return []


def read_lines(path):
    """Parse a trace file; returns (lines, problems)."""
    problems = []
    raw = open(path, "rb").read()
    if raw.startswith(b"\xef\xbb\xbf"):
        problems.append("file starts with a byte order mark")
    if not raw:
        return [], ["file is empty"]
    if not raw.endswith(b"\n"):
        problems.append("last line is not newline-terminated")
    try:
        text = raw.decode("utf-8")
    except UnicodeDecodeError as error:
        return [], ["not UTF-8: %s" % error]
    lines = []
    for number, chunk in enumerate(text.split("\n")[:-1] if text.endswith("\n") else text.split("\n")):
        if chunk == "":
            problems.append("line %d is blank" % number)
            continue
        try:
            lines.append(json.loads(chunk))
        except ValueError as error:
            problems.append("line %d is not JSON: %s" % (number, error))
    return lines, problems


def file_rules(lines):
    """The file-level rules of section 5.6."""
    out = []
    if not lines:
        return ["no lines"]
    if lines[0].get("kind") != "run_start" or lines[0].get("seq") != 0:
        out.append("rule 1: the first line must be run_start with seq 0")
    if sum(1 for l in lines if l.get("kind") == "run_start") != 1:
        out.append("rule 1: there must be exactly one run_start")
    for i, line in enumerate(lines):
        if line.get("seq") != i:
            out.append("rule 2: line %d has seq %r" % (i, line.get("seq")))
            break
    for key in ("schema", "run_id", "thread_id"):
        if any(l.get(key) != lines[0].get(key) for l in lines):
            out.append("rule 3: %s differs between lines" % key)
    ends = [i for i, l in enumerate(lines) if l.get("kind") == "run_end"]
    if len(ends) > 1 or (ends and ends[0] != len(lines) - 1):
        out.append("rule 4: run_end must be unique and last")
    rounds = [l.get("index") for l in lines if l.get("kind") == "round"]
    if rounds != list(range(len(rounds))):
        out.append("rule 5: round indexes are %r" % rounds)
    if ends and lines[ends[0]].get("rounds") != len(rounds):
        out.append("rule 5: run_end.rounds is %r, there are %d round lines" % (lines[ends[0]].get("rounds"), len(rounds)))
    for l in lines:
        if l.get("kind") != "tool_call":
            continue
        where = "tool_call seq %r" % l.get("seq")
        if l["ended_ms"] < l["started_ms"]:
            out.append("rule 6: %s ends before it starts" % where)
        if l["round"] >= len(rounds):
            out.append("rule 6: %s has round %d, but there are %d round lines" % (where, l["round"], len(rounds)))
        if "hunks" in l:
            out += ["%s: %s" % (where, p) for p in hunk_rules(l)]
        if l.get("path") == "." and l["status"] not in ("error", "denied"):
            out.append("rule 9: %s targets '.' but has status %s" % (where, l["status"]))
    return out


def hunk_rules(line):
    """Rules 7 and 8 for one edit line."""
    out = []
    hunks = line["hunks"]
    before, after = line["sha256_before"], line["sha256_after"]
    if (before == after) != (not hunks):
        out.append("rule 7: hashes are %s but there are %d hunks" % ("equal" if before == after else "different", len(hunks)))
    for h, g in zip(hunks, hunks[1:]):
        old_gap = g[0] - (h[0] + h[1])
        new_gap = g[2] - (h[2] + h[3])
        if old_gap < 1 or old_gap != new_gap:
            out.append("rule 7: hunks %r and %r are not separated by the same number of common lines" % (h, g))
    if before is None and after is not None and hunks:
        if not (len(hunks) == 1 and hunks[0][:3] == [1, 0, 1] and hunks[0][3] >= 1):
            out.append("rule 8: a created file must be one hunk [1,0,1,n]")
    if before is not None and after is None:
        if not (len(hunks) == 1 and hunks[0][0] == 1 and hunks[0][2:] == [1, 0]):
            out.append("rule 8: a removed file must be one hunk [1,n,1,0]")
    return out


def main(argv):
    schema_path = os.path.join(os.path.dirname(os.path.abspath(__file__)), "trace-v1.schema.json")
    only_lines = False
    files = []
    args = list(argv)
    while args:
        arg = args.pop(0)
        if arg == "--schema":
            schema_path = args.pop(0)
        elif arg == "--lines":
            only_lines = True
        else:
            files.append(arg)
    if not files:
        print(__doc__)
        return 2
    validator = Validator(json.load(open(schema_path)))
    bad = 0
    for path in files:
        lines, problems = read_lines(path)
        for i, line in enumerate(lines):
            problems += ["line %d: %s" % (i, p) for p in validator.errors(line, validator.root)]
            problems += ["line %d: %s" % (i, p) for p in calendar_problems(line)]
        if not only_lines and not problems:
            problems += file_rules(lines)
        if problems:
            bad += 1
            print("%s: INVALID" % path)
            for p in problems[:40]:
                print("  " + p)
        else:
            print("%s: ok (%d lines)" % (path, len(lines)))
    return 1 if bad else 0


if __name__ == "__main__":
    sys.exit(main(sys.argv[1:]))
