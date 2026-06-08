# ferro-airflow-dag-parser — mutation-kill rationale

Baseline: `cargo mutants` reported **71.5 % kill (67 missed of 235 viable)**.
Source: `dd-pack/mutation/ferro-airflow-dag-parser/mutants.out/missed.txt`.

## Constraint

This crate is shared with a concurrent `ferro-air` session, so **no library
source was edited** — not even `#[cfg(test)]` blocks. All new coverage lives in
a single new integration file, `crates/ferro-airflow-dag-parser/tests/mutation_guard.rs`,
which drives only the crate's PUBLIC surface:

- `extract_static_dag`, `extract_all_static_dags` (api re-exports)
- `dynamic_markers_for` / `detect_dynamic_markers`
- `parse_dag_path`, `ParseOutcome::source_hash`
- `ParseCache`
- `ruff_impl::extract` (exported via `pub mod ruff_impl`)

Because the source is off-limits, a mutant in a private helper that is **not
transitively reachable** from those entry points is classified by-design rather
than killed (making the helper `pub` would be a source change that could
conflict with the concurrent session — explicitly disallowed by the task).

## Result

- **61 missed mutants killed** via public-API integration tests (added in
  `tests/mutation_guard.rs`, 45 tests).
- **6 missed mutants classified by-design** as un-killable from integration
  tests without a source-visibility change (see below).

Projected kill after this wave: (235 − 67 + 61) / 235 = **229 / 235 ≈ 97.4 %**,
with the residual 6 (2.6 %) being the by-design private-unreachable set.

---

## Killed via public API (61)

Grouped by source file; each group maps to the named tests.

### common.rs — `validate_safe_identifier` `>` → `>=` (1)
`dag_id_at_exact_max_len_is_accepted` (250 chars accepted) +
`dag_id_one_over_max_len_is_rejected` (251 rejected, length reported) +
`task_id_at_exact_max_len_is_accepted`. The 250/251 boundary pins `len > max_len`
from both sides, so the `>=` mutant (which rejects exactly-250 valid ids) dies.

### panic_safe.rs — bracket / unary caps reached via the extractor (5)
The module is private, but `extract_all_static_dags` → `ruff_impl::extract_all`
→ `parse_module_safely` runs both pre-screens on every parse, so the caps are
fully reachable with pathological source.

- `132 > → >=` and `132 > → ==` (`max_bracket_depth_exceeds`):
  `bracket_depth_exactly_at_cap_passes` feeds depth **32**. Under `>=` (`32>=32`)
  and under `==` (`32==32`) the input is wrongly rejected; the real `>` lets it
  through. `bracket_depth_one_over_cap_rejected` (33) and
  `bracket_depth_far_over_cap_rejected` (200) pin rejection on the high side.
- `136 delete match arm `)`/`]`/`}``: `closing_brackets_reduce_depth_so_balanced_input_passes`
  feeds 40 balanced `(1)` pairs. Without the closing arm depth climbs to 40 (> 32)
  and the balanced input is wrongly rejected.
- `164 > → >=` and `164 > → ==` (`max_unary_op_run_exceeds`):
  `unary_op_run_exactly_at_cap_passes` feeds a run of **64** (rejected under both
  `>=` and `==`, accepted by real `>`); `unary_op_run_far_over_cap_rejected` (300)
  pins the high side.

### cache.rs — `hash_source` `^=` → `|=` (1)
`source_hash_matches_exact_fxhash_xor_value` parses a 31-byte file via
`parse_dag_path` and asserts the exact XOR digest `4_461_567_911_320_149_738`.
The `|=` mutant only sets bits and yields a different digest (the byte `0x61`
distinguishes them). `distinct_sources_hash_differently` reinforces per-byte
mixing.

### ruff_impl.rs — walker, matching, edges, stringify (≈30)
- `55 extract → Ok(Default::default())`: `ruff_impl_extract_returns_first_dag_not_default`
  calls the public `ruff_impl::extract` and asserts the real `dag_id`.
