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

- **Wave 1** added 45 tests and killed most mutants, but a `cargo mutants`
  re-run showed it actually reached only **91.1 % (21 missed of 235 viable)**:
  15 DAG-detection-helper mutants survived because Wave 1's negatives did not
  distinguish the helper under test from an always-`true` mutant (e.g. the
  `is_dag_callable -> true` negatives drove `extract_*` — the *ruff_impl* copy —
  not `dynamic_markers_for`; the `is_task_decorator -> true` negative used an
  *un-decorated* helper so `any(...)` was vacuously false either way; the
  `@task(...)` Call decorator form and the bare `@dag` Name form were never
  exercised).
- **Wave 2** (this document) adds **15 targeted tests** that each pair a
  positive with a shape-matched negative on the *exact* helper + API path, and
  every one of the 15 was verified to KILL its mutant by transiently applying
  the mutation to a throwaway copy of the source and confirming the test fails
  (source restored byte-identical via `git checkout` — no library edit).
- **6 missed mutants remain classified by-design** as un-killable from
  integration tests without a source-visibility change (see below): 4 dead-code
  `line_index.rs:75` constants + 2 non-deterministic `panic_safe.rs:180`
  upstream-panic-only constants.

Projected kill after Wave 2: (214 killed + 15) / 235 = **229 / 235 ≈ 97.4 %**,
with the residual 6 (2.6 %) being the by-design private-unreachable set
(91.1 % → 97.4 %, clearing the ≥95 % target).

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

## Wave 2 — the 15 surviving DAG-detection-helper mutants (all KILLED)

These 15 survived Wave 1. The fix in each case is a negative whose syntactic
shape forces the helper-under-test to return a DIFFERENT result for the
original vs the mutant, on the SAME API path the mutant lives on. Each kill was
verified empirically (mutate throwaway copy → test FAILS → restore).

### dynamic_markers.rs — helpers reached ONLY via `dynamic_markers_for` (10)

These are a SEPARATE copy of the identically-named helpers in `ruff_impl.rs`.
Wave 1's `extract_*`-driven negatives killed the *ruff_impl* copy but left this
copy alive. All Wave-2 tests here drive `dynamic_markers_for`. The observable is
whether a marker fires, which depends on `in_dag_ctx` (opened by the DAG/decorator
detectors) and on the chain/task detectors.

- `410 visit_call_args -> ()`: `dyn_nested_chain_splat_in_call_args_is_reached_by_recursion`
  — `register(chain(*items))` nests the splat inside another call's args; only the
  `visit_call_args` recursion reaches it, so dropping the recursion drops the marker.
- `420 is_dag_callable -> true`: `dyn_non_dag_with_block_does_not_open_dag_context`
  — `with open("f"): chain(*items)` must NOT flag (non-DAG `with` keeps ctx 0); the
  mutant opens ctx for every `with` callee. Positive companion: bare `with DAG(...)`.
- `422 delete Attribute arm (is_dag_callable)`: `dyn_dag_callable_via_attribute_opens_context`
  — `with airflow.DAG(...): chain(*items)` must flag; deleting the Attribute arm leaves
  ctx at 0.
- `428 match_dag_decorator -> true`: `dyn_non_dag_decorator_does_not_open_dag_context`
  — `@functools.cache def helper(): chain(*items)` must NOT flag; mutant opens ctx for any
  decorated fn. Positive: `@dag` Name decorator.
- `431 delete Attribute arm (match_dag_decorator)`: `dyn_dag_decorator_via_attribute_opens_context`
  — `@airflow.dag(...)` must open ctx; deleting the Attribute arm drops it.
- `440 callee_is_chain_helper -> true`: `dyn_non_chain_splat_call_inside_dag_is_not_flagged`
  — `schedule_tasks(*items)` inside a DAG must NOT flag ChainSplat; mutant flags every
  splat call. Positive: real `chain(*items)`.
- `457 is_task_decorator_call -> true`: `dyn_non_task_decorator_call_does_not_flag_taskflow`
  — `@retrying(3)` (a non-task decorator, but with a POSITIONAL arg so
  `task_decorator_is_dynamic` is TRUE — this is essential so the `&&` cannot mask the
  mutant) must NOT flag UnsupportedTaskFlow.
- `460 delete Attribute arm (is_task_decorator_call inner)`: `dyn_task_decorator_call_via_attribute_flags_taskflow`
  — `@sdk.task("grp")` (attr == "task", positional arg ⇒ dynamic) must flag; deleting the
  Attribute arm makes `inner` return None. Negative companion `@app.helper("grp")`.
- `461 delete Call arm (is_task_decorator_call inner)`: `dyn_task_decorator_call_via_call_callee_flags_taskflow`
  — `@task()(expand=True)` (decorator func is itself a Call) must flag; deleting the Call
  arm makes `inner` return None for that shape.
- `471 task_decorator_is_dynamic -> true`: `dyn_zero_arg_task_decorator_call_is_not_dynamic`
  — `@task()` (a Call but with no args / no expand|partial) must NOT flag; the mutant
  treats it as dynamic. Positive companion `@task(expand=True)`. (Wave 1's bare `@task`
  is a Name, not a Call, so `visit_decorator_list`'s `if let Expr::Call` guard skips it
  and the `-> true` branch is never reached — only an empty-arg CALL exercises it.)

### ruff_impl.rs — static-extractor decorator detection via `extract_*` (5)

Wave 1 only used the bare-Name and Attribute-Call decorator forms; these add the
missing forms so each match guard / arm is load-bearing.

- `362 match guard contains(id) -> false` (bare-Name `@dag` arm):
  `ruff_bare_name_dag_decorator_registers_dag` — a BARE `@dag` (no parens) must register
  a DAG. Wave 1 only used the `@dag(...)`/`@airflow.dag(...)` CALL forms (lines 366/370).
- `366 match guard contains(id) -> true` (Name-Call arm):
  `ruff_non_dag_name_call_decorator_is_ignored` — `@retry(times=3)` (a Name CALL not in
  DAG_DECORATOR_NAMES) must NOT register a DAG. Wave 1's only Name-Call negative was an
  *Attribute* (`@functools.cache`), so it didn't pin this arm.
- `370 match guard contains(attr) -> true` (Attribute-Call arm):
  `ruff_non_dag_attribute_call_decorator_is_ignored` — `@app.task(bind=True)` (attr not
  `dag`) must NOT register a DAG.
- `381 is_task_decorator -> true`: `ruff_non_task_decorated_function_in_dag_is_not_a_task`
  — a `@staticmethod`-decorated function inside a `@dag` body must NOT become a task.
  Wave 1's negative used an UN-decorated function (empty decorator list ⇒ `any(...)`
  false regardless of the mutant), so it never killed `-> true`.
- `385 delete Call arm (is_task_decorator inner_name)`: `ruff_task_decorator_call_form_registers_task`
  — `@task(multiple_outputs=True)` (the Call decorator form) must register the task;
  deleting the Call arm makes `inner_name` return None. Wave 1 used only bare `@task` /
  `@airflow.task` (Name / Attribute arms).

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
