# P0 spike 4: stat-cache snapshot timing on a 100k-file repository

Branch `p0/spike-stat-cache` (a throwaway; this file is the only part meant to be merged). The prototype and the timing harness are in commit `b115dfb`: `crates/craftsman-rs/src/workspace_checkpoint/stat_cache_spike.rs` and `docs/spikes/`.

Question, from `docs/craftsman-harness-plan.md` section 7.2 (P0 spikes): does a stat-cache snapshot of a 100,000-file repository stay under 300 ms? Above 300 ms, the plan adds 2 weeks to P2c (now 6 to 7 engineer-weeks).

## Result

- **The shipped snapshot is far over the line: 15.4 to 17.0 s for 100,001 files** (median 15.6 s on a first snapshot, 16.1 s on a repeat, 15.7 s after one changed file, 16.6 s on a freshly cloned tree), 52 to 55 times 300 ms. A turn with three mutating tool calls spends 78.5 s in snapshots (5 snapshots). 97% of the time is the per-file open, read and close; hashing is 2.5%. An `open` costs about 28 us here even for `/dev/null`, against 1 us for `lstat`, and a Trend Micro Endpoint Security extension is active, which fits but is not proven.
- **The stat-cache snapshot is under the line, warm and with one changed file, in every variant measured.** With one thread and the safe fd-relative walk: 277 ms for a repeat (274 to 281) and 274 ms with one file changed (271 to 278). With four threads: 166 ms (165 to 170) and 167 to 169 ms. The cheaper path-based `lstat` variant: 252 and 151 ms. The one-thread margin is 23 ms (8%), and `git ls-files` alone is 89 to 105 ms of every scan.
- **So the plan's rule ("above 300 ms adds 2 weeks to P2c") is not triggered.** Recommendation: keep P2c at 6 to 7 engineer-weeks, with the scope listed below.
- **Not solved by the cache**: the first scan of a session (6.2 s with one thread, 2.8 s with four, against 15.6 s today); Windows, which I did not measure; and the obvious key (size, mtime, inode, mode) is not safe on its own (see Correctness).
- **Two findings the plan does not have**: the tuple (size, mtime, inode, mode) misses an edit that puts the old mtime back, which only ctime catches; and the shipped function fails the whole snapshot when git lists any symlink, so such repositories get no workspace checkpoints today.

## What was measured

**The function.** `capture_snapshot` in `crates/craftsman-rs/src/workspace_checkpoint.rs`. `WorkspaceCheckpointManager` calls it at turn start (`before_agent`), at turn end (`after_agent`) and after every `execute`, `write_file` and `edit_file` call (`finish_mutation`), so a turn with k mutating calls takes k + 2 snapshots. `begin_mutation` also deep-copies the baseline snapshot on every call. One snapshot is: `git ls-files -co --exclude-standard -z` (parsed, sorted, de-duplicated), then for every listed file `open_relative_read` (open the root, `openat` each parent directory, then the file, all with `O_NOFOLLOW`: 5 opens for a file three directories deep), `fstat`, a read (the whole file into memory while it is at most 5 MiB and the scan has kept at most 25 MiB, otherwise streamed in 64 KiB chunks), SHA-256 and hex formatting.

**The repository.** `/tmp/oc-p0-artifacts/repo100k`, built by `docs/spikes/p0-stat-cache-gen-repo.py` (seed 20261016). Verified, not assumed:

| | |
|---|---|
| Tracked data files | 100,000, random base64 text, 207,571,479 bytes in all (mean 2,076 B, range about 1.5 to 2.5 KiB) |
| Layout | 1,000 leaf directories `a?/b?/c?` of 100 files (1,110 directories below the root), no symlinks |
| Ignored | `.gitignore` holds `build/`; `build/` has 10,000 files of the same shape in 100 directories (39 MiB on disk) |
| Git | one commit on `main`, `git gc` run, `git status` clean; `git ls-files -co --exclude-standard` lists 100,001 paths (the data files plus `.gitignore`) |
| On disk | 597 MiB in all, `.git` 167 MiB |
| Rewriting cases | use `repo100k-work`, a `cp -cR` clone, so `repo100k` is never written |