- `91 delete AnnAssign arm`: `ann_assign_target_collects_task`.
- `101 delete ClassDef|If|For|While|Try arm`: `nested_class_body_is_walked_for_dags`
  (a DAG under `if True:` must still be found).
- `226 == → !=` (set_downstream direction): `set_downstream_records_directed_edge`
  ( (a,b) ) vs `set_upstream_records_reversed_edge` ( (b,a) ).
- `301 delete Call arm` / `302 delete Attribute arm` in `resolve_to_task_id`:
  `setter_arg_resolved_through_call_and_attribute` (`up().set_downstream(down.output)`
  resolves a Call operand and an Attribute operand to edge (up,down)).
- `346 delete Attribute arm` / `is_dag_callable → true` (343):
  `dag_callable_via_attribute_is_recognized` (positive) +
  `non_dag_attribute_call_is_not_a_dag` (negative).
- `362/366/370 match_dag_decorator guards`, `delete Attribute arm`:
  `dag_decorator_via_attribute_is_recognized` (positive) +
  `bare_name_decorator_that_is_not_dag_is_ignored` (negative `@functools.cache`).
- `381 is_task_decorator → true`, `384/385 delete Attribute/Call arms`:
  `task_decorator_via_attribute_collects_function_name` (positive) +
  `non_task_decorated_function_is_not_a_task` (negative).
- `417/418/419/424/425/428 stringify_expr arms` (None/bool/number/Name/Attribute/Call):
  `schedule_stringifies_each_literal_kind` pins all six exact renderings
  (`None`, `true`, `5`, `legacy`, `module.timetable`, `Timetable(...)`).
- `449 push_unique_edge` `&& → ||`, `== → !=` ×2: `duplicate_edges_are_deduplicated`
  (identical edge collapses to one) + `distinct_edges_sharing_an_endpoint_are_all_kept`
  ( (a,b) and (a,c) both kept — the `||` mutant would drop the second).

### dynamic_markers.rs — visitor, line/col, callable matching (≈24)
- `181 line_col → (0,0)/(0,1)/(1,0)/(1,1)` (4): `path_stem_marker_reports_exact_line_and_col`
  pins (2,17); `fstring_task_id_marker_reports_exact_line_and_rendering` pins line 4;
  `dynamic_schedule_marker_reports_exact_line` pins line 2. The exact non-(0,0)/(1,1)
  coordinates kill all four constant replacements.
- `196 / 213 -= → += | /=` (4): `dag_context_does_not_leak_past_the_with_block` and
  `dag_decorator_context_does_not_leak_past_the_function` — a trailing
  `chain(*items)` after the block must NOT flag; a broken decrement leaves the
  context "open" and flags it.
- `216 delete Assign arm` / `222 delete AnnAssign arm`:
  `assign_value_inside_dag_is_walked_for_markers` and
  `ann_assign_value_inside_dag_is_walked_for_markers` (chain splat on an
  assignment / annotated-assignment RHS).
- `257 delete While arm` / `261 delete Try|ClassDef arm`:
  `while_and_try_bodies_are_walked_for_dag_context` (markers nested under
  `while` and `try` bodies must survive).
- `381 > → >=`, `381 && → ||` in `visit_call`:
  `chain_splat_outside_dag_is_not_flagged` — module-scope `chain(*items)`
  (in_dag_ctx == 0) must NOT flag; `>=` would fire at depth 0 and `||` would
  fire because the helper name matches. `chain_splat_inside_dag_is_flagged`
  pins the positive.
- `410 visit_call_args → ()`: nested markers (f-string inside an operator call
  argument, chain helper inside an assignment value) are only reached through
  `visit_call_args` recursion — `assign_value_inside_dag_is_walked_for_markers`
  and the f-string test exercise it.
- `419/420/422 is_dag_callable → true / delete Attribute arm`:
  `chain_splat_inside_dag_is_flagged` (DAG opened) +
  `chain_splat_outside_dag_is_not_flagged` and the import-time pair below
  (negative — a non-DAG context manager must not open `in_dag_ctx`).
- `428/431 match_dag_decorator → true / delete Attribute arm`: covered via the
  `@dag` / `@airflow.dag` positive paths plus the scope-leak negatives.
