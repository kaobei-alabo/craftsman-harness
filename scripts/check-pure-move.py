#!/usr/bin/env python3
"""Check that a Rust source split is a pure move.

Usage:
    python3 -I scripts/check-pure-move.py [--strict-layout] OLD NEW [NEW ...]

OLD is a file path, or REV:PATH to read the file from git (for example
`92ae92c:crates/craftsman-rs/src/tui_repl.rs`). Each NEW is a .rs file or a
directory searched recursively for .rs files.

Both sides are lexed (comments, strings, raw strings, byte strings, chars and
lifetimes are tracked so braces inside them do not count), split into
top-level items, and compared as multisets of item texts. Before comparing:

- visibility qualifiers (`pub`, `pub(crate)`, `pub(super)`, `pub(self)`,
  `pub(in path)`) are removed everywhere outside comments and strings;
- `use` items and `mod name;` declarations are dropped;
- inline `mod name { ... }` wrappers are dropped and their contents are
  compared as items of their own (prefixed with the module name), so a test
  module may be split across several files;
- module doc comments (`//!`, `/*! */`) are taken out of the item list and
  checked separately: every module doc line of OLD must still appear in some
  NEW file (as many times as in OLD); NEW files may add module doc lines of
  their own (each new file gets a module doc).

An item keeps its outer attributes and the comments directly before it.
Nested items (methods inside an impl, fns inside a fn) stay inside their item.

Items are compared token for token: whitespace between tokens is ignored,
and so is a trailing comma before a closing bracket of a list with more than
one element (rustfmt rewraps a signature that grew a visibility qualifier
past the line width and adds that comma); comments, string literals and every
other token must be identical. An
item whose tokens match but whose layout changed is printed as LAYOUT ONLY;
pass --strict-layout to make that a failure too.

Exits 1 and prints every item that differs or is missing on one side, else
prints a one-line summary and exits 0. Exits 2 on a usage or lexing error.
"""

import collections
import difflib
import os
import subprocess
import sys


class Tok:
    __slots__ = ("kind", "text")

    def __init__(self, kind, text):
        self.kind = kind
        self.text = text

    def __repr__(self):
        return f"Tok({self.kind!r}, {self.text!r})"


IDENT_START = set("abcdefghijklmnopqrstuvwxyzABCDEFGHIJKLMNOPQRSTUVWXYZ_")
IDENT_CHARS = IDENT_START | set("0123456789")


def is_ident_start(c):
    return c in IDENT_START or (ord(c) > 127 and c.isidentifier())


def is_ident_char(c):
    return c in IDENT_CHARS or (ord(c) > 127 and ("a" + c).isidentifier())


class LexError(Exception):
    pass