**The machine.** Mac17,2 (Apple M5, 10 cores: 4 + 6), 24 GiB RAM (`hw.memsize` 25,769,803,776), macOS 26.6.2 (25G83, Darwin 25.6.0, arm64), APFS data volume `/dev/disk3s5` (case-insensitive, checked), rustc and cargo 1.99.0, git 2.50.1 (Apple Git-155). Builds: `cargo test --release --locked --no-default-features`, the default release profile (no LTO). The machine runs Trend Micro's Endpoint Security system extensions (`com.trendmicro.icore.es` and `.es.sa`, both activated and enabled, listed by `systemextensionsctl`), which matters below.

**Method.** One test per process. Every timed case first waits, inside the process, until the 1-minute load average is below 3, and the harness records the load it started with and the load at the end (the `load1 at start` column). 5 samples per case unless the table says 3; median, with the range as min to max. The machine is shared and its baseline load sat at 3.0 to 3.5 during the session, so cases often waited minutes for a dip. All 210 records behind these tables started with the load average between 2.80 and 3.00 (none at or above 3.0); 95 of them ended above 3.0 because the case itself adds load (the highest: 5.11 after the 10-thread run of the shipped function, 4.49 after the repeat case). Both ends are in the raw records, 210 JSON lines kept on the measuring machine in `/tmp/oc-p0-artifacts/wp5b/merged-all.jsonl` (not committed; `docs/spikes/p0-stat-cache-table.py` turns them into the tables). For the later cases the Trend Micro service (`iCoreService`) ran at about one full core against a lifetime average of 6.6% of a core. It started running hot after I created four more clones of the repository (about 444,000 files); I could not confirm that those were the cause, and the harness cannot separate that activity from the measurement.

**Cold page cache.** macOS offers no unprivileged way that I know of to drop the file cache, and `purge` empties it for every process on a shared machine, so I did not try it. "First snapshot" is a fresh process per sample on the repository itself, whose files earlier runs had left in the page cache. "Freshly cloned" is a fresh `cp -cR` clone per sample: new inodes, so the file data is not in the page cache, but the directory metadata is warm because the clone has just been created. Neither is a true cold start.

## Results

### 1. The shipped snapshot function (`capture_snapshot`)

| Case, 5 runs each | median | min to max | CPU median | load1 at start |
|---|---:|---:|---:|---:|
| First snapshot, fresh process, files in the page cache | 15,571 ms | 15,472 to 15,697 | 8,567 ms | 2.89 to 2.98 |
| First snapshot, freshly cloned tree (file data not in the page cache) | 16,647 ms | 16,455 to 16,951 | 9,075 ms | 2.89 to 3.00 |
| Repeat, same process, after one warm-up snapshot | 16,140 ms | 15,979 to 16,625 | 8,916 ms | 2.95 |
| After changing one file (one byte, same size) | 15,749 ms | 15,422 to 15,986 | 8,660 ms | 2.93 |
| Once more, inside the breakdown test | 15,928 ms | 15,466 to 16,107 | 8,775 ms | 2.94 |

- **15.4 to 17.0 s in every case, 52 to 55 times the 300 ms line.** First, repeat and one-change cost the same because the function opens, reads and hashes every listed file every time. Freshly cloned trees (file data not in the page cache) take 1.1 s longer than the runs on cached files (16,647 against 15,571 ms): the data reads are a small part of it. Peak resident memory of a first snapshot: 67 MiB (it keeps the bytes of 12,664 files, 26,214,336 bytes, under the 25 MiB cap).
- CPU time is 8.6 to 9.1 s of the 15.4 to 17.0 s of wall time; the rest is time spent blocked in the kernel.
- Around each snapshot: copying the baseline (`begin_mutation`) 3.0 ms, `diff_snapshots` 18.4 ms with nothing changed and 20.2 ms (19.2 to 20.7) with one change, dropping a snapshot 2.3 ms.
- Per turn, with k mutating calls: (k + 2) snapshots. At k = 3 that is 5 snapshots, about 80 s of snapshot time on this repository (measured end to end in section 5).

### 2. Where the time goes

One pass over the 100,001 listed files, with a timer around each stage of `capture_snapshot_file` (the same calls in the same order, over the whole list):