- `440/442 callee_is_chain_helper → true / delete Attribute arm`:
  `chain_helper_via_attribute_is_recognized` (positive Attribute callee) +
  `chain_splat_outside_dag_is_not_flagged` (negative).
- `448/450 is_operator_constructor → true / delete Attribute arm`:
  `for_loop_operator_construction_is_flagged_via_attribute` (positive Attribute
  callee) + `for_loop_non_operator_call_is_not_flagged` (negative `print(i)`).
- `457/460/461 is_task_decorator_call → true / delete Attribute/Call arms`:
  `taskflow_expand_decorator_is_flagged_but_bare_task_is_not` +
  `taskflow_decorator_call_with_only_positional_arg_is_dynamic`.
- `471 task_decorator_is_dynamic → true`, `471 delete !`:
  `taskflow_expand_decorator_is_flagged_but_bare_task_is_not` — bare `@task`
  (args empty, no expand/partial) must NOT flag, but `@task(expand=True)` must;
  dropping the `!` or returning `true` flips one of those.
- `493 is_constant_bool → false`: `import_time_branching_under_nonconstant_if_is_flagged`
  (non-constant test flags) + `constant_if_guarding_dag_is_not_branching`
  (`if True:` must NOT flag) — the `false` mutant makes every test "non-constant".
- `500 render_fstring → String::new() / "xyzzy"`:
  `fstring_task_id_marker_reports_exact_line_and_rendering` asserts the exact
  rendering `t_{…}`.

---

## Classified by-design — not killable from integration tests (6)

These mutants sit in private helpers that the public API cannot reach
deterministically. Per the shared-crate constraint, the helpers are **not made
`pub`** to kill them (that would be a source change that could conflict with the
concurrent `ferro-air` session). This is a legitimate by-design classification
for this crate.

| Mutant | Helper | Why unreachable |
| --- | --- | --- |
| `line_index.rs:75 ruff_line_col → (0,0)` | `ruff_line_col` | Dead code. The function carries an explicit `#[allow(dead_code)]` and a repo-wide grep finds **no call site**. It is `pub(crate)` and never invoked, so no test — integration or even in-crate — can observe its output. The live line/col path used by every marker is `LineIndex::line_col` / `MarkerVisitor::line_col`, which **is** covered (and those constant-replacement mutants are killed by the exact-coordinate assertions above). |
| `line_index.rs:75 ruff_line_col → (0,1)` | `ruff_line_col` | Same dead-code helper. |
| `line_index.rs:75 ruff_line_col → (1,0)` | `ruff_line_col` | Same dead-code helper. |
| `line_index.rs:75 ruff_line_col → (1,1)` | `ruff_line_col` | Same dead-code helper. |
| `panic_safe.rs:180 panic_message → String::new()` | `panic_message` | `panic_safe` is a **private module** (`mod panic_safe;`), so `panic_message` is not callable from an integration test. It is invoked only on the `catch_unwind` **panic** branch of `parse_module_safely`, i.e. only when the *upstream* `ruff_python_parser` panics. Whether any given input still panics depends on the pinned upstream version; the crate's own `shim_catches_upstream_parser_panic` test already accepts `Ok` / `Parse` / `Internal` precisely because the panic is no longer guaranteed to reproduce. The returned string is observable only inside the `ParseError::Internal` message, so this cannot be pinned deterministically from a public-API test. |
| `panic_safe.rs:180 panic_message → "xyzzy".into()` | `panic_message` | Same non-deterministic upstream-panic-only path. |

### Notes on near-equivalent caps (killed, not classified)

The bracket/unary `> → ==` mutants look equivalent at first glance (depth is
incremented by 1, so `> limit` and `== limit` reject the same *deeply* nested
inputs, differing only in the reported `Some(N)` value). They are **not**
equivalent here because an exactly-at-cap input (depth 32 / run 64) is rejected
under `==`/`>=` but accepted under the real `>`; the
`*_exactly_at_cap_passes` tests exploit exactly that gap, so these mutants are
killed rather than classified.