def lex(src, name):
    """Split Rust source into tokens. Never drops a character."""
    toks = []
    i = 0
    n = len(src)

    def line_of(pos):
        return src.count("\n", 0, pos) + 1

    def quoted(start, j, quote):
        # j points just after the opening quote; returns index after closing.
        while j < n:
            c = src[j]
            if c == "\\":
                j += 2
                continue
            if c == quote:
                return j + 1
            j += 1
        raise LexError(f"{name}:{line_of(start)}: unterminated literal")

    def raw(start, j):
        # j points at the first '#' or '"' after the r prefix.
        hashes = 0
        while j < n and src[j] == "#":
            hashes += 1
            j += 1
        if j >= n or src[j] != '"':
            return None
        close = '"' + "#" * hashes
        end = src.find(close, j + 1)
        if end < 0:
            raise LexError(f"{name}:{line_of(start)}: unterminated raw string")
        return end + len(close)

    while i < n:
        c = src[i]
        start = i
        if c.isspace():
            while i < n and src[i].isspace():
                i += 1
            toks.append(Tok("ws", src[start:i]))
        elif src.startswith("//", i):
            end = src.find("\n", i)
            i = n if end < 0 else end
            toks.append(Tok("comment", src[start:i]))
        elif src.startswith("/*", i):
            depth = 0
            while i < n:
                if src.startswith("/*", i):
                    depth += 1
                    i += 2
                elif src.startswith("*/", i):
                    depth -= 1
                    i += 2
                    if depth == 0:
                        break
                else:
                    i += 1
            if depth:
                raise LexError(f"{name}:{line_of(start)}: unterminated block comment")
            toks.append(Tok("comment", src[start:i]))
        elif c == '"':
            i = quoted(start, i + 1, '"')
            toks.append(Tok("str", src[start:i]))
        elif c == "'":
            if i + 1 < n and src[i + 1] == "\\":
                i = quoted(start, i + 1, "'")
                toks.append(Tok("char", src[start:i]))
            elif i + 2 < n and src[i + 2] == "'":
                i += 3
                toks.append(Tok("char", src[start:i]))
            else:
                i += 1
                while i < n and is_ident_char(src[i]):
                    i += 1
                toks.append(Tok("lifetime", src[start:i]))
        elif is_ident_start(c):
            # String-literal prefixes: b"", br"", r"", r#""#, c"", cr"", b''.
            j = i
            while j < n and src[j] in "bcr" and j - i < 2:
                j += 1
            prefix = src[i:j]
            done = False
            for k in range(len(prefix), 0, -1):
                p = prefix[:k]
                e = i + k
                if p in ("r", "br", "cr") and e < n and src[e] in '#"':
                    end = raw(start, e)
                    if end is not None:
                        i = end
                        toks.append(Tok("str", src[start:i]))
                        done = True
                        break
                if p in ("b", "c") and e < n and src[e] == '"':
                    i = quoted(start, e + 1, '"')
                    toks.append(Tok("str", src[start:i]))
                    done = True
                    break
                if p == "b" and e < n and src[e] == "'":
                    i = quoted(start, e + 1, "'")
                    toks.append(Tok("char", src[start:i]))
                    done = True
                    break
            if done:
                continue
            if src.startswith("r#", i) and i + 2 < n and is_ident_start(src[i + 2]):
                i += 2
            while i < n and is_ident_char(src[i]):
                i += 1
            toks.append(Tok("ident", src[start:i]))
        elif c.isdigit():
            while i < n and (is_ident_char(src[i]) or (
                src[i] == "." and i + 1 < n and src[i + 1].isdigit()
            )):
                i += 1
            toks.append(Tok("num", src[start:i]))
        else:
            i += 1
            toks.append(Tok("punct", c))
    if "".join(t.text for t in toks) != src:
        raise LexError(f"{name}: lexer lost characters")
    return toks


OPEN = {"(": ")", "[": "]", "{": "}"}
CLOSE = {")", "]", "}"}
SEMI_ENDED = {"use", "const", "static", "type", "let", "extern"}


def significant(toks, i):
    while i < len(toks) and toks[i].kind in ("ws", "comment"):
        i += 1
    return i


def strip_visibility(toks):
    """Return a token list with visibility qualifiers (and the whitespace
    after them) removed."""
    out = []
    i = 0
    n = len(toks)
    while i < n:
        t = toks[i]
        if t.kind == "ident" and t.text == "pub":
            j = i + 1
            k = significant(toks, j)
            if k < n and toks[k].text == "(":
                m = significant(toks, k + 1)
                if m < n and toks[m].kind == "ident" and toks[m].text in (
                    "crate", "self", "super", "in"
                ):
                    depth = 0
                    p = k
                    while p < n:
                        if toks[p].text == "(":
                            depth += 1
                        elif toks[p].text == ")":
                            depth -= 1
                            if depth == 0:
                                break
                        p += 1
                    j = p + 1
            if j < n and toks[j].kind == "ws":
                j += 1
            i = j
            continue
        out.append(t)
        i += 1
    return out


