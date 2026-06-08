<!-- SPDX-License-Identifier: Apache-2.0 -->
# ferro-blob-store mutation-kill rationale

Baseline before this wave: **73.1% kill (14 missed of 52 viable)**.
Source of truth: `dd-pack/mutation/ferro-blob-store/mutants.out/missed.txt`.

Scope of this wave: **tests only** under `crates/ferro-blob-store/src/**`
(`#[cfg(test)]` modules). No library logic was changed.

All 14 previously-missed mutants now have a dedicated killing test that
asserts the exact observable behaviour the mutant breaks (concrete values,
not `is_ok()`). None were classified as by-design un-killable — i.e. the
irreducible-remainder set is **empty** for this crate.

## Mutant → killing test map

| # | Mutant (file:line) | Replacement | Killing test | Why it kills |
|---|--------------------|-------------|--------------|--------------|
| 1 | `digest.rs:100` `<TryFrom<String>>::try_from` | `Ok(Default::default())` | `digest::tests::try_from_string_round_trips_exact` | Asserts parsed digest has the SAME algo/hex and round-trips to the SAME wire string for both sha256 and sha512, and that malformed input is rejected. A default/empty placeholder fails every value assertion (and the reject case). |
| 2 | `digest.rs:107` `<From<Digest>>::from` | `Default::default()` | `digest::tests::into_string_is_exact_wire_form` | Asserts the produced String equals the exact `sha256:<hex>` wire form, is non-empty, and parses back to the same Digest. An empty default String fails. |
| 3 | `memory.rs:38` `is_empty` | `true` | `memory::tests::is_empty_tracks_contents` | Asserts `!is_empty()` after a put; a constant `true` fails. |
| 4 | `memory.rs:32` `len` | `1` | `memory::tests::len_counts_distinct_puts` | Asserts `len()==0` on a fresh store and `==i+1` / `==3` after distinct puts; a constant `1` fails the 0 and 3 cases. |
| 5 | `memory.rs:81` `delete` | `Ok(())` | `memory::tests::delete_actually_removes` | Asserts that after delete the blob is gone: `!contains`, `get` errors, `len()==0`, `is_empty()`. A no-op stub leaves the entry present. |
| 6 | `fs.rs:43` `tmp_dir` | `Default::default()` | `fs::tests::tmp_dir_is_under_root` | Asserts the tmp dir != empty `PathBuf`, is under the store root, ends in `.tmp`, and equals `<root>/sha256/.tmp`. An empty default path fails. |
| 7 | `fs.rs:101` `get` match-guard `e.kind()==NotFound` | `true` | `fs::tests::fs_get_non_not_found_error_propagates` | Plants a *directory* at the blob path so `read` yields a non-NotFound Io error; asserts the error is NOT `NotFound` and IS `Io(_)`. With the guard forced `true` the error would be swallowed into `NotFound`. |
| 8 | `fs.rs:109` `contains` | `Ok(true)` | `fs::tests::fs_contains_absent_is_false` | Asserts `contains(absent)==false` (and `contains(present)==true`). A constant `Ok(true)` fails the absent case. |
| 9 | `fs.rs:112` `contains` match-guard `==NotFound` | `true` | `fs::tests::fs_contains_non_not_found_error_propagates` | Makes the 2-char prefix path a *file* so `metadata` traversal yields a non-NotFound error; asserts it propagates as `Io(_)`. Guard forced `true` would mis-map it to `Ok(false)`. |
| 10 | `fs.rs:112` `contains` match-guard `==NotFound` | `false` | `fs::tests::fs_contains_absent_is_false` | For an absent blob the kind IS NotFound; forcing the guard `false` falls through to the `Err(e)` arm, so `contains(absent)` would return `Err` and the test's `.unwrap()` panics. Original returns `Ok(false)`. |
| 11 | `fs.rs:112` `contains` `== → !=` | `!=` | `fs::tests::fs_contains_absent_is_false` | Same absent-blob case: `!=` makes the NotFound arm not match, returning `Err`; `.unwrap()` on the expected `Ok(false)` panics. |
| 12 | `fs.rs:118` `delete` | `Ok(())` | `fs::tests::fs_delete_removes_and_propagates_real_errors` | Asserts a present blob is actually removed after delete (`!contains`, `get` errors). A no-op stub leaves it present. |
| 13 | `fs.rs:121` `delete` match-guard `==NotFound` | `true` | `fs::tests::fs_delete_removes_and_propagates_real_errors` | Removes a *directory* (via `remove_file`) → non-NotFound Io error; asserts it surfaces as `Io(_)`. Guard forced `true` would swallow it as `Ok(())`. |
| 14 | `fs.rs:147` `collect_algo` `|| → &&` | `&&` | `fs::tests::fs_list_skips_non_two_char_prefix_dir` | Plants a stray 3-char hex prefix dir (`aaa/`) holding a 61-char hex file → 64 hex total. With OR, `len != 2` alone skips it, so the bogus digest must NOT be listed. With AND the dir would be descended and the (structurally valid) digest listed. Test asserts the genuine digest is present and the bogus one absent. |

## Notes / caveats

- **Platform dependence of the non-NotFound guard kills** (mutants 7, 9,
  13). These rely on the OS returning an `io::ErrorKind` other than
  `NotFound` when a path component is a directory/file in the wrong shape
  (e.g. `NotADirectory` / `IsADirectory` style). This holds on Linux (the
  CI/dev target here) and the tests pass; the assertions are written
  against the *variant* (`BlobStoreError::Io(_)` and "not NotFound"), not
  a specific raw `ErrorKind`, so they remain robust across Unix-likes. If
  ever run on an exotic platform that maps these to `NotFound`, the tests
  would still pass (no false failure) but those three guard mutants could
  resurface as missed — re-evaluate per target if that ever occurs.

- **By-design un-killable set: none.** Every viable missed mutant for this
  crate is observably distinguishable from the original, so no
  defensive-cascade / bitwise-equivalent / exact-boundary / lifecycle /
  unreachable-by-construction classifications were needed.

## Expected outcome

- Expected newly killed: **14 / 14**.
- Expected remaining by-design un-killable: **0**.
- Projected kill rate: **52 / 52 viable = 100%** (orchestrator's central
  `cargo mutants` re-run is the authority).