| Stage | median | share | per file |
|---|---:|---:|---:|
| open chain: 5 opens and 4 directory closes (`open_relative_read`) | 12,445 ms | 78.1% | 124 us |
| read | 2,625 ms | 16.5% | 26 us |
| SHA-256 and hex | 391 ms | 2.5% | 3.9 us |
| close of the file | 331 ms | 2.1% | 3.3 us |
| `fstat` | 33 ms | 0.2% | 0.3 us |
| `git ls-files -co --exclude-standard -z`, parse, sort (`enumerate_paths`) | 92 ms | 0.6% | |
| Sum | 15,933 ms | | |
| `capture_snapshot` measured whole | 15,928 ms | | |

The stages reproduce the whole function to 0.04%. The filesystem calls (open, read, close) are 97% of the time; hashing is 2.5% and `git ls-files` is 0.6%.

- **An `open` costs about 28 us on this machine, whatever it opens, and a `stat` about 1 us.** Hot, per call: `lstat` 0.9 us, `fstatat` 0.9 us, `open` + `close` of a file 28.9 us (CPU 15.1), of a directory 28.0 us, of `/dev/null` 27.7 us (CPU 14.1), `openat` + `close` 28.8 us, `openat` + `fstat` + `read` of 2 KB + `close` 29.9 us. Half of each open is time blocked, not CPU. A per-open hook fits that, and the Trend Micro Endpoint Security extension is active here, but I did not repeat the loop without the extension or on another machine, so the cause is an inference. The real function makes 5 opens per file (24.9 us each by the table above), which is why the open chain dominates.
- The split of the per-file cost between the open and the read is not stable: an earlier, unrecorded run of cumulative passes (open only; open + fstat; open + fstat + read) charged 14.5 s to the open chain and 0.15 s to the read, with a similar total. The sum of open, read and close was within 5% (14.7 s then, 15.4 s here); the split was not.
- **Hashing is not the bottleneck.** The `sha2` crate runs at 570 MB/s here (364 ms for the 207.6 MB corpus in memory); the system CommonCrypto `CC_SHA256` runs at 2,842 MB/s (73 ms). Parallel or faster hashing alone cannot bring the function near 300 ms.
- **A floor that does not depend on the hook.** If open, read and close were free, the function would still take 516 ms here (enumeration 92 ms, `fstat` 33 ms, hashing 391 ms), above 300 ms. With CommonCrypto for the hash it would be 198 ms.
- Enumeration: measured separately, the index half of `git ls-files` (`-c`) takes 16 ms and the untracked-files walk (`-o`) 81 ms; the real `-co --exclude-standard -z` takes 87 ms, less than the sum because the two share git's start-up (7 ms for `git --version`). Walking every directory with `read_dir` takes 57 ms for the 100,000 entries.

### 3. The stat-cache prototype

Same repository, machine and load gate. The prototype re-reads and re-hashes a file only when its key changed or it is inside the racy window, and it replays the shipped function's 5 MiB and 25 MiB retention rules. Four variants: how the stat pass reaches a file (`fstatat` on parent directories kept open with `O_NOFOLLOW`, the same discipline as the shipped function; or `lstat` on the full path) and how many threads run the stat and read passes (1, or 4 std threads over 500-path chunks, no new dependency). After every run in this table the prototype's snapshot was asserted equal to the shipped function's (paths, hashes, modes, retained bytes).

Median in ms (min to max), 5 runs each, load1 at start 2.80 to 2.98:

| scan | `fstatat(dirfd)`, 1 thread | `lstat(path)`, 1 thread | `fstatat(dirfd)`, 4 threads | `lstat(path)`, 4 threads |
|---|---:|---:|---:|---:|
| First scan, empty cache (every file opened, read, hashed) | 6,203 (6,148 to 6,281) | 6,154 (6,148 to 6,189) | 2,763 (2,728 to 2,779) | 2,774 (2,754 to 2,881) |
| **Repeat, nothing changed** | **277.3 (274.0 to 280.9)** | 251.8 (245.1 to 257.1) | **165.9 (164.7 to 170.2)** | 151.3 (149.9 to 155.9) |
| **One file changed, early in path order** (its content is retained) | **273.5 (270.8 to 276.2)** | 254.9 (246.9 to 258.0) | **166.7 (165.0 to 171.1)** | 153.7 (149.9 to 239.4) |
| **One file changed, late in path order** (hash only) | **274.1 (271.2 to 278.2)** | 262.2 (254.8 to 272.1) | **169.2 (165.7 to 189.2)** | 151.2 (150.4 to 164.6) |

Where a repeat scan spends its time (median ms):