def split_items(toks, name, prefix="", docs=None):
    """Split a token list into (label, text) items; recurse into inline mods.
    Module doc comments found between items are appended to `docs`, one entry
    per line, as (module prefix, stripped line)."""
    if docs is None:
        docs = []
    items = []
    headers = []
    i = 0
    n = len(toks)
    while True:
        # Skip whitespace and module docs between items.
        while i < n and (
            toks[i].kind == "ws"
            or (toks[i].kind == "comment" and toks[i].text.startswith(("//!", "/*!")))
        ):
            if toks[i].kind == "comment":
                docs.extend((prefix, line.strip()) for line in toks[i].text.splitlines())
            i += 1
        if i >= n:
            break
        start = i
        # Leading comments and attributes.
        j = i
        while True:
            j = significant(toks, j)
            if j < n and toks[j].text == "#":
                k = significant(toks, j + 1)
                if k < n and toks[k].text == "!":
                    k = significant(toks, k + 1)
                if k < n and toks[k].text == "[":
                    j = match(toks, k, name) + 1
                    continue
            break
        body = j
        if body >= n:
            items.append((prefix + "<trailing comments>", render(toks[start:n])))
            break
        # Item keyword: skip qualifiers.
        k = body
        words = []
        while k < n and len(words) < 6:
            k = significant(toks, k)
            if k >= n:
                break
            if toks[k].kind == "ident":
                words.append(toks[k].text)
                if toks[k].text in ("unsafe", "async", "default"):
                    k += 1
                    continue
                if toks[k].text == "extern":
                    k += 1
                    k2 = significant(toks, k)
                    if k2 < n and toks[k2].kind == "str":
                        k = k2 + 1
                    continue
                if toks[k].text == "const":
                    k2 = significant(toks, k + 1)
                    if k2 < n and toks[k2].kind == "ident" and toks[k2].text in (
                        "fn", "unsafe", "async", "extern"
                    ):
                        k = k2
                        continue
                break
            break
        kw = words[-1] if words else ""
        first = words[0] if words else ""
        semi_only = (first in SEMI_ENDED and kw != "fn") or kw == "crate"
        is_use = first == "use"
        # Find the end of the item.
        end = None
        p = k
        while p < n:
            t = toks[p]
            if t.text in OPEN and t.kind == "punct":
                close = match(toks, p, name)
                if t.text == "{" and not semi_only:
                    end = close
                    break
                p = close + 1
                continue
            if t.kind == "punct" and t.text == ";":
                end = p
                break
            if t.kind == "punct" and t.text in CLOSE:
                raise LexError(f"{name}: unbalanced {t.text!r} at top level")
            p += 1
        if end is None:
            raise LexError(f"{name}: item without end near {render(toks[body:body+8])!r}")
        item = toks[start:end + 1]
        label = header_label(toks[body:end + 1])
        if is_use:
            pass
        elif kw == "mod":
            # `mod x;` is dropped; `mod x { ... }` is flattened.
            if toks[end].text == "}":
                brace = next(q for q in range(body, end + 1) if toks[q].text == "{")
                modname = label.split()[-1].rstrip("{").strip()
                headers.append(prefix + render(toks[start:brace]).strip())
                sub, subheaders, _ = split_items(
                    toks[brace + 1:end], name, prefix + modname + "::", docs
                )
                items.extend(sub)
                headers.extend(subheaders)
        else:
            items.append((prefix + label, render(item)))
        i = end + 1
    return items, headers, docs


def match(toks, p, name):
    """Index of the bracket that closes toks[p]."""
    want = [OPEN[toks[p].text]]
    q = p + 1
    while q < len(toks):
        t = toks[q]
        if t.kind == "punct":
            if t.text in OPEN:
                want.append(OPEN[t.text])
            elif t.text in CLOSE:
                if t.text != want[-1]:
                    raise LexError(f"{name}: mismatched {t.text!r}")
                want.pop()
                if not want:
                    return q
        q += 1
    raise LexError(f"{name}: unclosed {toks[p].text!r}")


def render(toks):
    return "".join(t.text for t in toks)


def header_label(toks):
    out = []
    for t in toks:
        if t.kind == "punct" and t.text in ("{", ";", "="):
            break
        if t.kind == "comment":
            continue
        out.append(t.text)
    return " ".join("".join(out).split())


def read_old(spec):
    if os.path.exists(spec):
        with open(spec, encoding="utf-8") as f:
            return spec, f.read()
    if ":" in spec:
        out = subprocess.run(
            ["git", "show", spec], capture_output=True, check=False
        )
        if out.returncode != 0:
            sys.exit(f"git show {spec} failed: {out.stderr.decode(errors='replace').strip()}")
        return spec, out.stdout.decode("utf-8")
    sys.exit(f"no such file: {spec}")


def new_files(paths):
    files = []
    for p in paths:
        if os.path.isdir(p):
            for root, _, names in os.walk(p):
                files.extend(os.path.join(root, f) for f in names if f.endswith(".rs"))
        elif os.path.isfile(p):
            files.append(p)
        else:
            sys.exit(f"no such file or directory: {p}")
    return sorted(files)


def items_of(name, src):
    toks = strip_visibility(lex(src, name))
    return split_items(toks, name)


def token_key(name, text):
    """An item's tokens with whitespace dropped. Comments, strings and every
    other token are kept exactly, except a trailing comma before a closing
    bracket in a list that has another comma at the same depth (rustfmt adds
    one when it wraps a list; a one-element tuple `(x,)` keeps its comma)."""
    toks = [t for t in lex(name=name, src=text) if t.kind != "ws"]
    commas = []  # per open bracket: number of commas seen at its depth
    drop = set()
    for i, t in enumerate(toks):
        if t.kind != "punct":
            continue
        if t.text in OPEN:
            commas.append(0)
        elif t.text in CLOSE:
            n = commas.pop() if commas else 0
            j = i - 1
            while j >= 0 and toks[j].kind == "comment":
                j -= 1
            if j >= 0 and toks[j].kind == "punct" and toks[j].text == "," and n >= 2:
                drop.add(j)
        elif t.text == "," and commas:
            commas[-1] += 1
    return tuple(t.text for i, t in enumerate(toks) if i not in drop)


