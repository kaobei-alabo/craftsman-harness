# OSS review: text inherited from deepagents

> **Status:** prepared on 2026-10-16 (UTC) and revised on 2026-10-17 (UTC) for the review owner. It records what was checked, what was not, and the questions only the owner can answer. It is not legal advice and it states no legal conclusion. Line numbers are for commit `6c64777` (main after PR #76); they move when P1 renames and splits files, so every quoted row also gives the text to search for.
>
> **Owner:** to be named (plan section 11, decision 6). **Written outcome:** not yet written; due before P5 starts (section 2).
>
> **Companion:** [`NOTICE`](../NOTICE), the interim attribution text the plan asks for in P0.
>
> **How to read:** section 1 is the short version. Section 2 is the part to act on: what to do this week, the questions in the order they need answers, and the dates behind that order. Sections 3 to 7 are the evidence, kept as an appendix for checking a claim. Section 8 is the form for the written outcome.

## 1. What this review found

The owner's to-do list is section 2. This section summarizes what the evidence in sections 3 to 7 shows.

- **The lineage shows in the text.** `crates/deepagents-rs` describes itself as "Rust port of LangChain deepagents" (`crates/deepagents-rs/Cargo.toml:6`). Of the 20 named prompt constants (A19), **2 are verbatim** (identical to an upstream string, or an exact prefix of one), **9 are adapted** from upstream text (one of them from LangChain's `langchain` package, not from `deepagents`) and **9 have no match** in the upstream sources compared (section 5.1; the table says for each verdict whether it was measured or rests on reading). Several model-visible error strings and tool descriptions are also identical to upstream's apart from placeholder names (5.2), and the four golden request files that PR #76 added hold the prompts and tool descriptions as the model receives them (5.6).
- **Upstream is MIT-licensed, and its copyright line depends on the version.** `deepagents` carries "Copyright (c) 2025 Harrison Chase" in its 17 tagged releases 0.2.4 to 0.3.11, which include 0.3.8, the release named in `crates/craftsman-rs/src/backend_fs.rs:2`, and "Copyright (c) LangChain, Inc." in its 64 tagged releases 0.3.12 to 0.7.26 (the releases of 2026-10-16) and on `main`. The line was changed on 2026-02-04. The copied strings that were checked (constants 1, 3 and 9 and the four repeated error messages of 5.2) are identical on both sides of that change (section 4), so the text alone does not say which line applies (question 3). `NOTICE` reproduces both files exactly as published; `langchain` carries the second text.
- **No license file, copyright header or formal credit existed before `NOTICE`; the only license declaration is `license = "MIT"` on `deepagents-rs`.** No LICENSE, NOTICE or COPYING file was tracked and no source file has a copyright or SPDX header; the declaration is at `crates/deepagents-rs/Cargo.toml:7`. Comments in the code name upstream (5.3), but none is a notice. `crates/craftsman-rs/Cargo.toml` and the workspace manifest declare none. `NOTICE` does not say what license applies to Craftsman.
- **Binaries are published without license text beside them.** `release.yml` uploads bare executables, a manifest and checksums (question 5).
- **Not covered by `NOTICE` or by the inventory:** other third-party material noticed along the way (an Apache-2.0 schema bundle, voice assets, 405 locked Rust packages) and text that comments say was copied from Craftsman's own Python CLI, whose source is not in this repository (sections 5.8 and 7).
- **What this review could not do:** decide any legal question, see the Python CLI, compare against Claude Code, Codex or opencode, or establish how each string entered the repository (section 6).

## 2. What the owner has to do, and in what order

### 2.1 This week, and what this review already settles

**This week.** The plan's P0 exit needs the first two items; the third protects the next release.

1. Name the owner of decision 6, the legal reviewer and a date for the written outcome (question 8). The plan's P0 exit asks for the owner and the date (2.3).
2. Answer question 3 and merge `NOTICE`. The plan wants the interim `NOTICE` merged before the next release tag. If the answer is to keep both copyright lines, that question needs no change to `NOTICE`.
3. Before anyone pushes the next `v*` tag, answer questions 5 and 7. A `v*` tag runs `release.yml`, which publishes the binaries with no license text beside them, and one provider profile sends the `DeepAgents` billing-origin value to NVIDIA.

Everything else can wait for the dates in 2.2.

**What this review already settles** (facts, not legal conclusions):

- `NOTICE` exists. It reproduces both published upstream `LICENSE` files byte for byte, names LangChain, and says nothing about Craftsman's own license.
- The inventory covers the sources compared: for the 20 prompt constants, 2 verbatim, 9 adapted and 9 with no match found (5.1), with the other model-facing strings (5.2) and every comment that claims copying (5.3).
- The facts that questions 1, 2, 5 and 7 need are collected in the question texts in 2.2: where the license declarations are, what the release workflow publishes, which header value is sent to a third party.
- The dates the plan sets for all of this (2.3).

**What it does not settle:** any of the eight questions, what license applies to Craftsman, whether any use of the inherited text is permitted, and what the Craftsman Python CLI contains.

### 2.2 The questions, in the order they need answers

| Order | Question | Decide before | Needs first |
|---|---|---|---|
| 1 | **8.** Who owns decision 6, by what date, and who gives the legal review? | P0 exit (now) | nothing |
| 2 | **3.** Which upstream copyright line or lines should `NOTICE` carry? | next `v*` tag (`NOTICE` is merged before it) | nothing |
| 3 | **5.** Does redistribution of binaries need more than `NOTICE`? | next `v*` tag (a tag publishes binaries) | nothing |
| 4 | **7.** Should the `DeepAgents` billing-origin value (A20, decision 2) go before the next release? | next `v*` tag | nothing |
| 5 | **1.** What license applies to this repository? | P1 (the crate is renamed there) | nothing |
| 6 | **2.** Are the `Cargo.toml` `license` fields right? | P1 | question 1 |
| 7 | **4.** Was any other third-party text copied? | P5 (the written outcome gates P5) | nothing |
| 8 | **6.** Does `NOTICE` stay after P1 and P5? | P5 | what P1 and P5 leave inherited |

**Dependencies.** Question 8 comes first, because every other answer needs an owner and, for the legal parts, a reviewer. Question 1 comes before question 2. Questions 5 and 7 must be answered before any `v*` tag. Questions 4 and 6 belong to the written outcome, which the plan requires before P5 starts; question 6 can be settled only once it is known what P1 and P5 leave inherited. *Decide before* is this review's proposal: P0 exit, the next release tag and P5 are the plan's own dates (2.3); P1 for questions 1 and 2 is proposed because P1 renames the crate and rewrites its manifest description.

**The questions in full.**

1. **What license applies to this repository?** No LICENSE, NOTICE or COPYING file was tracked before `NOTICE`; `crates/craftsman-rs/Cargo.toml` and the workspace manifest declare none. `NOTICE` deliberately says nothing about it. The plan's Appendix B renames the crate, so the answer should also say what happens to `deepagents-rs`'s declaration at the rename.
2. **Are the `Cargo.toml` `license` fields right?** The only declaration is `crates/deepagents-rs/Cargo.toml:7`, added in `a0f7afa` (2026-07-17, the crate skeleton) next to the description "Rust port of LangChain deepagents". Who decided on MIT and on what basis, who holds the copyright in the Rust code, and does `craftsman-rs` need a `license`, a `license-file` or `publish = false`? No workflow runs `cargo publish`.
3. **Which upstream copyright line or lines should `NOTICE` carry?** `NOTICE` carries both published lines: "2025 Harrison Chase" (the line at release 0.3.8, the release the comments name, and at every tagged release from 0.2.4 to 0.3.11) and "LangChain, Inc." (the line at the head and from 0.3.12). It also names `langchain` for the `TODO_SYSTEM_PROMPT` text. The copied strings that were checked exist under both lines (section 4), so the text cannot decide it. Keep both, keep one, or add anything else?
4. **Was any other third-party text copied?** Candidates: the Craftsman Python CLI's text (5.8; does it itself contain upstream text? for constant 16 an upstream passage exists in upstream's own CLI package, so the Python CLI's text may itself descend from it), Claude Code, OpenAI Codex and opencode (named in comments and in commit `9d0e167`), Anthropic's prompting samples (behind upstream's Claude profiles), the A2UI spec text, and text whose production is not recorded (copied from the Python package, typed, or generated with an assistant; section 6). The comments in 5.3 are the starting list for asking the authors.
5. **Does redistribution of binaries need more than `NOTICE`?** Facts: `release.yml` copies the bare executable (`.github/workflows/release.yml:91-93`), assembles a manifest and `SHA256SUMS`, and publishes `out/*` (`.github/workflows/release.yml:218-222`), with no license file in the assets; the service-agent image copies only the verified binary (`docker/service-agent.Dockerfile:3-5`, `docker/service-agent.Dockerfile:17`); `install.sh` builds from source with `cargo install` (`install.sh:92`); `Cargo.lock` lists 405 packages (the two workspace crates included), and no file in the repository collects their notices. Should `NOTICE` and a dependency-license report ship as release assets, inside the image, or be printed by the binary, and who generates them?
6. **Does `NOTICE` stay after P1 and P5?** P1 removes lineage text and P5 rewrites model-facing text with a 5% overlap exit criterion, but names and shapes in 5.4 are kept or renamed case by case (Appendix B leaves two rows open, `docs/craftsman-harness-plan.md:1198`). What remains inherited after P5, and does the notice stay?
7. **Should the `DeepAgents` billing-origin value (A20, decision 2) go before the next release?** The plan's default is to drop it in P5. It is another project's identifier sent to a third party; the owner and the provider contact decide.
8. **Who owns decision 6, by what date, and who gives the legal review?** The plan lists the owner as "to be named" (`docs/craftsman-harness-plan.md:1129`) and requires the owner and a date at P0 exit (`docs/craftsman-harness-plan.md:634`). The written outcome should answer questions 1 to 7.

### 2.3 Where the dates come from

| Fact | Where | Text |
|---|---|---|
| P0 deliverable: an OSS-compliance review of the inherited text, with an interim `NOTICE` merged **before the next release tag**. The plan notes that the repository has no `LICENSE` or `NOTICE`, that `deepagents-rs` declares MIT and that the release workflow publishes binaries. | `docs/craftsman-harness-plan.md:624` | `carrying the upstream copyright merged before the next release tag` |
| The latest release tag is v0.3.0 and the crate version 0.4.0 is unreleased. | `docs/craftsman-harness-plan.md:620` | `the latest release tag is v0.3.0` |
| `release.yml` runs on tags that match `v*`, so the `baseline-*` tags of P0 do not trigger it; the plan's "next release tag" is the next `v*` tag. | `.github/workflows/release.yml:4` | `tags: ["v*"]` |
| P0 exit: owners and dates named for decisions 1, 5, 6, 9, 10, 11, 15 and 16; the OSS review has an owner and a date and the interim `NOTICE` is merged; **the written outcome is due before P5 starts**. | `docs/craftsman-harness-plan.md:634` | `its written outcome is due before P5 starts` |
| The "Still to do" line of section 11 repeats it. | `docs/craftsman-harness-plan.md:1142` | `decision 6 needs the written outcome of the OSS review before P5` |
| P5 is gated on the written outcome. | `docs/craftsman-harness-plan.md:880` | `P5 does not start before decision 6 (the OSS review) has a written outcome` |
| Decision 6 row: needed before P0, with the written outcome before P5; the adopted default. | `docs/craftsman-harness-plan.md:1129` | `P5 does not start without a written outcome` |
| The OSS review is a lead-time item outside engineering. | `docs/craftsman-harness-plan.md:608` | `its written outcome gates P5` |
| P1 adds a `PROVENANCE(P5)` marker above each of the 20 constants. The verdict column in 5.1 shows which constants carry upstream text. | `docs/craftsman-harness-plan.md:649` | `marker above each of the 20 prompt constants (A19)` |
| P1 adds `scripts/check-lineage.sh` with an allow list. | `docs/craftsman-harness-plan.md:650` | `scripts/check-lineage.sh` |
| P5 exit: for each new string at most 5% of its 6-grams may be present in the inherited text (taken from `baseline-0.4.0`), with a reviewer sign-off. The *6-word windows* column in 5.1 uses the same unit but measures the current text against upstream, not new text against the baseline. | `docs/craftsman-harness-plan.md:890` | `is at most 5%` |
| Risk register: "Licensing of inherited text" is mitigated by this review, the interim `NOTICE` and an overlap report. | `docs/craftsman-harness-plan.md:1088` | `P5 does not start without the review` |
| The plan's definition of inherited text. | `docs/craftsman-harness-plan.md:1224` | `that came from the upstream project` |

### 2.4 Terms used in this document

- *The plan*: `docs/craftsman-harness-plan.md`. *P0*, *P1* and *P5* are its phases "Prepare", "Own the names" and "Own the wording"; this review is a P0 deliverable, P1 renames the crate and rewrites lineage text, and P5 rewrites the model-facing strings. *Decision 6* is the plan's decision on the OSS review.
- *A19* and *A29*: rows of the plan's evidence appendix. A19 counts the rename blast radius and names the 20 prompt constants; A29 counts lineage text in comments.
- A `v*` tag is a git tag whose name starts with `v`; pushing one runs `release.yml`.
- *6-word window* (the plan says 6-gram): any six consecutive words of a text; the plan's P5 exit measures overlap as the share of these found in the inherited text.
- *Verbatim*, *adapted*, *no match found*, *measured* and *read*: defined in section 3.
- *SPDX*: standard short license identifiers such as `MIT`. *Blob*: git's object id for the contents of a file. *SHA-256*: a checksum used to compare two files byte for byte.

## 3. Scope and method

**Scope.** Text in this repository that came from, or follows, the open-source Python project `deepagents` (LangChain) and the `langchain` package it builds on. Other third-party material is listed in section 7 but not inventoried. The inventory covers the 80 compiled Rust files (the three files without a `mod` declaration, `repl.rs`, `repl_input.rs` and `status_bar.rs`, were scanned for string literals separately: 21 literals of six or more words, one shared 6-word run, a UI message `(press Ctrl-C again to quit)` at `crates/craftsman-rs/src/repl.rs:177`) and the other 38 tracked text files (every tracked file except the Rust sources, `Cargo.lock`, `NOTICE`, the plan, this review, the voice benchmark data and `.git-blame-ignore-revs`): the README, `AGENTS.md`, `CLAUDE.md`, the documents under `docs/`, `examples/`, `docker/`, `install.sh`, the manifests, the workflows and the four golden request files under `crates/craftsman-rs/tests/golden/`.

**Tree state.** The Rust sources, manifests, README and workflows are those of `6c64777` (main after PR #76). This branch adds `NOTICE`, this review and one line of `AGENTS.md`. Since `0b77980`, the commit the plan pins, three commits touch `crates/`: `7de34d2` (the rustfmt baseline, PR #72), `088dbca` (the local-only database, PR #74, which removed the Turso replica sync) and `43c2d93` (golden model-request tests, PR #76, which added two test files and four JSON fixtures). The plan's A19 and A29 counts are reproduced below; two of them changed with `088dbca`.

**Upstream sources compared.** Everything was downloaded on 2026-10-16 and 2026-10-17 (UTC) over HTTPS, as source archives (codeload.github.com) or single files (raw.githubusercontent.com) at pinned commits, and read as text; nothing downloaded was executed.

| Label | Repository and commit | What was compared |
|---|---|---|
| `0.3.8` | `langchain-ai/deepagents` at `7db4736d717ab5412daa3c594b57b1f4e3bb3eca` (tag `deepagents==0.3.8`, commit dated 2026-01-22) | the release named in `crates/craftsman-rs/src/backend_fs.rs:2`; whole archive, 193 text files |
| `main` | `langchain-ai/deepagents` at `c5313d7005534c8f8514004890c6f6d5174c6e2f` (commit dated 2026-10-16 08:32 UTC) | the main branch; whole archive, 1,809 text files. `main` has since advanced by two commits (section 4) |
| `cli-0.0.10` | `langchain-ai/deepagents` at `8a14a0de06c85bfc891c84651b27419522acd1cf` (tag `deepagents-cli==0.0.10`) | an older snapshot of the repository, to widen the search for the 20 constants (5.1); whole archive, 90 text files |
| `lc-1.2.4` | `langchain-ai/langchain` at `1d60235b1b0dfc1a64de107e30ff050ad71fff14` (tag `langchain==1.2.4`) | the `agents/middleware/` files `todo.py`, `summarization.py`, `types.py` and `human_in_the_loop.py`; 1.2.4 is the minimum `langchain` that deepagents 0.3.8 requires (`0.3.8 libs/deepagents/pyproject.toml:10`) |
| `lc-master` | `langchain-ai/langchain` at `c088db971097f0f12c7b8c30d473e8dbde09c823` (commit dated 2026-10-15 18:20 UTC) | the same four files |
| `a2ui` | `a2ui-project/a2ui` at `0d0ed91817421b979f7e56f7311761dd0b419763` (commit dated 2026-10-16 16:11 UTC) | the A2UI project, to see whether constant 18 follows its text (5.1); whole archive, 3,828 text files |
| `0.3.11`, `0.3.12` | `langchain-ai/deepagents` at `7d17be00b59e586c55517eaca281342e1a6559ff` and `6c7e354b2b98a8b8244dff32157de9615926fe9e` (tags `deepagents==0.3.11` and `deepagents==0.3.12`) | seven files only (`middleware/subagents.py`, `middleware/memory.py`, `graph.py`, `backends/utils.py`, `backends/state.py`, `backends/filesystem.py`, `backends/store.py`), the releases just before and just after the copyright change |
| `cli-0.0.40`, `cli-0.0.30` | `langchain-ai/deepagents` at `88c2b5cb874dc1d093acf54d2a967ba6e085c99b` and `eb0b26c9578c8ed9f20aea3b2d8b852af1a4a6ac` (tags `deepagents-cli==0.0.40`, dated 2026-04-20, and `deepagents-cli==0.0.30`, dated 2026-03-07) | one file each (`libs/cli/deepagents_cli/agent.py`), to see whether the passage found for constant 16 (5.1) is older than this repository |
| `lc-1.0.0`, `lc-1.1.0`, `lc-1.2.0`, `lc-1.2.3` | `langchain-ai/langchain` at `4a65e827f7d7fd8139a4232f408a005f704dc71b`, `9c21f83e82234b4a57f770fb5916feb83c906c7a`, `9f61ed8b81201d098919c6415839037e052ed417` and `76da99e022652684c7ea28bbf74220b0955a5287` (tags `langchain==1.0.0`, `langchain==1.1.0`, `langchain==1.2.0` and `langchain==1.2.3`) | one file each (`todo.py`), to see which releases carry the todo prompt that constant 4 follows (section 4) |

Upstream locations are written `<label> <path>:<line>`. For `0.3.8`, `main` and `cli-0.0.10` the path is relative to `libs/deepagents/deepagents/` (for `cli-0.0.10` to `libs/deepagents-cli/deepagents_cli/`, for `cli-0.0.40` to `libs/cli/deepagents_cli/`); for the `lc-` labels it is relative to `libs/langchain_v1/langchain/agents/middleware/`; paths that start with `libs/` or `README.md` are relative to the repository root.

**Method.**

1. *References.* Every `path:line` in this document, every quotation in the tables whose header has `Where` and `Text` columns, and every double-quoted passage in the prose was checked by a script (Python standard library): the file exists, the line (range) exists, the quoted text is on the cited line, and a double-quoted passage occurs in the repository (this review excepted), an upstream archive or a commit message. A passage in curly quotes is a run of words only: its words occur in that order in one of the same texts, punctuation and case ignored. Results are in section 6.
2. *Counts.* The counts of the plan's rows A19 and A29 were reproduced with `git grep` (below), and every line counted in A29 was checked against this document to see that it is cited (section 6). Every other count in the prose was recomputed from the tables, the tree or git by a second script.
3. *Word runs (measured).* Text is lower-cased and reduced to words (`[a-z0-9_]+`), so punctuation and line breaks do not matter. For each of the 20 constants, and for each of the 733 string literals of six or more words in the 80 compiled files (the literals against `0.3.8`, `main`, `lc-1.2.4` and `lc-master` only), the script reports the longest run of consecutive identical words found in any one file of a corpus and the share of the text's 6-word windows that occur in the corpus. The 6-word window is the unit of the plan's P5 overlap report. The other tracked text files were scanned the same way (5.5); the four golden request files were compared as the JSON strings they contain.
4. *Character level (measured).* Every upstream `.py` file was parsed with `ast` (never imported) and each string literal of 30 or more characters that shares a 6-word window with a constant was compared with it after collapsing whitespace: equal, prefix of or extended by the constant, and the longest common substring (`difflib`, no junk heuristic). "Verbatim" is decided here. Constant 16 shares no 6-word window with any upstream string, so this comparison could not reach it; the passage found for it by reading (5.1) was compared directly, as the share of the constant's characters that lie in matching blocks of 12 or more characters.
5. *Reading.* Where the measurement finds no long run but a passage appears to follow an upstream passage, or where a counterpart might exist, the two texts were read side by side. A verdict that rests on reading is marked **read** or **measured, read** in the Basis column of 5.1 and is a judgment of this review, not a measurement alone.
6. *Upstream facts.* License files were downloaded as bytes and hashed; tags, dates and commit history come from the GitHub REST API; the `LICENSE` of every one of the 81 stable `deepagents` release tags was downloaded by tag name and hashed (section 4).

**Verdict words.** *verbatim*: the constant equals an upstream string, or is an exact prefix of one. *adapted*: a long run of words is shared with edits, or the text follows an identified upstream passage point by point in different words. *no match found*: no run of six or more words in common with the sources compared (apart from identifiers such as URLs and message names) and no upstream passage the text could be lined up with; this is as close as the review can come to "original", and it is not a statement about authorship or about works that were not compared. **Basis**: *measured* means the verdict follows from the comparison alone; *read* means the comparison finds at most a 7-word run and the verdict rests on reading the two texts; *measured, read* means a measurement and a reading both bear on it: the comparison finds a long run, or no run at all, or (constant 16) an overlap in characters without a 6-word window, and the likeliest upstream counterpart was also read (for *no match found*, to see whether a counterpart exists in other words).

**Reproducing the plan's counts (A19, A29).** In a shell that understands arrays (bash or zsh), at `6c64777`:

```sh
EX=(':!crates/craftsman-rs/src/repl.rs' ':!crates/craftsman-rs/src/repl_input.rs' ':!crates/craftsman-rs/src/status_bar.rs')
git grep -n -i -E 'deepagents|deep agents|langchain|langgraph' -- 'crates/**/*.rs' "${EX[@]}" | wc -l                   # 148 = 29 + 116 + 3
git grep -n -i -E 'deepagents|deep agents|langchain|langgraph' -- 'crates/**/*.rs' "${EX[@]}" \
  | grep -v -E ':[0-9]+:.*(deepagents_rs|deepagents-rs|DeepAgent)' | wc -l                                              # 29 comment lines
git grep -n Python -- 'crates/**/*.rs' "${EX[@]}" | wc -l                                                               # 65 lines
git grep -l Python -- 'crates/**/*.rs' "${EX[@]}" | wc -l                                                               # in 21 files
git grep -n -i -E 'parity|mirror|byte-matched|upstream|port of' -- 'crates/**/*.rs' "${EX[@]}" | wc -l                  # 73 lines
git grep -l -E 'deepagents_rs|deepagents-rs|DeepAgent' -- 'crates/**/*.rs' "${EX[@]}" | wc -l                           # 38 files
git grep -n 'deepagents_rs::' -- 'crates/craftsman-rs/src/*.rs' "${EX[@]}" | wc -l                                          # 111 lines (in 26 files)
```

At `6c64777` five of these seven numbers equal the plan's A19 and A29 (`docs/craftsman-harness-plan.md:1168`, `docs/craftsman-harness-plan.md:1178`); all seven equal them at `03c38ae` and at `0b77980`. The two that differ are one lower than the plan's: 65 lines say "Python" (the plan: 66) and 73 lines match the word list (the plan: 74). `088dbca` deleted one comment line that said "Python" (in `config.rs`) and one that said "mirroring" (in `store_fs.rs`), and reworded two others that still count. The plan itself was not changed by this review.

## 4. What was verified about upstream

| Fact | Result | Source | How |
|---|---|---|---|
| Project | `langchain-ai/deepagents`, described as *The batteries-included agent harness.*; default branch `main`; license detected as SPDX `MIT` | https://api.github.com/repos/langchain-ai/deepagents | REST API |
| `LICENSE` at the head | MIT License, "Copyright (c) LangChain, Inc."; 21 lines, 1,067 bytes; git blob `3957738673765912ca83a0048f833e095d61e087`; SHA-256 `4ec67e4ca6e6721dba849b2ca82261597c86a61ee214bbf21416006b7b2d0478` | https://raw.githubusercontent.com/langchain-ai/deepagents/c5313d7005534c8f8514004890c6f6d5174c6e2f/LICENSE | bytes downloaded; blob hash computed with `git hash-object` and equal to the `sha` the REST API reports for `LICENSE` on `main`; equal to the copy in the archive at that commit and to the copy in `NOTICE` |
| `LICENSE` at release 0.3.8 | MIT License, "Copyright (c) 2025 Harrison Chase"; 21 lines, 1,071 bytes; blob `8f1b6f1a01734749311ab619ac901284fc91892e`; SHA-256 `73ffc16b15022baf657a8db210aca77fc9569ddbbc8b063081ed426f2a807be1`; identical to the file in the initial commit `5c3ad025ba22c7a53b512e28e44d10c990bc49d0` (2025-07-27) | https://raw.githubusercontent.com/langchain-ai/deepagents/7db4736d717ab5412daa3c594b57b1f4e3bb3eca/LICENSE | bytes downloaded; `git hash-object`; `cmp` against the initial commit's file; equal to the copy in `NOTICE` |
| When the line changed | Two commits touch `LICENSE`: `5c3ad025ba22c7a53b512e28e44d10c990bc49d0` (2025-07-27, *Initial commit*) and `fd645308188da38934b3f00007c1bc5d3fc756d3` (2026-02-04, *chore(sdk): update license (#1088)*); the text differs only in the copyright line | https://api.github.com/repos/langchain-ai/deepagents/commits?path=LICENSE | REST API; the two files compared line by line |
| Which releases carry which line | Of 102 `deepagents==` tags, 81 are stable (`X.Y.Z`; the other 21 are alpha and beta pre-releases and were not downloaded). 17 stable tags, 0.2.4 to 0.3.11, carry the 2025 line (all with SHA-256 `73ffc16b...`); 64, 0.3.12 to 0.7.26, carry "LangChain, Inc." (all with `4ec67e4c...`); no tag changes back. This is the state of 2026-10-16; releases published later were not scanned | https://api.github.com/repos/langchain-ai/deepagents/git/matching-refs/tags/deepagents== | `LICENSE` of each stable tag downloaded by tag name and hashed |
| Dates around the change | 0.3.11 is dated 2026-02-04 16:03 UTC, the license commit 2026-02-04 22:50 UTC, 0.3.12 2026-02-06; 0.3.8 is dated 2026-01-22 | `https://api.github.com/repos/langchain-ai/deepagents/git/commits/` plus the commit sha | REST API |
| Copied text on both sides of the change | Constants 1, 3 (as an exact prefix) and 9, and the four repeated error messages of 5.2, compare identically at `0.3.11` and at `0.3.12`, with the same longest runs as at `0.3.8`, so these copied strings exist under both copyright lines | the seven files listed in section 3, at both tags | section 3, methods 3 and 4 |
| Upstream NOTICE file | none: no file named `NOTICE*` anywhere in the archives at `7db4736` or `c5313d7`; `libs/deepagents/LICENSE` at `c5313d7` is identical to the root file (same SHA-256); the other `LICENSE` files in that archive were not examined | the archives | file listing; SHA-256 |
| Package metadata | `license = { text = "MIT" }` in `0.3.8 libs/deepagents/pyproject.toml:6` and `main libs/deepagents/pyproject.toml:7` | the archives | grep |
| Upstream README | mentions the license only as a badge that links https://opensource.org/licenses/MIT (`main README.md:16`); no copyright line; the Acknowledgements section says "Inspired by Claude Code" (`main README.md:108`) | https://raw.githubusercontent.com/langchain-ai/deepagents/c5313d7005534c8f8514004890c6f6d5174c6e2f/README.md | grep of the archive |
| LangChain | `langchain-ai/langchain` `LICENSE` is byte-identical to deepagents' head file (blob `3957738673765912ca83a0048f833e095d61e087`, SHA-256 `4ec67e4c...`) at tag `langchain==1.2.4`, at `c088db97...` and at its master branch on 2026-10-16; SPDX `MIT` | https://api.github.com/repos/langchain-ai/langchain/license and raw files | bytes downloaded and compared byte for byte (the SHA-256 values are equal) |
| Dependency on LangChain | deepagents 0.3.8 requires `langchain>=1.2.4` (`0.3.8 libs/deepagents/pyproject.toml:10`) and imports `TodoListMiddleware` from it (`0.3.8 graph.py:7`) | the archive | grep |
| LangChain's todo prompt across releases | the string `WRITE_TODOS_SYSTEM_PROMPT` in `todo.py` is the same text (1,074 characters) in `langchain==1.0.0`, `1.1.0`, `1.2.0`, `1.2.3` and `1.2.4`, and differs on `lc-master` (1,370 characters); the string `WRITE_TODOS_TOOL_DESCRIPTION` is the same text (3,654 characters) in those five releases and differs on `lc-master` (3,873 characters) | the single files at those tags | parsed with `ast` (never run); the two strings compared and hashed |
| The headless-mode passage compared with constant 16 | the `interactive_preamble` and `ambiguity_guidance` strings are the same 15 lines at `main` (`main libs/code/deepagents_code/agent.py:1674-1688`) and at tag `deepagents-cli==0.0.40` (`cli-0.0.40 agent.py:578-592`, dated 2026-04-20); the file at tag `deepagents-cli==0.0.30` (dated 2026-03-07) has no `npm init` text | the single files at those tags | lines compared; text searched |
| When the cited profile files were added | `_anthropic_opus_4_7.py`, `_anthropic_sonnet_4_6.py`, `_openai_codex.py` and `harness_profiles.py` (all under `libs/deepagents/deepagents/profiles/harness/`) in `7365ad1600`, *feat(sdk): profiles API (#2892)*, 2026-04-28; `profiles/provider/_nvidia.py` in `4cb47491b0`, *feat(sdk): add NVIDIA NIM app-origin attribution (#4455)*, 2026-07-02; `profiles/harness/_nvidia_nemotron_3_ultra.py` in `d5a60ece73`, *feat(sdk): add NVIDIA Nemotron 3 Ultra harness profile (#4192)*, 2026-07-07. None of these files exists at 0.3.8 | `https://api.github.com/repos/langchain-ai/deepagents/commits?path=` plus each path | oldest commit that touches each path (the Haiku profile was not checked) |
| `main` has moved | on 2026-10-16 `main` is two commits ahead of `c5313d7`, at `11aaba8bb259494c36d2bc6af02ad93e6226f230`; they change 18 files: 12 `uv.lock` files, three `pyproject.toml` files, `middleware/async_subagents.py`, `middleware/_parent_notifier.py` and one test. `LICENSE` and the prompt and profile files compared in this document are not among them; `libs/deepagents/pyproject.toml` is, and its `license` line (line 7) is unchanged | https://api.github.com/repos/langchain-ai/deepagents/compare/c5313d7005534c8f8514004890c6f6d5174c6e2f...11aaba8bb259494c36d2bc6af02ad93e6226f230 | REST API |
| The A2UI project (section 7) | GitHub reports SPDX `Apache-2.0` for `a2ui-project/a2ui` (LICENSE blob `b24d03ff56002c5252a12265de3c958ba96c9162`) | https://api.github.com/repos/a2ui-project/a2ui/license | REST API; its copyright line and any NOTICE file were not checked |

## 5. Inventory

### 5.1 The 20 named prompt constants (A19)

All 20 exist, in 14 files (7 constants in `deepagents-rs` and 13 in `craftsman-rs`, in 6 and 8 files), as the plan says. Longest identical run and window share are the best of the six corpora `0.3.8`, `main`, `cli-0.0.10`, `lc-1.2.4`, `lc-master` and `a2ui` (the label is in brackets). Verdict and Basis are defined in section 3.

| # | Constant | Where | Words | Verdict | Basis | Longest identical run (words) | 6-word windows found upstream |
|---|---|---|---|---|---|---|---|
| 1 | `EMPTY_FILE_REMINDER` | `crates/deepagents-rs/src/backend.rs:7` | 8 | **verbatim** | measured | 8 of 8 (0.3.8) | 100.0% (0.3.8) |
| 2 | `TODO_REMINDER` | `crates/deepagents-rs/src/builtin/reminders.rs:27-31` | 52 | **no match found** | measured | none of 6 or more | 0% |
| 3 | `TASK_TOOL_DESCRIPTION` | `crates/deepagents-rs/src/builtin/subagents.rs:60` | 231 | **verbatim** | measured | 231 of 231 (0.3.8) | 100.0% (0.3.8) |
| 4 | `TODO_SYSTEM_PROMPT` | `crates/deepagents-rs/src/builtin/todos.rs:10` | 128 | **adapted** | measured | 121 of 128 (lc-1.2.4) | 94.3% (lc-1.2.4) |
| 5 | `SUMMARY_SYSTEM` | `crates/deepagents-rs/src/builtin/compaction.rs:49-56` | 96 | **no match found** | measured, read | none of 6 or more | 0% |
| 6 | `SUMMARY_REQUEST` | `crates/deepagents-rs/src/builtin/compaction.rs:59-60` | 11 | **no match found** | measured | none of 6 or more | 0% |
| 7 | `MEMORY_GUIDELINES` | `crates/deepagents-rs/src/builtin/memory.rs:21-32` | 110 | **adapted** | read | 6 of 110 (0.3.8) | 1.0% (0.3.8) |
| 8 | `BASE_SYSTEM_PROMPT` | `crates/craftsman-rs/src/harness_profiles/general.rs:13-24` | 132 | **no match found** | measured, read | none of 6 or more | 0% |
| 9 | `GENERAL_PURPOSE_DESCRIPTION` | `crates/craftsman-rs/src/harness_profiles/general.rs:5` | 67 | **adapted** | measured | 56 of 67 (0.3.8) | 83.9% (0.3.8) |
| 10 | `GENERAL_PURPOSE_SYSTEM_PROMPT` | `crates/craftsman-rs/src/harness_profiles/general.rs:7-8` | 14 | **no match found** | measured, read | none of 6 or more | 0% |
| 11 | `COMMON_SUFFIX` (claude) | `crates/craftsman-rs/src/harness_profiles/claude.rs:5` | 64 | **adapted** | read | none of 6 or more | 0% |
| 12 | `OPUS_47_OVERLAY` | `crates/craftsman-rs/src/harness_profiles/claude.rs:7` | 60 | **adapted** | measured, read | 17 of 60 (main) | 23.6% (main) |
| 13 | `SYSTEM_PROMPT_SUFFIX` (openai) | `crates/craftsman-rs/src/harness_profiles/openai.rs:5` | 82 | **adapted** | read | none of 6 or more | 0% |
| 14 | `SYSTEM_PROMPT_SUFFIX` (nemotron) | `crates/craftsman-rs/src/harness_profiles/nemotron.rs:6` | 149 | **adapted** | read | 6 of 149 (main) | 1.4% (main) |
| 15 | `READ_FILE_DESCRIPTION` (nemotron) | `crates/craftsman-rs/src/harness_profiles/nemotron.rs:8` | 51 | **adapted** | read | 7 of 51 (0.3.8) | 4.3% (0.3.8) |
| 16 | `NONINTERACTIVE_INSTRUCTIONS` | `crates/craftsman-rs/src/prompt.rs:13-16` | 32 | **adapted** | measured, read | none of 6 or more | 0% |
| 17 | `MEMORY_PROTOCOL` | `crates/craftsman-rs/src/prompt.rs:18-29` | 99 | **no match found** | measured, read | none of 6 or more | 0% |
| 18 | `SYSTEM_PROMPT_SUFFIX` (A2UI) | `crates/craftsman-rs/src/a2ui.rs:192-261` | 508 | **no match found** | measured | 15 of 508 (a2ui) | 4.2% (a2ui) |
| 19 | `DEFAULT_AGENTS_MD` | `crates/craftsman-rs/src/agents.rs:12-31` | 108 | **no match found** | measured, read | none of 6 or more | 0% |
| 20 | `SERVICE_BOUNDARY` | `crates/craftsman-rs/src/service_agent.rs:49-53` | 82 | **no match found** | measured | none of 6 or more | 0% |

Notes on each row (upstream locations use the labels from section 3). *No shared run* means no run of six or more identical words. Each constant's first-adding commit is the oldest commit in which `git log -S` finds its current text; section 6 notes which of those commits carry an AI co-author trailer.

- **#1 `EMPTY_FILE_REMINDER`.** Equal to upstream `EMPTY_CONTENT_WARNING` (51 characters): `0.3.8 backends/utils.py:17`, `0.3.8 middleware/filesystem.py:39` and `main backends/utils.py:24`; the string is also the same at `cli-0.0.10`, `0.3.11` and `0.3.12`. The doc comment at `crates/deepagents-rs/src/backend.rs:6` says so. An 8-word status message. First added in `98500f4` (2026-07-16, application) and `36f99bb` (2026-07-17, framework).
- **#2 `TODO_REMINDER`.** No 6-word run in common with any source compared (the longest shared run is 4 words). The module header calls the block Claude-Code-style (`crates/deepagents-rs/src/builtin/reminders.rs:2`); Claude Code text was not available to compare. First added in `13416eb` (2026-07-17).
- **#3 `TASK_TOOL_DESCRIPTION`.** Craftsman's 1,349 characters are an exact prefix of the 6,571-character constant at `0.3.8 middleware/subagents.py:115`, up to and including usage note 5. Upstream goes on with notes 6 and 7 and example blocks that Craftsman leaves out; Craftsman appends its own note 6 at run time (`crates/deepagents-rs/src/builtin/subagents.rs:306-309`). The same prefix relation holds at `cli-0.0.10`, `0.3.11` and `0.3.12`; at `main` the constant has been rewritten (`main middleware/subagents.py:493`) and the longest shared run is 11 words. The `{available_agents}` placeholder is upstream's. The doc comment at `crates/deepagents-rs/src/builtin/subagents.rs:59` says the wording follows deepagents' constant. First added in `b3364e2` (2026-07-17).
- **#4 `TODO_SYSTEM_PROMPT`.** The first 121 of 128 words (713 of 757 characters) are identical to LangChain's `WRITE_TODOS_SYSTEM_PROMPT` (`lc-1.2.4 todo.py:105`). Craftsman rewords the last sentence and leaves out upstream's closing section on usage notes. The text is not in the `deepagents` repository: `deepagents` imports `TodoListMiddleware` from `langchain` (`0.3.8 graph.py:7`). On langchain master the prompt has since changed (longest shared run 82 words), so Craftsman's text comes from an earlier langchain version. The doc comment at `crates/deepagents-rs/src/builtin/todos.rs:9` says the wording follows upstream's `TodoListMiddleware`. First added in `b98394b` (2026-07-17).
- **#5 `SUMMARY_SYSTEM`.** No shared run (the longest is 3 words). Read against LangChain's `DEFAULT_SUMMARY_PROMPT` (`lc-1.2.4 summarization.py:33`), which `deepagents` reuses: the structure differs (a four-part continuation summary against XML-tagged context extraction). The module header calls the approach Claude-Code-style (`crates/deepagents-rs/src/builtin/compaction.rs:1`). First added in `3a32b68` (2026-07-17).
- **#6 `SUMMARY_REQUEST`.** No shared run (11 words in all). First added in `3a32b68` (2026-07-17).
- **#7 `MEMORY_GUIDELINES`.** A rewording and trim of the `<memory_guidelines>` block of upstream's `MEMORY_SYSTEM_PROMPT` (`0.3.8 middleware/memory.py:93`). The opening sentence has the same structure (upstream: "The above <agent_memory> was loaded in from files in your filesystem"), the `<agent_memory>` and `<memory_guidelines>` tags are the same, and most bullets have a counterpart in upstream's text (update memory at once, before replying; keep the principle behind a correction; do not record transient details; never store credentials), reworded; upstream's examples section is not carried over. The only run of six words that is shared is “in your filesystem as you learn” (words only, punctuation ignored); no copied run is longer than six words, and the text follows upstream's structure and tags (adapted). Self-described at `crates/deepagents-rs/src/builtin/memory.rs:3` and `crates/deepagents-rs/src/builtin/memory.rs:19-20`. First added in `07d5ca1` (2026-07-17).
- **#8 `BASE_SYSTEM_PROMPT`.** No shared run (the longest is 5 words). Read against upstream's base prompt, a single sentence (`0.3.8 graph.py:30`), which is unrelated. `crates/craftsman-rs/src/harness_profiles/general.rs:10-12` says this is the earlier Craftsman base prompt moved intact, and `crates/craftsman-rs/src/prompt.rs:1` calls the surrounding assembly a mirror of the Craftsman Python CLI, so its origin may be that CLI (not checked). First added in `409f8bb` (2026-07-17).
- **#9 `GENERAL_PURPOSE_DESCRIPTION`.** The first 318 of 381 characters (56 of 67 words) are identical to upstream's `DEFAULT_GENERAL_PURPOSE_DESCRIPTION` (`0.3.8 middleware/subagents.py:254`; the same text is at `main middleware/subagents.py:521`, `0.3.11` and `0.3.12`). The last sentence is Craftsman's ("This agent has access to the main agent's non-interactive host, web, and platform tools." against upstream's "This agent has access to all tools as the main agent."). The subagent name `general-purpose` is upstream's too (`0.3.8 middleware/subagents.py:298`). First added in `a88682e` (2026-07-17).
- **#10 `GENERAL_PURPOSE_SYSTEM_PROMPT`.** No shared run (14 words in all). Read against upstream's `DEFAULT_SUBAGENT_PROMPT` (`0.3.8 middleware/subagents.py:105`), which is unrelated. First added in `a88682e` (2026-07-17).
- **#11 `COMMON_SUFFIX` (claude).** No shared 6-word run, but it condenses the sections of upstream's Claude profiles (parallel tool calls, investigate before answering, reflect on tool results; `main profiles/harness/_anthropic_haiku_4_5.py:32`, `main profiles/harness/_anthropic_sonnet_4_6.py:32`, `main profiles/harness/_anthropic_opus_4_7.py:26`) into one paragraph in the same order, plus a sentence on subagents that corresponds to the `<subagent_usage>` section of upstream's Opus 4.7 profile. Upstream says its Claude text matches Anthropic's published samples verbatim and cites Anthropic's prompting guide (`main profiles/harness/_anthropic_opus_4_7.py:13`, `main profiles/harness/_anthropic_opus_4_7.py:17-19`), so a third party sits behind the upstream text and Craftsman's author may have worked from either (section 6). The verdict rests on reading the two texts, not on a measurement. First added in `c524c18` (2026-07-22).
- **#12 `OPUS_47_OVERLAY`.** A 17-word run is identical to upstream's Opus 4.7 `<tool_usage>` section ("files, tests, or system output, use tools to observe that state directly rather than reasoning from memory"; `main profiles/harness/_anthropic_opus_4_7.py:39-41`); the closing sentence and the `<subagent_usage>` part are reworded condensations of `main profiles/harness/_anthropic_opus_4_7.py:39-46`. That upstream file has no counterpart at 0.3.8, so the text was written against a later upstream version than the one named in `crates/craftsman-rs/src/backend_fs.rs:2`. First added in `c524c18` (2026-07-22).
- **#13 `SYSTEM_PROMPT_SUFFIX` (openai).** No shared 6-word run, but it follows the points of upstream's Codex suffix in order: act as an autonomous senior engineer (Craftsman: operator) once given a direction, gather context, implement and verify; batch independent tool calls; reconcile every todo item as done, blocked with a reason, or cancelled before finishing (`main profiles/harness/_openai_codex.py:40`). The three Codex model ids are the same (`crates/craftsman-rs/src/harness_profiles/openai.rs:7` against `main profiles/harness/_openai_codex.py:27-31`). The verdict rests on reading the two texts, not on a measurement. First added in `c524c18` (2026-07-22).
- **#14 `SYSTEM_PROMPT_SUFFIX` (nemotron).** The sections `<approach>`, `<grounding>`, `<loop_control>`, `<tool_selection>` and a final-answer section appear in the same order as in upstream's Nemotron 3 Ultra suffix (`main profiles/harness/_nvidia_nemotron_3_ultra.py:1761`), each reworded and shortened (for example upstream: "Verify state with tools instead of recalling it"; Craftsman: "Verify state with tools"); upstream has further sections (`<state_changes>` and others). The only shared 6-word run is the sequence of tag names and the words that follow them. The verdict rests on reading the two texts, not on a measurement. First added in `c524c18` (2026-07-22).
- **#15 `READ_FILE_DESCRIPTION` (nemotron).** A condensed paraphrase of upstream's `_READ_FILE_DESCRIPTION_OVERRIDE` (`main profiles/harness/_nvidia_nemotron_3_ultra.py:488`): it covers the same points (paginate with offset and limit; keep reading to the end when asked for the whole file; smaller pages for large files; read before editing), condensed and in a different order. One 7-word run is shared ("always read a file before editing it"). First added in `c524c18` (2026-07-22).
- **#16 `NONINTERACTIVE_INSTRUCTIONS`.** The longest identical run is 5 words (“make reasonable assumptions and proceed”, words only), so no 6-word window is shared, but the text lines up point by point with a passage of upstream's headless-mode instructions (`main libs/code/deepagents_code/agent.py:1674-1688`, the `interactive_preamble` and `ambiguity_guidance` strings): both tell the model not to ask clarifying questions, to make reasonable assumptions and proceed, and to use non-interactive variants of shell commands, and both give the same two examples in the same order, `npm init -y` and `apt-get install -y`. Upstream's passage is longer (it also says that no human is available, adds a rule on ambiguity and names further flags). Run on that passage, the character-level comparison of section 3 (method 4) puts 94 of the constant's 219 characters (43%) in matching blocks of 12 or more characters. The same strings are in the CLI package of `deepagents` at tag `deepagents-cli==0.0.40` (`cli-0.0.40 agent.py:578-592`, dated 2026-04-20, before this repository's first commit) and are not in the same file at tag `deepagents-cli==0.0.30` (2026-03-07; no `npm init` in it). The verdict rests on that overlap and on reading the two texts. `crates/craftsman-rs/src/prompt.rs:11-12` says the constant mirrors the Craftsman Python CLI's constant of the same name (`chat.py`); that CLI's source is not in this repository (not checked), so the text may have come here through the Python CLI, and the Python CLI's text may itself descend from upstream's. First added in `98500f4` (2026-07-16).
- **#17 `MEMORY_PROTOCOL`.** No shared run. Read against upstream's memory guidance in the CLI (`cli-0.0.10 agent_memory.py:45`) and in `MEMORY_SYSTEM_PROMPT` (#7): no counterpart for its research, response and learning structure was found. The module header says the prompt assembly mirrors the Craftsman Python CLI (`crates/craftsman-rs/src/prompt.rs:1`; not checked). The identifier was first added in `98500f4` (2026-07-16) with an older text; the current wording was first added in `9dcb7f9` (2026-07-17).
- **#18 `SYSTEM_PROMPT_SUFFIX` (A2UI).** No shared prose with the `deepagents` and `langchain` sources, and none with the A2UI project itself (`a2ui`, section 3): the only shared run, 15 words, is protocol identifiers (a catalog URL, `version`, `updateComponents`). `crates/craftsman-rs/src/a2ui.rs:190-191` says it is a verbatim copy of the Craftsman Python CLI's block (not checked). It teaches the A2UI v0.9 basic catalog, whose schema bundle is vendored under Apache-2.0 (section 7). First added in `6b96952` (2026-07-20).
- **#19 `DEFAULT_AGENTS_MD`.** No shared run. Read against upstream's default agent prompt for its CLI (`0.3.8 libs/deepagents-cli/deepagents_cli/default_agent_prompt.md:1`): a different text with a different structure. `crates/craftsman-rs/src/agents.rs:9-11` says it is verbatim from the Craftsman Python CLI's `memory.DEFAULT_AGENTS_MD` (not checked). First added in `b5f7255` (2026-07-20).
- **#20 `SERVICE_BOUNDARY`.** No shared run; Craftsman-specific (service-agent profile). First added in `95d3c16` (2026-08-20).

**Totals.** Verbatim 2 (constants 1 and 3), adapted 9 (4, 7, 9, 11, 12, 13, 14, 15, 16), no match found 9 (2, 5, 6, 8, 10, 17, 18, 19, 20). By basis: measured only 8 (1, 2, 3, 4, 6, 9, 18, 20), measured and read 7 (5, 8, 10, 12, 16, 17, 19), read only 5 (7, 11, 13, 14, 15). Constants 16, 18 and 19 say they come from the Craftsman Python CLI, and 17 sits in a module that says it mirrors that CLI; the CLI was not available to check.

### 5.2 Other model-facing strings that match upstream

The plan notes that every tool description is an inline string outside the 20 constants (A19). The scan of all 733 string literals of six or more words found 61 with a run of six or more identical words in some corpus; 8 of them are among the 20 constants, 20 are in the table below, and the other 33 are listed after it. The first four rows are the same text repeated across the backends and tools (11 occurrences); for those a second script blanked every `{placeholder}` on both sides and found each Rust string equal to an upstream string. The quoted text is what the cited lines have in common. Placeholders are Craftsman's.

| Where | Text | Longest identical run (words) | Upstream location | Note |
|---|---|---|---|---|
| `crates/deepagents-rs/src/backend.rs:172`<br>`crates/deepagents-rs/src/backend.rs:503`<br>`crates/craftsman-rs/src/store_fs.rs:146`<br>`crates/craftsman-rs/src/tools.rs:180` | `because it already exists.` | 16 | `0.3.8 backends/state.py:136`, `0.3.8 backends/filesystem.py:316`, `0.3.8 backends/store.py:320` | The whole message, including "Read and then make an edit, or write to a new path." (on the next line in `tools.rs`), is identical except the placeholder name (`{file_path}` upstream). Tool result seen by the model. |
| `crates/deepagents-rs/src/backend.rs:204`<br>`crates/deepagents-rs/src/backend.rs:536`<br>`crates/craftsman-rs/src/store_fs.rs:176` | `Error: String not found in file: '{old}'` | 6 | `0.3.8 backends/utils.py:200`, `main backends/utils.py:745` | Identical except the placeholder name (`{old_string}` upstream). Tool result. |
| `crates/deepagents-rs/src/backend.rs:214`<br>`crates/deepagents-rs/src/backend.rs:545`<br>`crates/craftsman-rs/src/store_fs.rs:186` | `times in file. Use replace_all=True to replace all instances, or provide a more specific string with surrounding context.` | 19-21 | `0.3.8 backends/utils.py:203`, `main backends/utils.py:749-750` | Identical except placeholder names. Tool result. |
| `crates/deepagents-rs/src/builtin/subagents.rs:373` | `We cannot invoke subagent {subagent_type} because it does not exist` | 15 | `0.3.8 middleware/subagents.py:411`, `main middleware/subagents.py:847` | Identical except the final placeholder. Tool result. |
| `crates/deepagents-rs/src/builtin/subagents.rs:327` | `Launch an ephemeral subagent to handle a complex, multi-step independent task` | 10 | `main middleware/subagents.py:493` | Tool description of `task`; the opening words follow upstream's current constant. |
| `crates/craftsman-rs/src/agent.rs:429` | `Read a file from the local filesystem. Results use cat -n format,` | 9 | `0.3.8 middleware/filesystem.py:174` | Tool description of `read_file`; the 9-word run "cat -n format, with line numbers starting at 1" continues on the next line. |
| `crates/craftsman-rs/src/agent.rs:435` | `Absolute path to the file to read.` | 7 | `0.3.8 backends/protocol.py:203` | Parameter description of `read_file`; the opening of upstream's longer description. |
| `crates/craftsman-rs/src/agent.rs:436` | `Line number to start reading from (0-indexed).` | 8 | `0.3.8 backends/protocol.py:204` | Parameter description of `read_file`; the opening of upstream's longer description. |
| `crates/craftsman-rs/src/agent.rs:437` | `Maximum number of lines to read.` | 6 | `0.3.8 backends/protocol.py:205` | Parameter description of `read_file`; the opening of upstream's longer description. |
| `crates/craftsman-rs/src/agent.rs:444` | `Create a new file with the given content. Fails if the file already` | 6 | `0.3.8 backends/filesystem.py:316` | Tool description of `write_file`; the 6-word tail "or write to a new path" is shared. |
| `crates/deepagents-rs/src/builtin/todos.rs:33` | `Create and manage a structured task list for the current session.` | 8 | `lc-1.2.4 todo.py:45` | Tool description of `write_todos`; an 8-word run with LangChain's description. |
| `crates/deepagents-rs/src/backend.rs:126` | `this backend does not support command execution` | 6 | `0.3.8 middleware/filesystem.py:703-704` | Error text of the default `execute`. |
| `crates/craftsman-rs/src/agent.rs:484` | `Find files matching a glob pattern` | 6 | `0.3.8 backends/filesystem.py:538` | Tool description of `glob`. |

The remaining 33 literals with a run of six or more words are SQL, identifiers, URLs, test data or short generic phrases. File, line and the run length in brackets:

- Column lists in SQL DDL for the checkpoint tables: `crates/craftsman-rs/src/checkpoints.rs:190` (6), `crates/craftsman-rs/src/checkpoints.rs:222` (10), `crates/craftsman-rs/src/checkpoints.rs:233` (7), `crates/craftsman-rs/src/checkpoints.rs:669` (6), `crates/craftsman-rs/src/checkpoints.rs:692` (7), `crates/craftsman-rs/src/checkpoints.rs:1303` (7), `crates/craftsman-rs/src/threads_db.rs:231` (7), `crates/craftsman-rs/src/threads_db.rs:245` (16).
- Public model identifiers (Nemotron ids and their tests): `crates/craftsman-rs/src/harness_profiles/nemotron.rs:11` (6), `crates/craftsman-rs/src/harness_profiles/nemotron.rs:12` (7), `crates/craftsman-rs/src/harness_profiles/nemotron.rs:13` (7), `crates/craftsman-rs/src/harness_profiles/nemotron.rs:14` (7), `crates/craftsman-rs/src/harness_profiles/tests.rs:42` (6), `crates/craftsman-rs/src/harness_profiles/tests.rs:43` (7), `crates/craftsman-rs/src/harness_profiles/tests.rs:226` (7), `crates/craftsman-rs/src/harness_profiles/tests.rs:229` (6), `crates/craftsman-rs/src/harness_profiles/tests.rs:232` (7).
- URLs in tests: `crates/craftsman-rs/src/web_tools.rs:776` (6), `crates/craftsman-rs/src/web_tools.rs:777` (6), `crates/craftsman-rs/src/web_tools.rs:826` (8).
- Test scripts and test data: `crates/craftsman-rs/src/lsp.rs:1221` (6), `crates/craftsman-rs/src/mcp.rs:790` (6), `crates/craftsman-rs/tests/loop_test.rs:131` (6), `crates/craftsman-rs/tests/loop_test.rs:211` (7).
- Short validation, status or UI messages and one test assertion: `crates/deepagents-rs/src/backend.rs:626` (6), `crates/craftsman-rs/src/a2ui.rs:79` (7), `crates/craftsman-rs/src/agents.rs:43` (6), `crates/craftsman-rs/src/desktop_serve.rs:1152` (6), `crates/craftsman-rs/src/platform_skills.rs:23` (6), `crates/craftsman-rs/src/serve.rs:264` (6), `crates/craftsman-rs/src/tui_repl.rs:2378` (6), `crates/craftsman-rs/src/workspace_diff.rs:46` (7), `crates/craftsman-rs/src/workspace_diff.rs:61` (6).

### 5.3 Comments, headers and manifests that say text is verbatim, byte-matched or follows upstream

38 entries. *Kind*: `header` is a module-level `//!` block, `doc` a `///` item comment, `inline` a `//` comment in code. These include the 29 comment lines the plan counts in A29 (lines naming deepagents, Deep Agents, LangChain or LangGraph) and the other lines that claim copying or matching. No file header carries a copyright notice, license text or SPDX identifier.

| Where | Kind | Text | What it says |
|---|---|---|---|
| `crates/deepagents-rs/Cargo.toml:6` | manifest | `(Rust port of LangChain deepagents)` | the crate describes itself as a port |
| `crates/deepagents-rs/src/backend.rs:6` | doc | `/// Byte-for-byte match with the Python deepagents empty-read sentinel.` | constant 1 is byte-identical to the Python sentinel |
| `crates/deepagents-rs/src/backend.rs:22` | doc | ``/// `cat -n`-style line numbering matching Python deepagents.`` | line-number format matches |
| `crates/deepagents-rs/src/backend.rs:132` | doc | `/// byte-matched to Python deepagents.` | file semantics and error strings are byte-matched |
| `crates/deepagents-rs/src/backend.rs:199` | inline | `// Not-found applies to replace_all too — Python deepagents checks` | edit behavior follows |
| `crates/deepagents-rs/src/builtin/memory.rs:3` | header | ``//! round. Wording is adapted from upstream deepagents' `MEMORY_SYSTEM_PROMPT`.`` | prompt wording adapted (constant 7) |
| `crates/deepagents-rs/src/builtin/memory.rs:5` | header | ``//! Unlike upstream (which loads in `before_agent` off the LangGraph runtime), our`` | design contrast with the LangGraph-based original |
| `crates/deepagents-rs/src/builtin/memory.rs:19-20` | doc | `/// Guidance block appended after the loaded memory contents. Adapted (trimmed)` | guidance block adapted and trimmed (constant 7) |
| `crates/deepagents-rs/src/builtin/memory.rs:77` | inline | `// A missing source is not an error — skip it (upstream skips` | missing-source behavior follows |
| `crates/deepagents-rs/src/builtin/subagents.rs:1` | header | ``//! `SubAgentMiddleware`: the deepagents `task`/`send_message` tools that let a`` | tools modelled on upstream's |
| `crates/deepagents-rs/src/builtin/subagents.rs:59` | doc | ``/// Task-tool description — wording follows deepagents' `TASK_TOOL_DESCRIPTION`.`` | constant 3 follows upstream's wording |
| `crates/deepagents-rs/src/builtin/todos.rs:9` | doc | `/// Prompt section — wording follows upstream TodoListMiddleware.` | constant 4 follows upstream's wording |
| `crates/deepagents-rs/src/agent.rs:3` | header | ``//! `run_turn` drives the LangGraph-style agent cycle: push the user message,`` | loop modelled on LangGraph's |
| `crates/deepagents-rs/src/middleware.rs:10` | doc | ``/// `after_model` verdict — the used subset of LangGraph's conditional edges.`` | `Decision` is a subset of LangGraph's edges |
| `crates/deepagents-rs/src/middleware.rs:15` | doc | `/// Re-run the model immediately (upstream's jump to "model").` | `Remodel` mirrors upstream's jump to model |
| `crates/deepagents-rs/src/middleware.rs:172` | doc | `/// Hooks run in reverse list order (upstream after-hook order); the first` | hook order follows upstream |
| `crates/deepagents-rs/src/state.rs:23` | doc | ``/// Upstream `_file_data_reducer`: right side wins; `None` value deletes the key.`` | reducer semantics follow upstream |
| `crates/deepagents-rs/src/subagent.rs:19-20` | doc | ``/// Declarative description of a subagent the `task` tool can spawn. Mirrors`` | subagent config mirrors upstream's |
| `crates/deepagents-rs/src/subagent.rs:89` | doc | `/// per-turn system extras — deepagents excludes messages/todos/structured` | state-exclusion rule follows upstream |
| `crates/deepagents-rs/src/interrupt.rs:29` | doc | `/// The human's resolution — deepagents' HITL action set.` | HITL actions follow upstream's |
| `crates/craftsman-rs/src/agent.rs:1-2` | header | ``//! The agent: craftsman-rs runs on the `deepagents-rs` framework (the Rust port`` | the framework is the Rust port of deepagents |
| `crates/craftsman-rs/src/agent.rs:97-99` | doc | `/// Fraction of a model's context window at which the token-based compaction` | compaction fraction 0.85 matches deepagents' default |
| `crates/craftsman-rs/src/agent.rs:103-104` | doc | `/// Token budget for the kept tail under the token-based trigger — matches` | 20,000-token tail matches OpenAI Codex's constant |
| `crates/craftsman-rs/src/ask_user.rs:1` | header | ``//! The `ask_user` tool — mid-task clarification (deepagents `ask_user` parity).`` | tool name and behavior follow upstream's |
| `crates/craftsman-rs/src/backend_fs.rs:2` | header | ``//! hardened operations in [`crate::tools`] (byte-matched to deepagents 0.3.8)`` | tool operations byte-matched to release 0.3.8 |
| `crates/craftsman-rs/src/harness_profiles/providers.rs:7` | inline | `// Mirrors Deep Agents' NVIDIA app-origin attribution profile. This only` | NVIDIA profile mirrors upstream's |
| `crates/craftsman-rs/src/mcp.rs:355` | doc | `/// LLM consumes), mirroring langchain-mcp-adapters' text extraction.` | text extraction mirrors langchain-mcp-adapters |
| `crates/craftsman-rs/src/serve.rs:5` | header | `//! the SAME deepagents loop the terminal REPL uses, streaming tokens back as` | serve mode uses the same loop |
| `crates/craftsman-rs/src/skills.rs:7` | header | `//! SKILL.md via read_file — the same protocol deepagents uses.` | skills protocol follows upstream's |
| `crates/craftsman-rs/src/skills.rs:93` | doc | `/// System-prompt block listing available skills (deepagents-style protocol).` | skills prompt block follows upstream's |
| `crates/craftsman-rs/src/store_fs.rs:8` | header | `//! unique-edit/glob/grep) are byte-matched to the framework's` | semantics byte-matched to `StateFs`, itself byte-matched to upstream |
| `crates/craftsman-rs/src/store_fs.rs:14` | header | ``//! keys include the `/memories/` prefix (matching upstream deepagents, whose`` | path routing follows upstream's composite backend |
| `crates/craftsman-rs/src/store_fs.rs:173` | inline | `// (Python deepagents checks before the replace_all branch as well).` | edit behavior follows |
| `crates/craftsman-rs/src/threads_db.rs:3` | header | ``//! The retired Python client also used `threads.db`, but with a LangGraph`` | the retired Python client's database used a LangGraph schema (compatibility code) |
| `crates/craftsman-rs/src/tools.rs:1-2` | header | `//! Host tools — same names and argument shapes as the Python CLI's` | tool names and argument shapes follow `LocalShellBackend`'s |
| `crates/craftsman-rs/src/tools.rs:145` | doc | `/// deepagents contract: numbering starts at 1 (models are trained on it —` | line numbering contract follows upstream's |
| `crates/craftsman-rs/src/tools.rs:174` | doc | ``/// Create `path` with `content`. deepagents parity: write_file only CREATES —`` | `write_file` refuses to overwrite, like upstream |
| `crates/craftsman-rs/src/tools.rs:494` | inline | `// read_file returns cat -n format (deepagents parity).` | `read_file` output format follows upstream's |

Other products named in comments (not upstream; the text of these products was not checked): `crates/deepagents-rs/src/agent.rs:107`, `crates/deepagents-rs/src/builtin/budgets.rs:7`, `crates/deepagents-rs/src/builtin/compaction.rs:1`, `crates/deepagents-rs/src/builtin/compaction.rs:48`, `crates/deepagents-rs/src/builtin/reminders.rs:2`, `crates/deepagents-rs/src/checkpoint.rs:52`, `crates/craftsman-rs/src/checkpoints.rs:10`, `crates/craftsman-rs/src/agent.rs:104`. The message of commit `9d0e167` lists "opencode, Claude Code, OpenAI Codex, LangChain deepagents" as products studied.

### 5.3b Lines the plan's word list matches that do not refer to upstream

The plan's word list (`parity`, `mirror`, `byte-matched`, `upstream`, `port of`) matches 73 compiled lines (A29). Those about deepagents, LangChain or the Craftsman Python CLI are in 5.3, 5.6 and 5.8. Of the others, 24 were read and use the words in another sense; 3 more mirror a Python or SDK source and are cited in 5.8:

| What the line means | Lines |
|---|---|
| One part of Craftsman mirrors another part of Craftsman (its own channel design, REPL, picker, frontend kit, workspace addon, subagent pattern) | `crates/deepagents-rs/src/agent.rs:871`, `crates/deepagents-rs/src/middleware.rs:60`, `crates/deepagents-rs/src/model.rs:51`, `crates/deepagents-rs/src/subagent.rs:50`, `crates/craftsman-rs/src/acp_bridge.rs:17`, `crates/craftsman-rs/src/agent.rs:1033`, `crates/craftsman-rs/src/desktop_serve.rs:78`, `crates/craftsman-rs/src/inbox_poller.rs:138`, `crates/craftsman-rs/src/inbox_poller.rs:273`, `crates/craftsman-rs/src/inbox_poller.rs:289`, `crates/craftsman-rs/src/picker.rs:191`, `crates/craftsman-rs/src/serve.rs:11`, `crates/craftsman-rs/src/serve.rs:227`, `crates/craftsman-rs/src/skills_browser.rs:7` |
| "Parity" in another sense (the old `/clear`, a UI label, a token estimate against the provider's count) | `crates/craftsman-rs/src/agent.rs:1186`, `crates/craftsman-rs/src/route.rs:20`, `crates/craftsman-rs/src/token_counter.rs:8` |
| "Upstream" in the network sense: the provider, proxy or server side of a connection | `crates/craftsman-rs/src/harness_profiles/providers.rs:8`, `crates/craftsman-rs/src/inbox_poller.rs:226`, `crates/craftsman-rs/src/llm.rs:165`, `crates/craftsman-rs/src/llm.rs:271`, `crates/craftsman-rs/src/llm.rs:272`, `crates/craftsman-rs/src/llm.rs:285`, `crates/craftsman-rs/src/serve.rs:77` |
| Mirror a Python or SDK source and are listed in 5.8 instead | `crates/craftsman-rs/src/tools.rs:366`, `crates/craftsman-rs/src/platform_rest.rs:132`, `crates/craftsman-rs/src/mcp.rs:154-155` |

Every one of the 73 lines is cited in 5.3, 5.3b, 5.6 or 5.8 (checked by script).

### 5.4 Names and shapes kept from upstream

These are interface names and shapes, not prose. They are listed for completeness; this review does not assess whether names raise any issue.

| What | Craftsman | Upstream | Note |
|---|---|---|---|
| `AgentMiddleware` trait and its six hook names | `crates/deepagents-rs/src/middleware.rs:92`, hooks `crates/deepagents-rs/src/middleware.rs:98-122` | `lc-1.2.4 types.py:343`; hooks at `lc-1.2.4 types.py:364, 388, 412, 436, 583, 607` | Same type name and the same six hook names; signatures differ. P1 renames the hooks (Appendix B). |
| `TodoListMiddleware` and the `write_todos` tool | `crates/deepagents-rs/src/builtin/todos.rs:12`, `crates/deepagents-rs/src/builtin/todos.rs:32` | `lc-1.2.4 todo.py:133` | Names only; the prompt text is constant 4. |
| `PatchToolCallsMiddleware` | `crates/deepagents-rs/src/builtin/patch_tool_calls.rs:8` | `0.3.8 middleware/patch_tool_calls.py:11` | Name only; behavior differs (Craftsman parses string-encoded arguments, upstream adds results for dangling tool calls). |
| `SubAgentMiddleware`, the `task` tool, the `general-purpose` subagent | `crates/deepagents-rs/src/builtin/subagents.rs:119`, `crates/deepagents-rs/src/builtin/subagents.rs:325` | `0.3.8 middleware/subagents.py:445`, `0.3.8 middleware/subagents.py:438`, `0.3.8 middleware/subagents.py:298` | Names and the `subagent_type` argument follow upstream; descriptions are constants 3 and 9. |
| `MemoryMiddleware` | `crates/deepagents-rs/src/builtin/memory.rs:52` | `0.3.8 middleware/memory.py:155` | Name only; prompt text is constant 7. |
| `CompositeBackend` routing `/memories/` | `crates/craftsman-rs/src/store_fs.rs:235` | `0.3.8 backends/composite.py:37` | Name and routing idea; see also the header at `crates/craftsman-rs/src/store_fs.rs:14`. |
| Host tool names and argument names (`execute`, `read_file`, `write_file`, `edit_file`, `ls`, `glob`, `grep`; `file_path`, `offset`, `limit`, `old_string`, `new_string`, `replace_all`) | `crates/craftsman-rs/src/agent.rs:418, 427, 442, 456, 472, 481, 492` | `0.3.8 middleware/filesystem.py:332, 391, 466, 545, 592, 651, 760`; arguments at `0.3.8 middleware/filesystem.py:355-358` and `0.3.8 middleware/filesystem.py:489-494` | Names and argument shapes kept on purpose (`crates/craftsman-rs/src/tools.rs:1-2`, `README.md:287`). Descriptions: section 5.2. |
| `ask_user` tool | `crates/craftsman-rs/src/ask_user.rs:34` | `main libs/code/deepagents_code/ask_user.py:298` | Tool name only; the description wording differs (no shared 6-word run). |
| Profile types and field names (`HarnessProfile`, `HarnessProfileConfig`, `GeneralPurposeSubagentProfile`, `ProviderProfile`; `base_system_prompt`, `system_prompt_suffix`, `tool_description_overrides`, `excluded_tools`, `excluded_middleware`, `general_purpose_subagent`) | `crates/deepagents-rs/src/profiles/harness.rs:18`, `crates/deepagents-rs/src/profiles/harness.rs:51`, `crates/deepagents-rs/src/profiles/harness.rs:96`, `crates/deepagents-rs/src/profiles/provider.rs:19` | `main profiles/harness/harness_profiles.py:83`, `main profiles/harness/harness_profiles.py:192`, `main profiles/harness/harness_profiles.py:483`, `main profiles/provider/provider_profiles.py:38` | Same type names and field names (upstream fields at `main profiles/harness/harness_profiles.py:256-308`; Craftsman fields at `crates/deepagents-rs/src/profiles/harness.rs:53-63`). Upstream added its profiles API on 2026-04-28 (#2892). Appendix B leaves the rename of these types open. |
| Model identifier lists that choose which models share a profile | `crates/craftsman-rs/src/harness_profiles/openai.rs:7`, `crates/craftsman-rs/src/harness_profiles/claude.rs:9`, `crates/craftsman-rs/src/harness_profiles/nemotron.rs:10-15` | `main profiles/harness/_openai_codex.py:28-30`, `main profiles/harness/_anthropic_sonnet_4_6.py:50`, `main profiles/harness/_nvidia_nemotron_3_ultra.py:46-54` | Model names are public identifiers; the grouping mirrors upstream's registry. |
| Billing-origin header `X-BILLING-INVOKE-ORIGIN: DeepAgents` on the `nvidia` provider profile (A20, decision 2) | `crates/craftsman-rs/src/harness_profiles/providers.rs:15` | `main profiles/provider/_nvidia.py:21`, `main profiles/provider/_nvidia.py:24` | Upstream's identifier for itself, sent to a third party from Craftsman. Added upstream 2026-07-02 (#4455). |
| Compaction trigger fraction 0.85 | `crates/craftsman-rs/src/agent.rs:102` | `0.3.8 graph.py:150`, `main middleware/summarization.py:263` | A numeric default equal to upstream's; the comment cites a function that exists at main but not at 0.3.8. |

### 5.5 README and documents that map the project to upstream

The README table "What replaces what" (lines 262-274) and the "Parity matrix" list (276-357) are what the plan's P1 calls the mapping table and the parity matrix. No passage of the README, `AGENTS.md`, the documents under `docs/`, the examples or `install.sh` shares more than 9 words in a row with a file of the `deepagents` and `langchain` corpora of section 3 (the 9-word runs are an `npx` MCP example in `examples/config/mcp-servers.json` and shell or CI boilerplate in `install.sh` and `.github/workflows/release.yml`). The only longer runs are boilerplate in the CI workflow and the Dockerfile: 12 words at `.github/workflows/ci.yml:27-29` (the concurrency block) and 13 words at `docker/service-agent.Dockerfile:12-13` (an `apt-get install` line). The four golden request files are the exception, because they hold the prompts themselves (5.6).

| Where | Text | What it is |
|---|---|---|
| `README.md:3` | ``A Rust workspace containing the **`deepagents-rs`** agent framework and`` | names the crate |
| `README.md:17` | `deepagents-rs/   framework: AgentMiddleware protocol (6 hooks), Backend` | describes the middleware protocol and its six hooks |
| `README.md:262` | `## What replaces what` | heading of the mapping table (rows 264-274) |
| `README.md:264` | `Legacy implementation` → `Current Rust implementation` | header row of the mapping table |
| `README.md:266` | `deepagents / LangGraph` → `` `deepagents-rs` framework crate (middleware protocol, backends, DeepAgent loop) `` | maps upstream to the crate |
| `README.md:267` | `deepagents middleware (todos, patch)` → `` `deepagents-rs::builtin` (`TodoListMiddleware`, `PatchToolCallsMiddleware`) `` | maps upstream middleware to Craftsman's |
| `README.md:268` | `` langchain-openai `ChatOpenAI` `` → `` `StreamingLlmProvider` over the SSE client (`llm.rs`) `` | maps a LangChain class to Craftsman's client |
| `README.md:269` | `` deepagents `LocalShellBackend` `` → `` `RealFs` Backend (`backend_fs.rs`) over hardened `tools.rs` ops `` | maps upstream's backend to Craftsman's |
| `README.md:273` | `langchain-mcp-adapters` → ``minimal MCP client (`mcp.rs`) ...`` | maps a LangChain package to Craftsman's client |
| `README.md:276` | `## Parity matrix (honest)` | heading of the parity list (lines 278-357) |
| `README.md:285` | `Tool-calling agent loop on the deepagents-rs middleware framework:` | claims parity on the loop |
| `README.md:287` | `` `grep` — same names and arg shapes deepagents exposes `` | claims upstream's tool names and argument shapes |
| `README.md:288` | `` `write_todos` planning (deepagents `TodoListMiddleware`) and malformed `` | claims parity on planning |
| `README.md:296-297` | ``Subagents / `task` tool + background execution (deepagents`` | claims parity on subagents |
| `README.md:303` | ``dispatched (ungated, matching Python's `interrupt_on`): streamable-HTTP`` | refers to the Craftsman Python CLI |
| `README.md:356-357` | `` `threads-rs.db` data is moved automatically; a legacy Python `threads.db` is `` | legacy database archive |
| `docs/xm-10-agent-capability.md:22-23` | ``Yes. `craftsman-rs` is built on the `deepagents-rs` framework (a Rust port of`` | describes the framework as a port |
| `docs/xm-10-agent-capability.md:43` | `the deepagents plan-track pattern` | names upstream's pattern |
| `AGENTS.md:2` | ``Purpose: Rust workspace for the Craftsman CLI — the `craftsman` terminal client / workspace-agent runtime plus the `deepagents-rs` agent framework.`` | names the crate only |
| `docs/platform-token-refactor.md:65` | ``- No changes to `deepagents-rs` (framework is untouched; `dep_guard` green).`` | names the crate only |

### 5.6 Tests and fixtures derived from upstream

No Rust test function name equals an upstream test name once a leading `test_` is dropped (728 upstream functions named `test*` at 0.3.8, 13,285 at main; the Rust sources have 582 test functions). These tests and fixtures carry upstream or Python-CLI terms in their names or content:

| Where | Text | What it is |
|---|---|---|
| `crates/deepagents-rs/src/middleware.rs:259` | `async fn hooks_compose_in_upstream_order() {` | test name refers to upstream's hook order |
| `crates/deepagents-rs/src/middleware.rs:340` | `async fn tool_hooks_compose_in_upstream_order() {` | test name refers to upstream's hook order |
| `crates/deepagents-rs/src/state.rs:136` | `right.insert("a.txt".into(), None); // None deletes (upstream semantics)` | assertion comment in the file-reducer test (reducer at `crates/deepagents-rs/src/state.rs:23`) |
| `crates/deepagents-rs/src/backend.rs:626` | `assert!(out.contains("File exists but has empty contents"));` | assertion on the shared sentinel text (constant 1) |
| `crates/craftsman-rs/src/harness_profiles/tests.rs:37` | `fn known_upstream_models_resolve_to_exact_profile_keys() {` | test name; the fixtures at `crates/craftsman-rs/src/harness_profiles/tests.rs:39-44` are model ids from upstream's registry |
| `crates/craftsman-rs/src/harness_profiles/tests.rs:181` | `serde_json::json!({"X-BILLING-INVOKE-ORIGIN": "DeepAgents"})` | fixture asserts the `DeepAgents` header value (A20) |
| `crates/craftsman-rs/tests/loop_test.rs:227` | `.and(header("x-billing-invoke-origin", "DeepAgents"))` | mock-server matcher on the `DeepAgents` header value (A20) |
| `crates/craftsman-rs/tests/loop_test.rs:211` | `assert!(description.contains("always read a file before editing it"));` | assertion on constant 15's closing words |
| `crates/craftsman-rs/src/agents.rs:89` | `fn default_agents_md_matches_python_shape() {` | test name; the shape is the Craftsman Python CLI's (constant 19) |
| `crates/craftsman-rs/src/config.rs:508` | `fn model_profile_matches_python_config_shape_and_returns_a_copy() {` | test name refers to the Craftsman Python CLI's config |
| `crates/craftsman-rs/src/config.rs:666` | `fn defaults_match_python() {` | test name refers to the Craftsman Python CLI's defaults |
| `crates/craftsman-rs/src/tools.rs:580` | `fn gated_set_matches_python_interrupt_on() {` | test name refers to the Craftsman Python CLI's gating |
| `crates/craftsman-rs/tests/loop_test.rs:223` | `async fn one_shot_applies_harness_provider_and_python_model_profiles() {` | test name refers to the Craftsman Python CLI's model profiles |
| `crates/craftsman-rs/src/threads_db.rs:242` | `async fn create_python_db(path: &Path) {` | fixture builds a LangGraph-schema `checkpoints` table (`crates/craftsman-rs/src/threads_db.rs:242-254`) that the retired Python client used |
| `crates/craftsman-rs/src/threads_db.rs:278` | `async fn python_database_is_archived_before_rust_database_moves() {` | test name; legacy-database migration test |
| `crates/craftsman-rs/src/threads_db.rs:296` | `async fn python_only_database_is_archived_for_a_fresh_rust_store() {` | test name; legacy-database migration test |
| `crates/craftsman-rs/tests/golden_model_request.rs:1` | ``//! Golden model requests: the exact JSON body the real `craftsman` binary sends to the`` | test that pins the request the model receives for each harness profile; the four files it compares with (`crates/craftsman-rs/tests/golden/model_request/claude.json`, `general.json`, `nemotron.json` and `openai.json`) hold the system prompt and the tool descriptions as sent, so they contain the text of the prompt constants and of the strings in 5.2. Measured like the other tracked text files, on the JSON strings decoded: the longest identical run is 206 words with `0.3.8` (text of constant 3), 121 with `lc-1.2.4` (the `write_todos` prompt of constant 4), 82 with `lc-master` and 58 with `main`; 17.5% to 19.7% of their 6-word windows are found in `0.3.8`. `AGENTS.md` tells contributors to regenerate them when a prompt or tool description changes, so they follow any rewording in P5 |

### 5.7 File headers that credit upstream

The `header` rows in 5.3 are the headers that credit upstream in prose. A search of all tracked files except `Cargo.lock`, `NOTICE` and this review for `copyright`, `SPDX`, `license`, `licence`, `MIT` (as a word) and `Apache` found only: `crates/deepagents-rs/Cargo.toml:7` (the MIT declaration), `crates/craftsman-rs/src/a2ui.rs:26` and `crates/craftsman-rs/src/data/a2ui_v0_9_schema.json:2` (the Apache-2.0 credit for the A2UI schema), the plan, and `docs/superpowers/specs/2026-07-27-voice-input-design.md:92`, `docs/superpowers/specs/2026-07-27-voice-input-design.md:94` and `docs/superpowers/specs/2026-07-27-voice-input-design.md:107` of the voice-input design document (statements about dependency and model licenses). No source file has a copyright notice, license text or SPDX identifier.

### 5.8 Text said to come from the Craftsman Python CLI (not from upstream)

66 lines in 22 files contain "Python": the 65 lines in 21 compiled `.rs` files that A29 counts and one in a manifest (`crates/craftsman-rs/Cargo.toml:61`). Outside the Rust sources, "Python" appears in two README lines (5.5) and in two lines of the voice-input design document (`docs/superpowers/specs/2026-07-27-voice-input-design.md:103` says the voice feature needs no Python runtime; `docs/superpowers/specs/2026-07-27-voice-input-design.md:246` concerns round-trip safety of the Python CLI's config), neither about upstream. Almost all refer to Craftsman's own predecessor, `craftsman_cli`, whose source is not in this repository; five lines refer to "Python deepagents" and are in 5.3; six lines concern the retired Python client's database. That CLI used deepagents as a library (`README.md:266`), and the first commit of this repository describes the Rust code as a port of it (`98500f4`: "initial Rust port of craftsman-harness"). None of the Python CLI's function or constant names that the Rust comments cite (`build_system_prompt`, `DEFAULT_AGENTS_MD`, `NONINTERACTIVE_INSTRUCTIONS`, `pick_from_list`, `compute_status`, `format_row`, `download_and_extract`) appears in the upstream archives, but that does not show that none of the CLI's text came from upstream; for `NONINTERACTIVE_INSTRUCTIONS` the text itself lines up with an upstream passage (5.1, constant 16).

| File | Lines containing "Python" | What the lines refer to |
|---|---|---|
| `crates/craftsman-rs/src/platform_skills.rs` | 1, 6, 78, 112, 232, 252, 296 | the Craftsman Python CLI (`craftsman_cli`) or a Python library it used |
| `crates/craftsman-rs/src/config.rs` | 1, 3, 133, 213, 291, 460 | the Craftsman Python CLI (`craftsman_cli`) or a Python library it used |
| `crates/craftsman-rs/src/hitl.rs` | 1, 6, 72, 204, 249, 288 | the Craftsman Python CLI (`craftsman_cli`) or a Python library it used |
| `crates/craftsman-rs/src/serve.rs` | 3, 12, 48, 49, 58, 542 | the Craftsman Python CLI (`craftsman_cli`) or a Python library it used |
| `crates/craftsman-rs/src/agents.rs` | 1, 4, 9, 63, 84 | the Craftsman Python CLI (`craftsman_cli`) or a Python library it used |
| `crates/craftsman-rs/src/a2ui.rs` | 1, 22, 53, 191 | the Craftsman Python CLI (`craftsman_cli`) or a Python library it used |
| `crates/craftsman-rs/src/picker.rs` | 2, 11, 16 | the Craftsman Python CLI (`craftsman_cli`) or a Python library it used |
| `crates/craftsman-rs/src/skills.rs` | 1, 46, 47 | the Craftsman Python CLI (`craftsman_cli`) or a Python library it used |
| `crates/craftsman-rs/src/hitl_middleware.rs` | 6, 11 | the Craftsman Python CLI (`craftsman_cli`) or a Python library it used |
| `crates/craftsman-rs/src/platform_rest.rs` | 16, 132 | the Craftsman Python CLI (`craftsman_cli`) or a Python library it used |
| `crates/craftsman-rs/src/prompt.rs` | 1, 11 | the Craftsman Python CLI (`craftsman_cli`) or a Python library it used |
| `crates/craftsman-rs/src/skills_browser.rs` | 2, 51 | the Craftsman Python CLI (`craftsman_cli`) or a Python library it used |
| `crates/craftsman-rs/src/tools.rs` | 1, 147 | the Craftsman Python CLI (`craftsman_cli`) or a Python library it used |
| `crates/craftsman-rs/src/agent.rs` | 773 | the Craftsman Python CLI (`craftsman_cli`) or a Python library it used |
| `crates/craftsman-rs/src/main.rs` | 199 | the Craftsman Python CLI (`craftsman_cli`) or a Python library it used |
| `crates/craftsman-rs/src/mcp.rs` | 154 | the Craftsman Python CLI (`craftsman_cli`) or a Python library it used |
| `crates/craftsman-rs/src/theme.rs` | 14 | the Craftsman Python CLI (`craftsman_cli`) or a Python library it used |
| `crates/craftsman-rs/src/threads_db.rs` | 3, 56, 81, 86, 146 | the retired Python client's LangGraph-schema database (compatibility code) |
| `crates/deepagents-rs/src/backend.rs` | 6, 22, 132, 199 | "Python deepagents": the upstream project (also in 5.3) |
| `crates/craftsman-rs/src/checkpoints.rs` | 7 | the retired Python client's LangGraph-schema database (compatibility code) |
| `crates/craftsman-rs/src/store_fs.rs` | 173 | "Python deepagents": the upstream project (also in 5.3) |
| `crates/craftsman-rs/Cargo.toml` | 61 | the Craftsman Python CLI (`craftsman_cli`) |

Lines that claim a copy of, or a mirror of, the Python CLI's text or behavior:

| Where | Text | What it says |
|---|---|---|
| `crates/craftsman-rs/src/a2ui.rs:1-2` | ``//! A2UI (Agent-to-UI) support for `craftsman-rs serve` — mirror of the Python`` | mirror of the Python CLI's `a2ui.py` |
| `crates/craftsman-rs/src/a2ui.rs:190-191` | `/// basic catalog, so names/props below are authoritative. VERBATIM copy of` | constant 18 is a verbatim copy of the Python CLI's block |
| `crates/craftsman-rs/src/agents.rs:1` | ``//! Agent profiles — mirror of Python `agents.py` + the AGENTS.md seeding half`` | agent profiles mirror `agents.py` and `memory.py` |
| `crates/craftsman-rs/src/agents.rs:9-11` | `/// Seeded into a new agent's AGENTS.md — verbatim from Python` | constant 19 is verbatim from `memory.DEFAULT_AGENTS_MD` |
| `crates/craftsman-rs/src/prompt.rs:1` | ``//! System prompt assembly — mirror of Python `memory.build_system_prompt`:`` | prompt assembly mirrors `memory.build_system_prompt` |
| `crates/craftsman-rs/src/prompt.rs:11-12` | `/// Appended in one-shot (-m) mode — mirror of Python` | constant 16 mirrors `NONINTERACTIVE_INSTRUCTIONS` (`chat.py`) |
| `crates/craftsman-rs/src/hitl.rs:1` | ``//! Human-in-the-loop approval gate — mirror of Python `approval.py` +`` | approval gate mirrors `approval.py` |
| `crates/craftsman-rs/src/hitl.rs:288` | `/// The tool result the agent sees on rejection — mirrors Python hitl.py's` | rejection text mirrors `hitl.py` |
| `crates/craftsman-rs/src/serve.rs:3` | ``//! Mirror of the Python CLI's `serve.py`: expose the agent over a WebSocket`` | serve mode mirrors `serve.py` |
| `crates/craftsman-rs/src/skills.rs:1` | ``//! Skills discovery — mirror of Python `skills.py`.`` | skills discovery mirrors `skills.py` |
| `crates/craftsman-rs/src/skills_browser.rs:2` | ``//! Rust port of Python `skills_browser.py`.`` | skills browser is a port of `skills_browser.py` |
| `crates/craftsman-rs/src/picker.rs:2` | ``//! port of the Python CLI's `picker.py` (`pick_from_list`).`` | picker is a port of `picker.py` |
| `crates/craftsman-rs/src/platform_skills.rs:1` | ``//! Platform skill sync engine — Rust port of Python `platform_skills.py`.`` | sync engine is a port of `platform_skills.py` |
| `crates/craftsman-rs/src/main.rs:199` | ``/// Headless one-shot (Python `craftsman -m` parity): stream to stdout, exit.`` | one-shot mode follows the Python CLI |
| `crates/craftsman-rs/src/config.rs:1` | ``//! Config compatible with the Python CLI's `~/.craftsman/config.json`.`` | config file shared with the Python CLI |
| `crates/craftsman-rs/src/platform_rest.rs:15-16` | ``/// Re-homed verbatim from `craftsman-sdk` (`core/src/url.rs`, which mirrored`` | URL normalization re-homed verbatim from the removed `craftsman-sdk` crate (an in-house crate, not upstream) |
| `crates/craftsman-rs/src/tools.rs:366` | `/// mirrors turn_view._summarize_call.` | tool-call summaries mirror the Python CLI's `turn_view._summarize_call` |
| `crates/craftsman-rs/src/platform_rest.rs:132` | `// Table mirrored from the SDK / Python normalization tests.` | the URL-normalization test table mirrors the removed SDK's and the Python CLI's tests |
| `crates/craftsman-rs/src/mcp.rs:154-155` | `A user-configured MCP server from` | MCP server loading follows the Python CLI's `_server_specs` (the comment continues on the next line) |

## 6. Verified and unverified

**Verified on 2026-10-16 and 2026-10-17 (UTC)** (how is in sections 3 and 4):

- The upstream project, its MIT license, both copyright lines, the exact bytes of both `LICENSE` files and that `NOTICE` reproduces them (compared with the files and with fresh downloads of the URLs printed in `NOTICE`); that LangChain's `LICENSE` equals the first reproduced text; which releases carry which line (81 stable tags, as of 2026-10-16) and that no upstream NOTICE file exists.
- That all 20 named constants exist where this document says, and the plan's A19 and A29 counts, at `6c64777`, `03c38ae` and `0b77980` (five of the seven equal the plan's at `6c64777` and all seven at the other two; section 3).
- References and quotations: 266 distinct references into this repository and 85 into the upstream archives, 126 quotations in tables, 31 double-quoted passages in the prose and 2 word runs in curly quotes were checked by script and are all found at the cited place. Every one of the 29 comment lines of A29 is cited in 5.3, every one of the 65 "Python" lines in 5.8, and every one of the 73 word-list lines in 5.3, 5.3b, 5.6 or 5.8.
- That constants 1 and 3 are verbatim (character level and word level agree), that constants 4, 9 and 12 share long runs with the sources named, and that the 11 occurrences of four error strings equal upstream's once placeholders are blanked.
- That all 20 constants were compared with six corpora (section 3). 12 verdicts also rest on reading the texts side by side (the rows marked read or measured, read in 5.1).
- The commits that first added each constant (`git log -S` on its current text), and that 10 of the 14 commits that first added the 20 constants carry a Co-Authored-By trailer that names an Anthropic assistant (`98500f4`, `13416eb`, `b3364e2`, `b98394b`, `3a32b68`, `07d5ca1`, `409f8bb`, `a88682e`, `9dcb7f9`, `6b96952`); the others (`36f99bb`, `c524c18`, `b5f7255`, `95d3c16`) do not.
- Every count stated in this document was recomputed from the tables, the tree or git by a second script, and agrees.

**Unverified or judged**

- *Which upstream version was followed.* `crates/craftsman-rs/src/backend_fs.rs:2` names 0.3.8, but `crates/craftsman-rs/src/agent.rs:99` cites `compute_summarization_defaults`, which exists at `main` and not at 0.3.8, and constants 12 to 15 follow upstream files added in 2026-04 and 2026-07. More than one version was consulted. Releases other than `0.3.8`, `0.3.11`, `0.3.12`, `main` and `cli-0.0.10` were not compared for text, apart from the single files named in section 3.
- *Paraphrase verdicts* (constants 7, 11, 13, 14 and 15 by reading alone, constant 16 by a character-level overlap and reading) are this review's reading of two texts, not a measurement, and the *no match found* verdicts for constants 5, 8, 10, 17 and 19 rest partly on reading the likeliest counterpart. Constants 11 and 13 share no wording with the upstream passages; they cover the same points in the same order, and those points are generic guidance that other sources contain.
- *The Craftsman Python CLI.* Its source is not available. Whether text it contributed (constants 16 to 19, and the files in 5.8) was itself taken from upstream or from another source is unknown; for constant 16 an upstream passage exists in upstream's own CLI package, so the Python CLI's text may itself descend from it.
- *Other products.* Claude Code, OpenAI Codex and opencode text was not compared.
- *A2UI.* Constant 18 was compared with the A2UI project's repository at one commit; the Python CLI's earlier copy of that block, and the A2UI documentation site, were not.
- *Anthropic's prompting guide.* Upstream says its Claude profile text matches Anthropic's published samples verbatim. Those samples were not fetched, so it is not known whether Craftsman's `COMMON_SUFFIX` or `OPUS_47_OVERLAY` also follow them.
- *Other upstream material.* Only the corpora in section 3 were compared; other `langchain` and `langgraph` files, other `deepagents` releases, and `langchain-mcp-adapters` were not.
- *Later upstream changes.* The tag counts in section 4 and the state of `main` are those of 2026-10-16; releases and commits published since were not scanned.
- *Provenance.* The commits that introduced the inherited strings do not record where the text was copied from, apart from the messages quoted in 5.1. How any string was produced (copied from the Python package, typed, or generated with an assistant) was not investigated.
- *Copyright holders* are taken from the text of the upstream `LICENSE` files. Authorship of upstream's text, and any third-party contributions to it, were not checked.
- *Dependencies and assets.* The licenses of the 405 packages in `Cargo.lock`, the A2UI repository's copyright line, the Hugging Face model and the voice benchmark file were not reviewed (section 7).
- *No paraphrase or overlap measure is a legal test.* The 6-word windows are a screening tool, and the plan's 5% figure is a plan choice, not a legal threshold.
- *The scripts* that produced the measurements and the reference checks are not part of the repository; section 3 describes what they do.

## 7. Other third-party material noticed (not inventoried, not in `NOTICE`)

| Item | Where | Facts |
|---|---|---|
| A2UI schema bundle and prompt | `crates/craftsman-rs/src/a2ui.rs:25-26`; `crates/craftsman-rs/src/data/a2ui_v0_9_schema.json:2` | The vendored JSON states its source (`github.com/a2ui-project/a2ui`, `specification/v0_9`) and "Apache-2.0". GitHub reports `Apache-2.0` for that repository. Its copyright line and any NOTICE file were not checked. Constant 18 teaches the same catalog; it shares no prose with the project's own text (5.1). |
| Voice input model | `crates/craftsman-rs/src/voice.rs:39-40` | The Whisper model is downloaded at run time from Hugging Face (`ggerganov/whisper.cpp`, a pinned revision), not shipped in the repository. The design document states its license (`docs/superpowers/specs/2026-07-27-voice-input-design.md:107`); that was not checked. |
| Voice benchmark data | `crates/craftsman-rs/src/voice.rs:1564` | `crates/craftsman-rs/src/data/voice-benchmark.pcm`, 64,000 bytes, included with `include_bytes!`; its provenance is not recorded in the repository. |
| Rust dependencies | `Cargo.lock` | 405 `[[package]]` entries, including the two workspace crates; the default `voice` feature (`crates/craftsman-rs/Cargo.toml:19`) adds native audio and speech crates. Their license terms are not reviewed here and are not in `NOTICE`. |
| Billing-origin header value | `crates/craftsman-rs/src/harness_profiles/providers.rs:15`; plan A20 | `DeepAgents` is upstream's identifier for itself, sent to NVIDIA NIM on the `nvidia` profile (5.4). This is a provider-terms question (decision 2) more than a license question; it is listed because a release could ship before P5 drops it. |

## 8. Outcome (to be completed by the owner)

| Field | Value |
|---|---|
| Review owner | to be named |
| Legal reviewer | to be named |
| Date the written outcome is due | before P5 starts |
| Written outcome | not yet written |
| `NOTICE` changes requested | none recorded |