| phase | `fstatat(dirfd)`, 1 thread | `lstat(path)`, 1 thread | `fstatat(dirfd)`, 4 threads | `lstat(path)`, 4 threads |
|---|---:|---:|---:|---:|
| enumerate (`git ls-files`, parse, sort) | 93.1 | 104.9 | 89.2 | 89.1 |
| stat pass over 100,001 paths | 183.6 | 146.7 | 74.9 | 60.7 |
| plan (merge with the previous scan) | 0.6 | 0.7 | 0.7 | 0.7 |
| read and hash | 0.1 | 0.2 | 0.1 | 0.1 |
| assemble | 0.5 | 0.5 | 0.5 | 0.5 |
| **scan total** | 277.3 | 251.8 | 165.9 | 151.3 |
| CPU time of the scan | 246 | 226 | 345 | 307 |
| turning the result into the old `WorkspaceSnapshot` (not in the total) | 4.2 | 4.4 | 5.3 | 5.3 |

- **Warm and one-change scans are under 300 ms in all four variants.** The slowest is the safe one-thread walk: 277 ms (274 to 281), 23 ms under the line, an 8% margin. With four threads it is 166 ms (165 to 170).
- **A changed file costs nothing measurable**: reading and hashing the one file takes 0.1 to 0.2 ms, and the repeat and one-change rows differ by less than the spread of the runs.
- **What dominates a warm scan** is `git ls-files` (89 to 105 ms in every variant, 34% of the one-thread scan and 54% of the four-thread scan) and the stat pass (184 ms for one thread, 75 ms for four). Everything else is under 2 ms. `git` is the floor of this design.
- **The safe walk costs 15 to 26 ms** (252 to 277 ms with one thread, 151 to 166 ms with four) for refusing symlinked parent directories, which the hazard matrix shows path-based `lstat` does not.
- Comparing two scans with the cache (`diff_cache`, entries that share an `Arc` are skipped) takes 0.3 ms, against 18.4 ms for the shipped `diff_snapshots`.
- **The first scan is the part that stays slow**: 6.2 s with one thread and 2.8 s with four, against 15.6 s for the shipped function (2.5 times and 5.6 times faster), because every file still needs an open and a read (59 us per file in the one-thread read pass, 26 us in the four-thread one). The cache saves the files that did not change; the first read of each file is paid once per session.

### 4. What threads buy (std only)

`std::thread::scope` over 500-path chunks, no new dependency. Median ms; 5 runs unless a column says 3; load1 at start 2.87 to 3.00. The machine has 10 cores: 4 "Super" cores (`hw.perflevel0`) and 6 Efficiency cores (`hw.perflevel1`).

| threads | warm stat pass, `fstatat(dirfd)` | warm stat pass, `lstat(path)` | prototype first scan minus enumeration (3 runs) | shipped per-file function, unchanged (3 runs) | SHA-256 and hex of the 207.6 MB, in memory |
|---:|---:|---:|---:|---:|---:|
| 1 | 187.6 | 143.4 | 6,257 | 15,468 | 372.0 |
| 2 | 121.2 | 96.5 | 3,617 | 9,959 | 186.2 |
| 4 | 74.3 | 59.5 | 2,681 | 6,839 | 96.7 |
| 8 | 76.4 | 61.4 | 1,971 | 5,762 | 64.2 |
| 10 | 71.6 | 59.1 | 1,814 | 5,586 | 55.9 |

- **Parallel hashing works and is worth almost nothing.** It scales to 3.8 times at 4 threads and 6.7 times at 10 (372 to 56 ms), but hashing is 2.5% of the shipped function: four threads save 0.28 s of 15.9 s, and in a warm scan there is nothing to hash (0.1 ms).
- **What threads buy is overlapping the filesystem waits.** The shipped per-file function, unchanged, goes from 15.5 s to 6.8 s with 4 threads and 5.6 s with 10 (2.3 and 2.8 times), still 19 times the 300 ms line without a cache. The prototype's first scan goes from 6.3 s to 2.7 s (4 threads) and 1.8 s (10). The warm stat pass goes from 188 ms to 74 ms with 4 threads (2.5 times) and gains nothing past 4, while its CPU time keeps growing (172 ms at 1 thread, 593 ms at 10).
- **Four threads is the knee** on this machine, which has 4 Super cores. (The one-thread `fstatat` row had one 621 ms run among five; its median is 187.6.)
- The shipped-function column excludes enumeration and approximates the retention budget by treating the first 12,600 files as retained.