def main(argv):
    args = [a for a in argv[1:] if a != "--strict-layout"]
    strict = len(args) != len(argv) - 1
    if len(args) < 2 or args[0] in ("-h", "--help"):
        print(__doc__.strip())
        return 2
    try:
        old_name, old_src = read_old(args[0])
        old_split = items_of(old_name, old_src)
        old_items = [
            (label, token_key(old_name, text), text, old_name)
            for label, text in old_split[0]
        ]
        old_docs = collections.Counter(old_split[2])
        new_docs = collections.Counter()
        paths = new_files(args[1:])
        new_items = []
        for path in paths:
            with open(path, encoding="utf-8") as f:
                its, _, docs = items_of(path, f.read())
            new_docs.update(docs)
            new_items.extend((label, token_key(path, text), text, path) for label, text in its)
    except LexError as e:
        print(f"error: {e}", file=sys.stderr)
        return 2

    old_c = collections.Counter((label, key) for label, key, _, _ in old_items)
    new_c = collections.Counter((label, key) for label, key, _, _ in new_items)
    only_old = old_c - new_c
    only_new = new_c - old_c
    lost_docs = old_docs - new_docs
    for (prefix, line), cnt in sorted(lost_docs.items()):
        print(f"MODULE DOC line missing in new{' (in mod ' + prefix + ')' if prefix else ''}: {line!r} (x{cnt})")

    # Items whose tokens match but whose layout (whitespace) differs.
    layout = []
    old_texts = collections.defaultdict(list)
    for label, key, text, _ in old_items:
        old_texts[(label, key)].append(text)
    for label, key, text, path in new_items:
        texts = old_texts.get((label, key))
        if texts and text not in texts:
            layout.append((label, path, texts[0], text))

    def show_diff(a, b):
        for line in difflib.unified_diff(
            a.splitlines(), b.splitlines(), "old", "new", lineterm="", n=2
        ):
            print("    " + line)

    for label, path, a, b in layout:
        print(f"LAYOUT ONLY (same tokens, rewrapped): {label} in {path}")
        show_diff(a, b)

    if not only_old and not only_new and not lost_docs:
        if layout and strict:
            print(f"not a pure move (--strict-layout): {len(layout)} item(s) changed layout")
            return 1
        print(
            f"pure move: all {sum(old_c.values())} items of {old_name} match "
            f"the {len(paths)} new file(s) token for token"
            + (f" ({len(layout)} with layout-only changes)" if layout else "")
            + f"; all {sum(old_docs.values())} module doc line(s) kept"
        )
        return 0

    old_text_of = {}
    for label, key, text, path in old_items:
        old_text_of.setdefault((label, key), (text, path))
    new_text_of = collections.defaultdict(list)
    for label, key, text, path in new_items:
        new_text_of[(label, key)].append((text, path))
    new_labels = {label for label, _ in new_c}
    old_labels = {label for label, _ in old_c}
    unmatched_new = collections.defaultdict(list)
    for (label, key), cnt in only_new.items():
        unmatched_new[label].append(key)
    shown = set()
    for (label, key), cnt in sorted(only_old.items()):
        if unmatched_new.get(label):
            other = unmatched_new[label].pop(0)
            shown.add((label, other))
            print(f"DIFFERS: {label} (in {new_text_of[(label, other)][0][1]})")
            show_diff(old_text_of[(label, key)][0], new_text_of[(label, other)][0][0])
        elif label in new_labels:
            print(f"COUNT differs: {label} has {cnt} fewer cop(y/ies) in new")
        else:
            print(f"MISSING in new: {label} (x{cnt})")
    for (label, key), cnt in sorted(only_new.items()):
        if (label, key) in shown:
            continue
        where = ", ".join(sorted({path for _, path in new_text_of[(label, key)]}))
        if label in old_labels:
            print(f"EXTRA copy in new: {label} (x{cnt}) in {where}")
        else:
            print(f"ONLY in new: {label} (x{cnt}) in {where}")
    print(
        f"not a pure move: {sum(only_old.values())} old item(s) and "
        f"{sum(only_new.values())} new item(s) without a match, "
        f"{sum(lost_docs.values())} module doc line(s) lost"
    )
    return 1


if __name__ == "__main__":
    sys.exit(main(sys.argv))