### 5. A whole turn

Three mutating tool calls in one turn, each editing one file (single runs, load1 at start 2.93 and 2.94). The shipped column is the real `WorkspaceCheckpointManager` with the libSQL checkpoint store; the prototype columns are scans plus the cache diff, without the store.

| step | shipped function | prototype, 1 thread | prototype, 4 threads |
|---|---:|---:|---:|
| turn start (empty cache: the first turn of a session) | 15,384 ms | 6,316 ms | 2,872 ms |
| tool call 0, after the call | 15,628 ms | 277 ms | 174 ms |
| tool call 1 | 15,669 ms | 273 ms | 167 ms |
| tool call 2 | 15,790 ms | 278 ms | 191 ms |
| turn end | 16,021 ms | 274 ms | 182 ms |
| **5 snapshots** | **78,507 ms** | **7,418 ms** | **3,587 ms** |
| the next turn, read-only (turn start and end, warm cache) | 2 snapshots, about 31 s (not run) | 579 ms | 377 ms |

`begin_mutation` itself (copying the baseline) took 3.2 to 5.6 ms per call. In the read-only turn the first scan re-read the 3 files edited a moment before (they were inside the racy window) and the second re-read none. After the first scan, a turn with three mutating calls is five warm scans: about 1.4 s with one thread or 0.8 s with four (5 times the repeat scan), against 78 s.

## Correctness: what fools a stat cache

The prototype's key is, per file, from `lstat` (or `fstatat` with `AT_SYMLINK_NOFOLLOW`): size, mtime (ns), ctime (ns), inode and mode. It also refuses to trust any file whose mtime or ctime was within 2 s of the start of the scan that recorded it (the racy rule git uses for its index), and it reads the key with `fstat` on the open descriptor before reading the bytes, so a later write cannot hide behind it. Unchanged files are not opened.

`stat_cache_hazard_matrix` (runs in the normal test suite, a few seconds) mutates nine files in ways that fool stat caches, rescans with five key designs and compares each result with what is on disk. "Caught" means the cache did not serve a stale entry.

| Mutation | tuple (size, mtime, inode, mode) | + 2 s racy rule | + ctime, `lstat(path)` | + ctime, `fstatat(dirfd)` | control: `stat()` follows symlinks |
|---|---|---|---|---|---|
| edit, same size, mtime advances | caught | caught | caught | caught | caught |
| edit, same size, old mtime put back (`touch -r`, `cp -p`, `rsync -t`, archive extraction) | MISSED | MISSED | caught | caught | MISSED |
| edit within 2 s of the previous scan, mtime put back | MISSED | caught | caught | caught | caught |
| rename over (new inode, same size, old mtime) | caught | caught | caught | caught | caught |
| chmod only | caught | caught | caught | caught | caught |
| append (size changes) | caught | caught | caught | caught | caught |
| file replaced by a symlink to a hard link of its inode | caught | caught | caught | caught | MISSED |
| delete and recreate (same size, old mtime put back) | caught* | caught* | caught | caught | caught* |
| parent directory replaced by a symlink to a directory holding a hard link | MISSED | MISSED | caught (scan refused) | caught (scan refused) | MISSED |

\* Caught because APFS gave the new file a new inode number: of 2,000 inode numbers freed by deleting 2,000 files, 0 were handed to 2,000 new files. On filesystems that reuse inode numbers promptly (ext4 and tmpfs are known to; not tested here) this row is covered only by ctime. The test derives its expectation from whether the inode changed.

What the table shows:

1. **The plain tuple is not enough.** An edit that keeps the size and puts the old mtime back is served from the cache with its old hash, and the 2 s window does not help because the mtime is old. Only ctime, which user space cannot set, catches it. Any tool that sets mtimes explicitly can produce this.
2. **The 2 s rule works, but this filesystem cannot show why it exists.** APFS timestamps are fine-grained: 5,000 same-size rewrites in a tight loop gave 5,000 distinct mtimes (smallest step 43 microseconds, median 48). The rule is for coarse-timestamp filesystems (HFS+, FAT, NFS and similar), which I could not test here; the matrix forces the same effect by putting the mtime back.
3. **`lstat`, not `stat`.** A following `stat` served the stale entry for a file that had become a symlink to a hard link of its old inode.
4. **Path-based `lstat` follows a symlinked parent directory.** The fd-relative walk (parents opened one by one with `O_NOFOLLOW`) refuses it, as the shipped function does ("could not safely open"). Hard-linking bumps ctime, which is why the ctime variants also catch the case, by chance.
5. **Git-ignored directories** are invisible to the real snapshot and to the cache alike. A directory that becomes ignored leaves the cache (the cache is rebuilt from the listed paths on every scan), and a file rewritten while its directory was ignored (same size, old mtime) comes back with its current hash when the directory is un-ignored (`ignored_directories_do_not_leak_stale_entries`).
6. **A file dated in the future** is inside the racy window forever and is re-read on every scan: slow for that file, never stale (`future_mtime_is_always_reread`).
7. **The prototype equals the shipped function** on a tree that exercises both retention caps (`prototype_matches_the_real_snapshot`): same paths, hashes, modes and retained bytes after a first scan, after an unchanged rescan (0 files re-read) and after an edit, a delete, a create, a chmod and a rewrite of a 4.5 MiB file (4 re-read). Deleting a retained file lets a later file fit in the 25 MiB budget, so that file must be re-read: the retention rule couples files to each other.
8. **A defect in the shipped function.** In a git repository, any symlink that git lists (tracked, untracked or dangling) makes `capture_snapshot` return `Err("could not safely open <link>: Too many levels of symbolic links")`, because the file is opened with `O_NOFOLLOW` and only `NotFound` is tolerated. The caller then prints "workspace checkpoint unavailable for this turn", so such a repository gets no workspace checkpoints at all. Without git, the directory-walk fallback skips symlinks and works. The prototype skips symlinks in both cases. P2c has to decide the behaviour (test `real_snapshot_errors_on_any_symlink_in_a_git_repo` records today's).

## Limits of these numbers

1. **One machine.** Apple M5, macOS 26.6.2, APFS, with an Endpoint Security extension. Linux (where the harness images run) and Windows were not measured, and these numbers do not predict them. Windows needs its own measurement: `std` gives no inode or ctime there without opening the file, so the key differs, and per-file metadata calls usually cost more than on APFS.
2. **The shipped function's 16 s is mostly a property of this machine's per-open cost, and I could not remove the suspected cause.** `open` plus `close` of `/dev/null` costs 27.7 us against 0.9 us for `lstat`, and half of it is blocked time. I did not repeat the loop with the extension off or on another machine. Without such a hook the shipped function would probably be much faster, but the floor in section 2 (516 ms for enumeration, `fstat` and hashing alone) is still above 300 ms. The warm stat-cache scan makes no file opens (it uses `lstat`, 0.9 us here), so I expect its numbers to travel better than the shipped function's; that is an inference, not a measurement.
3. **Noise.** The baseline load was 3.0 to 3.5 and the Trend Micro service ran hot for the later cases. The spread inside a row (for example 15,979 to 16,625 ms for the repeat case) and the shift of cost between open, read and close in section 2 say that differences under about 5% between rows mean nothing.
4. **Synthetic repository.** 2 KB random text files, four directory levels, 100 files per directory, 10,000 ignored files. Real repositories have larger files (more hashing and more retained bytes), deeper trees (a longer open chain in the shipped function) and more binary files. The 25 MiB cap keeps the bytes of only the first 12,664 files in path order.
5. **No true cold cache** (see Method), and small samples: 5 runs per case, 3 for some sweeps, 1 for each turn row. Medians and ranges are given, not confidence intervals.
6. **The prototype is a spike**: Unix only, no cancellation, no persistence between processes, and the libSQL store is not in its turn rows. The `.gitignore` tests do not cover submodules, worktrees, sparse checkouts or `core.fsmonitor`.

## Recommendation for P2c

**Size: supports the current 6 to 7 engineer-weeks. No increase (the +2 weeks is not triggered) and no decrease.**

- *Why not more.* Every prototype variant is under 300 ms warm and with one changed file; the slowest, one thread with the safe walk, is 277 ms. The prototype is std only: about 600 lines for the walker, key, planner, thread pool and cache, plus about 470 lines of correctness tests, and it already equals the shipped function including both retention caps.
- *Why not less.* The snapshot is one of P2c's four deliverables (`CancelToken`, one shutdown path and panic hook, the snapshot, the PTY rig), and this spike says nothing about the other three. My estimate, which is judgment and not a measurement, is 1.5 to 2 of the 6 to 7 weeks for the snapshot once it is integrated with `WorkspaceCheckpointManager` and the store, tested and kept off the cancel path.

What the snapshot item has to contain, and the evidence for each part:

1. **Key and racy rule.** Size, mtime (ns), ctime (ns), inode and mode from `lstat`; distrust any file whose newest timestamp is within 2 s of the start of the scan that recorded it; `fstat` the open file before reading it. In the hazard matrix the plain tuple serves a stale hash in 3 of 9 cases, with the 2 s rule in 2 of 9, with ctime in 0 of 9.
2. **An fd-relative walk** (parents opened one by one with `O_NOFOLLOW` and kept open across the sorted paths). It costs 15 to 26 ms more than path-based `lstat`, and it is the only variant that refuses a symlinked parent directory without help from ctime.
3. **Four threads** for the stat and read passes (std scoped threads over 500-path chunks): 2.5 times on the stat pass, nothing beyond 4 threads. No parallel hashing: hashing is 2.5% of the shipped function and 0.1 ms of a warm scan.
4. **Fix the symlink abort.** Decide what a symlink among the listed paths means (skip it, as the non-git walker and the prototype do, or record its target) and add the regression test. Today one symlink in a git repository switches workspace checkpoints off.
5. **Keep the scan off the cancel path, and say what runs on cancel.** A fast scan (166 to 277 ms) is about the whole 300 ms developer-machine cancel budget in P2c's own exit criteria. A cancelled turn should skip or defer the turn-end scan, and the exit criterion about rewind conflicts needs the post-turn hash comparison unchanged.
6. **The first scan.** 6.2 s with one thread and 2.8 s with four, against 15.6 s today. Run it as a background warm-up when the session starts so the first prompt does not wait for it. Do not persist the cache across sessions in P2c: it would need the previous scan's start time and the retained bytes persisted too, and every hazard in the table would then span process restarts and other writers.
7. **Content retention** stays coupled to scan order (the bytes of the first files by path, 25 MiB in all; deleting a retained file makes a later file be re-read). The prototype replays the rule to stay equal to the shipped function. P7b should replace it with pre-images taken when a file is first about to change.
8. **Keep the hazard matrix as a regression suite** (`stat_cache_hazard_matrix` and its siblings run in the normal test run), and add a report-only timing row for a warm scan of the 100k repository rather than a CI gate: comparable rows on this machine differed by up to 4% (15.6 s against 16.1 s for the same function, first against repeat).

**Where the risk moved: Windows.** Before P2d is sized, a small separate spike should run the equivalent of `stat_pass` and the warm scan on windows-2025 with an NTFS repository of 100,000 files (this module is Unix only). If the stat pass costs more than about 200 ms there (300 ms minus roughly 90 ms for git and a little overhead), the contingency belongs to P2d, not P2c.

**Plan text to correct when the sizes are revised.** The plan describes the snapshot cost as hashing: "each snapshot hashes every non-ignored file" (the diagram in section 1), "reads and hashes every file git lists" (section 5.8 and A24) and "every non-ignored file is hashed" (the risk table in section 10). Hashing is 2.5% of the cost; the cost is opening and reading every file (97%), and `git ls-files` is the floor of any design that keeps it (89 to 105 ms). A24 should also record the symlink abort and the measured 15.6 s.

## Reproduce

```sh
export CARGO_TARGET_DIR=<a private directory>   # never a target directory shared with other worktrees
python3 -I docs/spikes/p0-stat-cache-gen-repo.py /path/repo100k
cp -cR /path/repo100k /path/repo100k-work
cargo test --release --locked --no-default-features -p craftsman-rs --bin craftsman --no-run   # prints the test binary
CRAFTSMAN_SPIKE_REPO=/path/repo100k CRAFTSMAN_SPIKE_WORK=/path/repo100k-work CRAFTSMAN_SPIKE_OUT=/path/results.jsonl \
  docs/spikes/p0-stat-cache-run.sh <test binary>
python3 -I docs/spikes/p0-stat-cache-table.py /path/results.jsonl
cargo test --locked --no-default-features -p craftsman-rs stat_cache_spike    # the correctness tests, debug build
```
